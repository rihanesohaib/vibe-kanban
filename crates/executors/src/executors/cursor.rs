use core::str;
use std::{collections::HashMap, path::Path, process::Stdio, sync::Arc, time::Duration};

use async_trait::async_trait;
use command_group::AsyncGroupChild;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command};
use ts_rs::TS;
use workspace_utils::{
    command_ext::GroupSpawnNoWindowExt,
    diff::{create_unified_diff, normalize_unified_diff},
    msg_store::MsgStore,
    path::make_path_relative,
    shell::resolve_executable_path_blocking,
};

use crate::{
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executor_discovery::ExecutorDiscoveredOptions,
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, SpawnedChild,
        StandardCodingAgentExecutor,
    },
    logs::{
        ActionType, FileChange, NormalizedEntry, NormalizedEntryError, NormalizedEntryType,
        TodoItem, ToolStatus,
        plain_text_processor::PlainTextLogProcessor,
        utils::{
            ConversationPatch, EntryIndexProvider, patch, shell_command_parsing::CommandCategory,
        },
    },
    model_selector::ModelSelectorConfig,
    profile::ExecutorConfig,
    stdout_dup::create_stdout_pipe_writer,
};

mod mcp;
pub mod models;
const CURSOR_AUTH_REQUIRED_MSG: &str = "Authentication required. Please run 'cursor-agent login' first, or set CURSOR_API_KEY environment variable.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema)]
pub struct CursorAgent {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Force allow commands unless explicitly denied")]
    pub force: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Model id as listed by `cursor-agent --list-models`, e.g. auto, claude-opus-5-thinking-high, gpt-5.6-sol-high-fast. Leave empty for the CLI default."
    )]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Legacy reasoning level for older model ids; current model ids already encode effort."
    )]
    pub reasoning: Option<String>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
}

// Maps legacy (base model, reasoning) pairs saved in older configs to full ids.
// Current CLI ids already encode effort and pass through unchanged.
fn resolve_cursor_model_name<'a>(base_model: &'a str, reasoning: Option<&'a str>) -> &'a str {
    match (base_model, reasoning) {
        ("gpt-5.4", Some("medium")) => "gpt-5.4-medium",
        ("gpt-5.4", Some("high") | None) => "gpt-5.4-high",
        ("gpt-5.4", Some("xhigh")) => "gpt-5.4-xhigh",

        ("gpt-5.4-fast", Some("medium")) => "gpt-5.4-medium-fast",
        ("gpt-5.4-fast", Some("high") | None) => "gpt-5.4-high-fast",
        ("gpt-5.4-fast", Some("xhigh")) => "gpt-5.4-xhigh-fast",

        ("gpt-5.3-codex", Some("low")) => "gpt-5.3-codex-low",
        ("gpt-5.3-codex", Some("medium")) => "gpt-5.3-codex",
        ("gpt-5.3-codex", Some("high") | None) => "gpt-5.3-codex-high",
        ("gpt-5.3-codex", Some("xhigh")) => "gpt-5.3-codex-xhigh",

        ("gpt-5.3-codex-fast", Some("low")) => "gpt-5.3-codex-low-fast",
        ("gpt-5.3-codex-fast", Some("medium")) => "gpt-5.3-codex-fast",
        ("gpt-5.3-codex-fast", Some("high") | None) => "gpt-5.3-codex-high-fast",
        ("gpt-5.3-codex-fast", Some("xhigh")) => "gpt-5.3-codex-xhigh-fast",

        ("gpt-5.2-codex", Some("low")) => "gpt-5.2-codex-low",
        ("gpt-5.2-codex", Some("medium")) => "gpt-5.2-codex",
        ("gpt-5.2-codex", Some("high") | None) => "gpt-5.2-codex-high",
        ("gpt-5.2-codex", Some("xhigh")) => "gpt-5.2-codex-xhigh",

        ("gpt-5.2-codex-fast", Some("low")) => "gpt-5.2-codex-low-fast",
        ("gpt-5.2-codex-fast", Some("medium")) => "gpt-5.2-codex-fast",
        ("gpt-5.2-codex-fast", Some("high") | None) => "gpt-5.2-codex-high-fast",
        ("gpt-5.2-codex-fast", Some("xhigh")) => "gpt-5.2-codex-xhigh-fast",

        ("gpt-5.2", Some("medium")) => "gpt-5.2",
        ("gpt-5.2", Some("high") | None) => "gpt-5.2-high",

        ("gpt-5.1-codex-max", Some("medium")) => "gpt-5.1-codex-max",
        ("gpt-5.1-codex-max", Some("high") | None) => "gpt-5.1-codex-max-high",

        ("gpt-5.1", Some("medium")) => "gpt-5.1",
        ("gpt-5.1", Some("high") | None) => "gpt-5.1-high",

        ("opus-4.6", Some("standard")) => "opus-4.6",
        ("opus-4.6", Some("thinking") | None) => "opus-4.6-thinking",
        ("sonnet-4.6", Some("standard")) => "sonnet-4.6",
        ("sonnet-4.6", Some("thinking") | None) => "sonnet-4.6-thinking",
        ("opus-4.5", Some("standard")) => "opus-4.5",
        ("opus-4.5", Some("thinking") | None) => "opus-4.5-thinking",
        ("sonnet-4.5", Some("standard")) => "sonnet-4.5",
        ("sonnet-4.5", Some("thinking") | None) => "sonnet-4.5-thinking",

        _ => base_model,
    }
}

impl CursorAgent {
    pub fn base_command() -> &'static str {
        "cursor-agent"
    }

    fn resolved_model(&self) -> Option<&str> {
        self.model
            .as_deref()
            .map(|base| resolve_cursor_model_name(base, self.reasoning.as_deref()))
    }

    fn build_command_builder(
        &self,
        model: Option<&str>,
    ) -> Result<CommandBuilder, CommandBuildError> {
        // Headless runs must never block on the workspace trust prompt.
        let mut builder = CommandBuilder::new(Self::base_command()).params([
            "-p",
            "--output-format=stream-json",
            "--trust",
        ]);

        if self.force.unwrap_or(false) {
            builder = builder.extend_params(["--force"]);
        }

        if let Some(model) = model {
            builder = builder.extend_params(["--model", model]);
        }

        apply_overrides(builder, &self.cmd)
    }

    /// Returns the model to pass to the CLI and, when the requested model is
    /// not offered to this account, a notice explaining the fallback to auto.
    async fn effective_model(&self) -> (Option<String>, Option<String>) {
        let Some(requested) = self.resolved_model() else {
            return (None, None);
        };
        if requested == models::DEFAULT_MODEL_ID {
            return (Some(requested.to_string()), None);
        }
        let list = models::discover_models(Self::base_command(), &self.cmd).await;
        if models::is_known_unavailable(&list, requested) {
            let notice = format!(
                "Model `{requested}` is not available to this Cursor account, so this run uses Auto instead."
            );
            (Some(models::DEFAULT_MODEL_ID.to_string()), Some(notice))
        } else {
            (Some(requested.to_string()), None)
        }
    }

    async fn spawn_with_args(
        &self,
        current_dir: &Path,
        prompt: &str,
        resume_session: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        mcp::ensure_mcp_server_trust(self, current_dir).await;

        let (model, notice) = self.effective_model().await;
        let builder = self.build_command_builder(model.as_deref())?;
        let command_parts = match resume_session {
            Some(session_id) => {
                builder.build_follow_up(&["--resume".to_string(), session_id.to_string()])?
            }
            None => builder.build_initial()?,
        };
        let (executable_path, args) = command_parts.into_resolved().await?;

        let combined_prompt = self.append_prompt.combine_prompt(prompt);

        let mut command = Command::new(executable_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .args(&args);

        env.clone()
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn_no_window()?;

        if let Some(message) = notice {
            prepend_stdout_line(&mut child, notice_line(&message))?;
        }

        if let Some(mut stdin) = child.inner().stdin.take() {
            stdin.write_all(combined_prompt.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        Ok(child.into())
    }
}

/// Serialises a notice as a stdout event so it is persisted with the raw logs
/// and replayed whenever the transcript is rebuilt.
fn notice_line(message: &str) -> String {
    let mut line = serde_json::json!({ "type": VK_NOTICE_TYPE, "message": message }).to_string();
    line.push('\n');
    line
}

const VK_NOTICE_TYPE: &str = "vk_notice";

/// Replaces the child's stdout with a pipe that yields `line` first and then
/// everything the child writes.
fn prepend_stdout_line(child: &mut AsyncGroupChild, line: String) -> Result<(), ExecutorError> {
    let original = child.inner().stdout.take();
    let mut writer = create_stdout_pipe_writer(child)?;
    tokio::spawn(async move {
        if writer.write_all(line.as_bytes()).await.is_err() {
            return;
        }
        if let Some(mut stdout) = original {
            let _ = tokio::io::copy(&mut stdout, &mut writer).await;
        }
        let _ = writer.flush().await;
    });
    Ok(())
}

#[async_trait]
impl StandardCodingAgentExecutor for CursorAgent {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id {
            self.model = Some(model_id.clone());
        }
        if let Some(reasoning_id) = &executor_config.reasoning_id {
            self.reasoning = Some(reasoning_id.clone());
        }
        if let Some(permission_policy) = executor_config.permission_policy.clone() {
            self.force = Some(matches!(
                permission_policy,
                crate::model_selector::PermissionPolicy::Auto
            ));
        }
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_with_args(current_dir, prompt, None, env).await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_with_args(current_dir, prompt, Some(session_id), env)
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let entry_index_provider = EntryIndexProvider::start_from(&msg_store);

        // Custom stderr processor for Cursor that detects login errors
        let msg_store_stderr = msg_store.clone();
        let entry_index_provider_stderr = entry_index_provider.clone();
        let h1 = tokio::spawn(async move {
            let mut stderr = msg_store_stderr.stderr_chunked_stream();
            let mut processor = PlainTextLogProcessor::builder()
                .normalized_entry_producer(Box::new(|content: String| {
                    let content = strip_ansi_escapes::strip_str(&content);

                    NormalizedEntry {
                        timestamp: None,
                        entry_type: NormalizedEntryType::ErrorMessage {
                            error_type: NormalizedEntryError::Other,
                        },
                        content,
                        metadata: None,
                    }
                }))
                .time_gap(Duration::from_secs(2))
                .index_provider(entry_index_provider_stderr.clone())
                .build();

            while let Some(Ok(chunk)) = stderr.next().await {
                let content = strip_ansi_escapes::strip_str(&chunk);
                if let Some(message) = describe_model_rejection(&content) {
                    let error_message = NormalizedEntry {
                        timestamp: None,
                        entry_type: NormalizedEntryType::ErrorMessage {
                            error_type: NormalizedEntryError::Other,
                        },
                        content: message,
                        metadata: None,
                    };
                    let id = entry_index_provider_stderr.next();
                    msg_store_stderr
                        .push_patch(ConversationPatch::add_normalized_entry(id, error_message));
                } else if content.contains(CURSOR_AUTH_REQUIRED_MSG) {
                    let error_message = NormalizedEntry {
                        timestamp: None,
                        entry_type: NormalizedEntryType::ErrorMessage {
                            error_type: NormalizedEntryError::SetupRequired,
                        },
                        content: content.to_string(),
                        metadata: None,
                    };
                    let id = entry_index_provider_stderr.next();
                    msg_store_stderr
                        .push_patch(ConversationPatch::add_normalized_entry(id, error_message));
                } else {
                    // Always emit error message
                    for patch in processor.process(chunk) {
                        msg_store_stderr.push_patch(patch);
                    }
                }
            }
        });

        // Process Cursor stdout JSONL with typed serde models
        let current_dir = worktree_path.to_path_buf();
        let h2 = tokio::spawn(async move {
            let mut lines = msg_store.stdout_lines_stream();

            // Assistant streaming coalescer state
            let mut model_reported = false;
            let mut session_id_reported = false;

            let mut current_assistant_message_buffer = String::new();
            let mut current_assistant_message_index: Option<usize> = None;
            let mut current_thinking_message_buffer = String::new();
            let mut current_thinking_message_index: Option<usize> = None;

            let worktree_str = current_dir.to_string_lossy().to_string();

            use std::collections::HashMap;
            // Track tool call_id -> entry index
            let mut call_index_map: HashMap<String, usize> = HashMap::new();

            while let Some(Ok(line)) = lines.next().await {
                // Parse line as CursorJson
                let cursor_json: CursorJson = match serde_json::from_str(&line) {
                    Ok(cursor_json) => cursor_json,
                    Err(_) => {
                        // Handle non-JSON output as raw system message
                        if !line.is_empty() {
                            let entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::SystemMessage,
                                content: line.to_string(),
                                metadata: None,
                            };

                            let patch_id = entry_index_provider.next();
                            let patch = ConversationPatch::add_normalized_entry(patch_id, entry);
                            msg_store.push_patch(patch);
                        }
                        continue;
                    }
                };

                // Push session_id if present
                if !session_id_reported && let Some(session_id) = cursor_json.extract_session_id() {
                    msg_store.push_session_id(session_id);
                    session_id_reported = true;
                }

                // Unrecognised events (e.g. approval handshakes) must not split a
                // streaming assistant or thinking message.
                let is_passive = matches!(cursor_json, CursorJson::Unknown);
                let is_assistant_message =
                    is_passive || matches!(cursor_json, CursorJson::Assistant { .. });
                let is_thinking_message =
                    is_passive || matches!(cursor_json, CursorJson::Thinking { .. });
                if !is_assistant_message && current_assistant_message_index.is_some() {
                    // flush
                    current_assistant_message_index = None;
                    current_assistant_message_buffer.clear();
                }
                if !is_thinking_message && current_thinking_message_index.is_some() {
                    current_thinking_message_index = None;
                    current_thinking_message_buffer.clear();
                }

                match &cursor_json {
                    CursorJson::System { model, .. } => {
                        if !model_reported && let Some(model) = model.as_ref() {
                            let entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::SystemMessage,
                                content: format!("System initialized with model: {model}"),
                                metadata: None,
                            };
                            let id = entry_index_provider.next();
                            msg_store
                                .push_patch(ConversationPatch::add_normalized_entry(id, entry));
                            model_reported = true;
                        }
                    }

                    CursorJson::User { .. } => {}

                    CursorJson::Assistant { message, .. } => {
                        if let Some(chunk) = message.concat_text() {
                            current_assistant_message_buffer.push_str(&chunk);
                            let replace_entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::AssistantMessage,
                                content: current_assistant_message_buffer.clone(),
                                metadata: None,
                            };
                            if let Some(id) = current_assistant_message_index {
                                msg_store.push_patch(ConversationPatch::replace(id, replace_entry))
                            } else {
                                let id = entry_index_provider.next();
                                current_assistant_message_index = Some(id);
                                msg_store.push_patch(ConversationPatch::add_normalized_entry(
                                    id,
                                    replace_entry,
                                ));
                            };
                        }
                    }
                    CursorJson::Thinking { text, .. } => {
                        if let Some(chunk) = text
                            && !chunk.is_empty()
                        {
                            current_thinking_message_buffer.push_str(chunk);
                            let entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::Thinking,
                                content: current_thinking_message_buffer.clone(),
                                metadata: None,
                            };
                            if let Some(id) = current_thinking_message_index {
                                msg_store.push_patch(ConversationPatch::replace(id, entry));
                            } else {
                                let id = entry_index_provider.next();
                                current_thinking_message_index = Some(id);
                                msg_store
                                    .push_patch(ConversationPatch::add_normalized_entry(id, entry));
                            }
                        }
                    }

                    CursorJson::ToolCall {
                        subtype,
                        call_id,
                        tool_call,
                        ..
                    } => {
                        let subtype = subtype.as_deref().unwrap_or_default();
                        if subtype.eq_ignore_ascii_case("started") {
                            let (action_type, content) =
                                tool_call.to_action_and_content(&worktree_str);
                            let entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::ToolUse {
                                    tool_name: tool_call.display_name(),
                                    action_type,
                                    status: ToolStatus::Created,
                                },
                                content,
                                metadata: None,
                            };
                            let id = entry_index_provider.next();
                            if let Some(cid) = call_id.as_ref() {
                                call_index_map.insert(cid.clone(), id);
                            }
                            msg_store
                                .push_patch(ConversationPatch::add_normalized_entry(id, entry));
                        } else if subtype.eq_ignore_ascii_case("completed") {
                            let (action_type, content) =
                                tool_call.to_completed_action_and_content(&worktree_str);
                            let status = if tool_call.is_failure() {
                                ToolStatus::Failed
                            } else {
                                ToolStatus::Success
                            };
                            let entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::ToolUse {
                                    tool_name: tool_call.display_name(),
                                    action_type,
                                    status,
                                },
                                content,
                                metadata: None,
                            };
                            match call_id.as_ref().and_then(|cid| call_index_map.get(cid)) {
                                Some(&idx) => {
                                    msg_store.push_patch(ConversationPatch::replace(idx, entry))
                                }
                                None => {
                                    let id = entry_index_provider.next();
                                    msg_store.push_patch(ConversationPatch::add_normalized_entry(
                                        id, entry,
                                    ));
                                }
                            }
                        }
                    }

                    CursorJson::Result {
                        is_error, result, ..
                    } => {
                        if *is_error == Some(true) {
                            let content = result
                                .as_ref()
                                .and_then(Value::as_str)
                                .filter(|s| !s.trim().is_empty())
                                .unwrap_or("Cursor reported an error without details.")
                                .to_string();
                            let entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::ErrorMessage {
                                    error_type: NormalizedEntryError::Other,
                                },
                                content,
                                metadata: None,
                            };
                            let id = entry_index_provider.next();
                            msg_store
                                .push_patch(ConversationPatch::add_normalized_entry(id, entry));
                        }
                    }

                    CursorJson::Notice { message } => {
                        let entry = NormalizedEntry {
                            timestamp: None,
                            entry_type: NormalizedEntryType::SystemMessage,
                            content: message.clone(),
                            metadata: None,
                        };
                        let id = entry_index_provider.next();
                        msg_store.push_patch(ConversationPatch::add_normalized_entry(id, entry));
                    }

                    CursorJson::Unknown => {
                        // Protocol chatter (e.g. `interaction_query` approvals) is not
                        // transcript content; only surface events that carry an error.
                        let Some(message) = unknown_event_error_message(&line) else {
                            tracing::debug!("Ignoring unrecognised Cursor event");
                            continue;
                        };
                        let entry = NormalizedEntry {
                            timestamp: None,
                            entry_type: NormalizedEntryType::ErrorMessage {
                                error_type: NormalizedEntryError::Other,
                            },
                            content: message,
                            metadata: None,
                        };
                        let id = entry_index_provider.next();
                        msg_store.push_patch(ConversationPatch::add_normalized_entry(id, entry));
                    }
                }
            }
        });

        vec![h1, h2]
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        dirs::home_dir().map(|home| home.join(".cursor").join("mcp.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        let binary_found = resolve_executable_path_blocking(Self::base_command()).is_some();
        if !binary_found {
            return AvailabilityInfo::NotFound;
        }

        let api_key_set = std::env::var_os("CURSOR_API_KEY").is_some_and(|v| !v.is_empty())
            || self
                .cmd
                .env
                .as_ref()
                .is_some_and(|env| env.get("CURSOR_API_KEY").is_some_and(|v| !v.is_empty()));

        if api_key_set {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default();
            AvailabilityInfo::LoginDetected {
                last_auth_timestamp: now,
            }
        } else {
            AvailabilityInfo::InstallationFound
        }
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::CursorAgent,
            variant: None,
            model_id: self.model.clone(),
            agent_id: None,
            reasoning_id: self.reasoning.clone(),
            permission_policy: Some(crate::model_selector::PermissionPolicy::Auto),
        }
    }

    async fn discover_options(
        &self,
        _workdir: Option<&std::path::Path>,
        _repo_path: Option<&std::path::Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        fn to_options(list: models::CursorModelList, loading: bool) -> ExecutorDiscoveredOptions {
            ExecutorDiscoveredOptions {
                model_selector: ModelSelectorConfig {
                    models: list.models,
                    default_model: list.default_model,
                    ..Default::default()
                },
                loading_models: loading,
                error: list.error,
                ..Default::default()
            }
        }

        if let Some(cached) = models::cached_models(Self::base_command(), &self.cmd) {
            return Ok(Box::pin(futures::stream::once(async move {
                patch::executor_discovered_options(to_options(cached, false))
            })));
        }

        let initial =
            patch::executor_discovered_options(to_options(models::fallback_models(), true));
        let cmd = self.cmd.clone();
        let discovered = async move {
            let list = models::discover_models(Self::base_command(), &cmd).await;
            patch::executor_discovered_options(to_options(list, false))
        };
        Ok(Box::pin(
            futures::stream::once(async move { initial }).chain(futures::stream::once(discovered)),
        ))
    }
}
/* ===========================
Typed Cursor JSON structures
=========================== */

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum CursorJson {
    #[serde(rename = "system")]
    System {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default, rename = "apiKeySource")]
        api_key_source: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default, rename = "permissionMode")]
        permission_mode: Option<String>,
    },
    #[serde(rename = "user")]
    User {
        message: CursorMessage,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        message: CursorMessage,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(rename = "tool_call")]
    ToolCall {
        #[serde(default)]
        subtype: Option<String>, // "started" | "completed"
        #[serde(default)]
        call_id: Option<String>,
        #[serde(deserialize_with = "deserialize_tool_call")]
        tool_call: CursorToolCall,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(rename = "result")]
    Result {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        is_error: Option<bool>,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Written by this executor, not the CLI (see `notice_line`).
    #[serde(rename = "vk_notice")]
    Notice { message: String },
    #[serde(other)]
    Unknown,
}

impl CursorJson {
    pub fn extract_session_id(&self) -> Option<String> {
        match self {
            CursorJson::System { .. } => None, // session might not have been initialized yet
            CursorJson::User { session_id, .. } => session_id.clone(),
            CursorJson::Assistant { session_id, .. } => session_id.clone(),
            CursorJson::Thinking { session_id, .. } => session_id.clone(),
            CursorJson::ToolCall { session_id, .. } => session_id.clone(),
            CursorJson::Result { session_id, .. } => session_id.clone(),
            CursorJson::Notice { .. } | CursorJson::Unknown => None,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMessage {
    pub role: String,
    pub content: Vec<CursorContentItem>,
}

impl CursorMessage {
    pub fn concat_text(&self) -> Option<String> {
        let mut out = String::new();
        for CursorContentItem::Text { text } in &self.content {
            out.push_str(text);
        }
        if out.is_empty() { None } else { Some(out) }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum CursorContentItem {
    #[serde(rename = "text")]
    Text { text: String },
}

/* ===========================
Tool call structure
=========================== */

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub enum CursorToolCall {
    #[serde(rename = "shellToolCall")]
    Shell {
        args: CursorShellArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "lsToolCall")]
    LS {
        args: CursorLsArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "globToolCall")]
    Glob {
        args: CursorGlobArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "grepToolCall")]
    Grep {
        args: CursorGrepArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "semSearchToolCall")]
    SemSearch {
        args: CursorSemSearchArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "writeToolCall")]
    Write {
        args: CursorWriteArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "readToolCall")]
    Read {
        args: CursorReadArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "editToolCall")]
    Edit {
        args: CursorEditArgs,
        #[serde(default)]
        result: Option<CursorEditResult>,
    },
    #[serde(rename = "deleteToolCall")]
    Delete {
        args: CursorDeleteArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "updateTodosToolCall")]
    Todo {
        args: CursorUpdateTodosArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "mcpToolCall")]
    Mcp {
        args: CursorMcpArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "webSearchToolCall")]
    WebSearch {
        args: CursorWebSearchArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "webFetchToolCall")]
    WebFetch {
        args: CursorWebFetchArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "taskToolCall")]
    Task {
        args: CursorTaskArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    /// The agent looking up tool schemas before calling a tool.
    #[serde(rename = "getMcpToolsToolCall")]
    GetMcpTools {
        args: CursorGetMcpToolsArgs,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    /// Any tool kind this normalizer does not model, or a known kind whose
    /// arguments no longer match. Built by [`CursorToolCall::from_value`].
    #[serde(skip_deserializing)]
    Unknown {
        name: String,
        args: Option<serde_json::Value>,
        result: Option<serde_json::Value>,
    },
}

/// Envelope keys the CLI adds next to the `<kind>ToolCall` payload.
const TOOL_CALL_ENVELOPE_KEYS: &[&str] = &[
    "toolCallId",
    "startedAtMs",
    "completedAtMs",
    "hookAdditionalContexts",
];

/// Result keys the CLI uses for unsuccessful tool calls.
const FAILURE_RESULT_KEYS: &[&str] = &["error", "failure", "rejected", "permissionDenied"];

/// Argument keys that are bookkeeping rather than something a reader needs.
const NOISY_ARG_KEYS: &[&str] = &[
    "toolCallId",
    "conversationId",
    "requestId",
    "agentId",
    "timeout",
    "hardTimeout",
];

fn deserialize_tool_call<'de, D>(deserializer: D) -> Result<CursorToolCall, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(CursorToolCall::from_value(Value::deserialize(
        deserializer,
    )?))
}

impl CursorToolCall {
    /// Parses a `tool_call` object such as
    /// `{"readToolCall": {...}, "toolCallId": "...", "startedAtMs": "..."}`.
    /// Never fails: unrecognised payloads become [`CursorToolCall::Unknown`].
    pub fn from_value(value: Value) -> Self {
        let Value::Object(map) = value else {
            return Self::unknown("unknown", None);
        };
        let kind = map
            .keys()
            .find(|k| k.ends_with("ToolCall"))
            .or_else(|| {
                map.keys()
                    .find(|k| !TOOL_CALL_ENVELOPE_KEYS.contains(&k.as_str()))
            })
            .cloned();
        let Some(kind) = kind else {
            return Self::unknown("unknown", None);
        };
        let payload = map.get(&kind).cloned().unwrap_or(Value::Null);
        let tagged = serde_json::json!({ kind.clone(): payload.clone() });
        serde_json::from_value(tagged).unwrap_or_else(|_| {
            let name = kind.strip_suffix("ToolCall").unwrap_or(&kind);
            Self::unknown(name, Some(payload))
        })
    }

    fn unknown(name: &str, payload: Option<Value>) -> Self {
        let field = |key: &str| payload.as_ref().and_then(|p| p.get(key)).cloned();
        CursorToolCall::Unknown {
            name: name.to_string(),
            args: field("args"),
            result: field("result"),
        }
    }

    pub fn get_name(&self) -> &str {
        match self {
            CursorToolCall::Shell { .. } => "shell",
            CursorToolCall::LS { .. } => "ls",
            CursorToolCall::Glob { .. } => "glob",
            CursorToolCall::Grep { .. } => "grep",
            CursorToolCall::SemSearch { .. } => "semsearch",
            CursorToolCall::Write { .. } => "write",
            CursorToolCall::Read { .. } => "read",
            CursorToolCall::Edit { .. } => "edit",
            CursorToolCall::Delete { .. } => "delete",
            CursorToolCall::Todo { .. } => "todo",
            CursorToolCall::Mcp { .. } => "mcp",
            CursorToolCall::WebSearch { .. } => "web_search",
            CursorToolCall::WebFetch { .. } => "web_fetch",
            CursorToolCall::Task { .. } => "task",
            CursorToolCall::GetMcpTools { .. } => "tool_lookup",
            CursorToolCall::Unknown { name, .. } => name,
        }
    }

    /// Tool name shown in the transcript.
    pub fn display_name(&self) -> String {
        match self {
            CursorToolCall::Mcp { args, .. } => mcp_label(args),
            _ => self.get_name().to_string(),
        }
    }

    fn result_value(&self) -> Option<&Value> {
        match self {
            CursorToolCall::Shell { result, .. }
            | CursorToolCall::LS { result, .. }
            | CursorToolCall::Glob { result, .. }
            | CursorToolCall::Grep { result, .. }
            | CursorToolCall::SemSearch { result, .. }
            | CursorToolCall::Write { result, .. }
            | CursorToolCall::Read { result, .. }
            | CursorToolCall::Delete { result, .. }
            | CursorToolCall::Todo { result, .. }
            | CursorToolCall::Mcp { result, .. }
            | CursorToolCall::WebSearch { result, .. }
            | CursorToolCall::WebFetch { result, .. }
            | CursorToolCall::Task { result, .. }
            | CursorToolCall::GetMcpTools { result, .. }
            | CursorToolCall::Unknown { result, .. } => result.as_ref(),
            CursorToolCall::Edit { .. } => None,
        }
    }

    /// Whether a completed tool call reported failure.
    pub fn is_failure(&self) -> bool {
        let is_failure_key = |key: &String| FAILURE_RESULT_KEYS.contains(&key.as_str());
        if let CursorToolCall::Edit {
            result: Some(CursorEditResult::Unknown { data }),
            ..
        } = self
        {
            return data.keys().any(is_failure_key);
        }
        match self.result_value() {
            Some(Value::Object(map)) => map.keys().any(is_failure_key),
            _ => false,
        }
    }

    /// Like [`Self::to_action_and_content`], enriched with the tool's result.
    pub fn to_completed_action_and_content(&self, worktree_path: &str) -> (ActionType, String) {
        let (action, content) = self.to_action_and_content(worktree_path);
        let action = match self {
            CursorToolCall::Shell { args, result } => {
                shell_action_with_result(args, result.as_ref())
            }
            CursorToolCall::Mcp { args, result } => ActionType::Tool {
                tool_name: mcp_label(args),
                arguments: Some(serde_json::json!({
                    "name": args.name,
                    "args": args.args,
                    "providerIdentifier": args.provider_identifier,
                    "toolName": args.tool_name,
                })),
                result: result
                    .as_ref()
                    .and_then(|res| serde_json::from_value::<CursorMcpResult>(res.clone()).ok())
                    .and_then(CursorMcpResult::into_markdown)
                    .map(crate::logs::ToolResult::markdown),
            },
            CursorToolCall::Task { args, result } => ActionType::TaskCreate {
                description: args.description.clone(),
                subagent_type: args.subagent_label(),
                result: result
                    .as_ref()
                    .and_then(task_result_markdown)
                    .map(crate::logs::ToolResult::markdown),
            },
            _ => action,
        };
        (action, content)
    }

    pub fn to_action_and_content(&self, worktree_path: &str) -> (ActionType, String) {
        match self {
            CursorToolCall::Read { args, .. } => {
                let path = make_path_relative(&args.path, worktree_path);
                (ActionType::FileRead { path: path.clone() }, path)
            }
            CursorToolCall::Write { args, .. } => {
                let path = make_path_relative(&args.path, worktree_path);
                let changes = args
                    .contents
                    .iter()
                    .map(|content| FileChange::Write {
                        content: content.clone(),
                    })
                    .collect();
                (
                    ActionType::FileEdit {
                        path: path.clone(),
                        changes,
                    },
                    path,
                )
            }
            CursorToolCall::Edit { args, result, .. } => {
                let path = make_path_relative(&args.path, worktree_path);
                let mut changes = vec![];

                if let Some(apply_patch) = &args.apply_patch {
                    changes.push(FileChange::Edit {
                        unified_diff: normalize_unified_diff(&path, &apply_patch.patch_content),
                        has_line_numbers: false,
                    });
                }

                if let Some(str_replace) = &args.str_replace {
                    changes.push(FileChange::Edit {
                        unified_diff: create_unified_diff(
                            &path,
                            &str_replace.old_text,
                            &str_replace.new_text,
                        ),
                        has_line_numbers: false,
                    });
                }

                if let Some(multi_str_replace) = &args.multi_str_replace {
                    let edits: Vec<FileChange> = multi_str_replace
                        .edits
                        .iter()
                        .map(|edit| FileChange::Edit {
                            unified_diff: create_unified_diff(
                                &path,
                                &edit.old_text,
                                &edit.new_text,
                            ),
                            has_line_numbers: false,
                        })
                        .collect();
                    changes.extend(edits);
                }

                if changes.is_empty()
                    && let Some(CursorEditResult::Success(CursorEditSuccessResult {
                        diff_string: Some(diff_string),
                        ..
                    })) = &result
                {
                    changes.push(FileChange::Edit {
                        unified_diff: normalize_unified_diff(&path, diff_string),
                        has_line_numbers: false,
                    });
                }

                (
                    ActionType::FileEdit {
                        path: path.clone(),
                        changes,
                    },
                    path,
                )
            }
            CursorToolCall::Delete { args, .. } => {
                let path = make_path_relative(&args.path, worktree_path);
                (
                    ActionType::FileEdit {
                        path: path.clone(),
                        changes: vec![FileChange::Delete],
                    },
                    path.to_string(),
                )
            }
            CursorToolCall::Shell { args, .. } => {
                let cmd = &args.command;
                (
                    ActionType::CommandRun {
                        command: cmd.clone(),
                        result: None,
                        category: CommandCategory::from_command(cmd),
                    },
                    cmd.to_string(),
                )
            }
            CursorToolCall::Grep { args, .. } => {
                let pattern = &args.pattern;
                (
                    ActionType::Search {
                        query: pattern.clone(),
                    },
                    pattern.to_string(),
                )
            }
            CursorToolCall::SemSearch { args, .. } => {
                let query = &args.query;
                (
                    ActionType::Search {
                        query: query.clone(),
                    },
                    query.to_string(),
                )
            }
            CursorToolCall::Glob { args, .. } => {
                let pattern = args.glob_pattern.clone().unwrap_or_else(|| "*".to_string());
                if let Some(path) = args.path.as_ref().or(args.target_directory.as_ref()) {
                    let path = make_path_relative(path, worktree_path);
                    (
                        ActionType::Search {
                            query: pattern.clone(),
                        },
                        format!("Find files: `{pattern}` in {path}"),
                    )
                } else {
                    (
                        ActionType::Search {
                            query: pattern.clone(),
                        },
                        format!("Find files: `{pattern}`"),
                    )
                }
            }
            CursorToolCall::LS { args, .. } => {
                let path = make_path_relative(&args.path, worktree_path);
                let content = if path.is_empty() {
                    "List directory".to_string()
                } else {
                    format!("List directory: {path}")
                };
                (
                    ActionType::Other {
                        description: content.clone(),
                    },
                    content,
                )
            }
            CursorToolCall::Todo { args, .. } => {
                let todos = args
                    .todos
                    .as_ref()
                    .map(|todos| {
                        todos
                            .iter()
                            .map(|t| TodoItem {
                                content: t.content.clone(),
                                status: normalize_todo_status(&t.status),
                                priority: None, // CursorTodoItem doesn't have priority field
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                (
                    ActionType::TodoManagement {
                        todos,
                        operation: "write".to_string(),
                    },
                    "TODO list updated".to_string(),
                )
            }
            CursorToolCall::Mcp { args, .. } => {
                let tool_name = args.tool_name.as_deref().unwrap_or(&args.name);
                let label = mcp_label(args);
                let summary = tool_name.to_string();
                let mut arguments = serde_json::json!({
                    "name": args.name,
                    "args": args.args,
                });
                if let Some(p) = &args.provider_identifier {
                    arguments["providerIdentifier"] = serde_json::Value::String(p.clone());
                }
                if let Some(tn) = &args.tool_name {
                    arguments["toolName"] = serde_json::Value::String(tn.clone());
                }
                (
                    ActionType::Tool {
                        tool_name: label,
                        arguments: Some(arguments),
                        result: None,
                    },
                    summary,
                )
            }
            CursorToolCall::WebSearch { args, .. } => (
                ActionType::Search {
                    query: args.search_term.clone(),
                },
                format!("Web search: {}", args.search_term),
            ),
            CursorToolCall::WebFetch { args, .. } => (
                ActionType::WebFetch {
                    url: args.url.clone(),
                },
                args.url.clone(),
            ),
            CursorToolCall::Task { args, .. } => (
                ActionType::TaskCreate {
                    description: args.description.clone(),
                    subagent_type: args.subagent_label(),
                    result: None,
                },
                args.description.clone(),
            ),
            CursorToolCall::GetMcpTools { args, .. } => {
                let description = match (&args.tool_name, &args.server) {
                    (Some(tool), _) => format!("Look up tool: {tool}"),
                    (None, Some(server)) => format!("List tools: {server}"),
                    (None, None) => "List tools".to_string(),
                };
                (
                    ActionType::Other {
                        description: description.clone(),
                    },
                    description,
                )
            }
            CursorToolCall::Unknown { name, args, .. } => {
                let label = humanize_tool_name(name);
                let description = match summarize_args(args.as_ref(), worktree_path) {
                    Some(summary) => format!("{label}: {summary}"),
                    None => label,
                };
                (
                    ActionType::Other {
                        description: description.clone(),
                    },
                    description,
                )
            }
        }
    }
}

fn mcp_label(args: &CursorMcpArgs) -> String {
    let provider = args.provider_identifier.as_deref().unwrap_or("mcp");
    let tool_name = args.tool_name.as_deref().unwrap_or(&args.name);
    format!("mcp:{provider}:{tool_name}")
}

fn shell_action_with_result(args: &CursorShellArgs, result: Option<&Value>) -> ActionType {
    let outcome = result
        .and_then(|res| serde_json::from_value::<CursorShellResult>(res.clone()).ok())
        .and_then(CursorShellResult::into_outcome);
    let (stdout, stderr, exit_code) = match outcome {
        Some(out) => (out.stdout, out.stderr, out.exit_code),
        None => (None, None, None),
    };
    let non_empty = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    let output = match (non_empty(stdout), non_empty(stderr)) {
        (Some(out), Some(err)) => Some(format!(
            "STDOUT:\n{}\n\nSTDERR:\n{}",
            out.trim(),
            err.trim()
        )),
        (Some(out), None) => Some(out),
        (None, Some(err)) => Some(err),
        (None, None) => None,
    };
    ActionType::CommandRun {
        command: args.command.clone(),
        result: Some(crate::logs::CommandRunResult {
            exit_status: exit_code.map(|code| crate::logs::CommandExitStatus::ExitCode { code }),
            output,
        }),
        category: CommandCategory::from_command(&args.command),
    }
}

/// Collects the subagent's final messages from a `taskToolCall` result.
fn task_result_markdown(result: &Value) -> Option<String> {
    let outcome = result.get("success").or_else(|| result.get("failure"))?;
    let texts: Vec<&str> = outcome
        .get("conversationSteps")?
        .as_array()?
        .iter()
        .filter_map(|step| step.pointer("/assistantMessage/text")?.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect();
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n\n"))
    }
}

/// `readLintsToolCall` -> "Read lints".
fn humanize_tool_name(name: &str) -> String {
    let mut words = String::new();
    for (i, c) in name.chars().enumerate() {
        if c == '_' || c == '-' {
            words.push(' ');
        } else if c.is_uppercase() {
            if i > 0 {
                words.push(' ');
            }
            words.extend(c.to_lowercase());
        } else {
            words.push(c);
        }
    }
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Tool".to_string(),
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn preview_arg(value: &Value, worktree_path: &str) -> Option<String> {
    match value {
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) if s.starts_with('/') => Some(make_path_relative(s, worktree_path)),
        Value::String(s) => Some(s.lines().next().unwrap_or_default().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .filter_map(|item| match item {
                    Value::String(_) | Value::Number(_) | Value::Bool(_) => {
                        preview_arg(item, worktree_path)
                    }
                    _ => None,
                })
                .collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        _ => None,
    }
}

/// A short `key: value` summary of scalar arguments, skipping ids and timing.
fn summarize_args(args: Option<&Value>, worktree_path: &str) -> Option<String> {
    let Value::Object(map) = args? else {
        return None;
    };
    let parts: Vec<String> = map
        .iter()
        .filter(|(key, _)| {
            !NOISY_ARG_KEYS.contains(&key.as_str()) && !key.ends_with("Ms") && !key.ends_with("Id")
        })
        .filter_map(|(key, value)| {
            preview_arg(value, worktree_path)
                .map(|preview| format!("{key}: {}", truncate_chars(&preview, 80)))
        })
        .take(3)
        .collect();
    (!parts.is_empty()).then(|| truncate_chars(&parts.join(", "), 160))
}

/// Rewrites the CLI's `Cannot use this model: <id>. Available models: ...`
/// rejection into an actionable message, keeping the original text.
fn describe_model_rejection(stderr: &str) -> Option<String> {
    const MARKER: &str = "Cannot use this model:";
    let start = stderr.find(MARKER)?;
    let original = stderr[start..].trim();
    let rest = original[MARKER.len()..].trim_start();
    let model = rest
        .split(". ")
        .next()
        .unwrap_or(rest)
        .trim()
        .trim_end_matches('.');
    Some(format!(
        "Cursor rejected model `{model}` for this account. Pick another model (Auto always works) and send again.\n\nCursor CLI: {original}"
    ))
}

/// Extracts a message from an unrecognised event whose type mentions an error.
fn unknown_event_error_message(line: &str) -> Option<String> {
    let event: Value = serde_json::from_str(line).ok()?;
    let event_type = event.get("type")?.as_str()?;
    if !event_type.contains("error") {
        return None;
    }
    event
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| event.pointer("/error/message").and_then(Value::as_str))
        .or_else(|| event.get("error").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| Some(format!("Cursor reported `{event_type}`")))
}

fn normalize_todo_status(status: &str) -> String {
    match status.to_lowercase().as_str() {
        "todo_status_pending" => "pending".to_string(),
        "todo_status_in_progress" => "in_progress".to_string(),
        "todo_status_completed" => "completed".to_string(),
        "todo_status_cancelled" => "cancelled".to_string(),
        other => other.to_string(),
    }
}

/* ===========================
Typed tool results for Cursor
=========================== */

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorShellOutcome {
    #[serde(default)]
    pub stdout: Option<String>,
    #[serde(default)]
    pub stderr: Option<String>,
    #[serde(default, rename = "exitCode")]
    pub exit_code: Option<i32>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorShellWrappedResult {
    #[serde(default)]
    pub success: Option<CursorShellOutcome>,
    #[serde(default)]
    pub failure: Option<CursorShellOutcome>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum CursorShellResult {
    Wrapped(CursorShellWrappedResult),
    Flat(CursorShellOutcome),
    Unknown(serde_json::Value),
}

impl CursorShellResult {
    pub fn into_outcome(self) -> Option<CursorShellOutcome> {
        match self {
            CursorShellResult::Flat(o) => Some(o),
            CursorShellResult::Wrapped(w) => w.success.or(w.failure),
            CursorShellResult::Unknown(_) => None,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMcpTextInner {
    pub text: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMcpContentItem {
    #[serde(default)]
    pub text: Option<CursorMcpTextInner>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMcpOutcome {
    #[serde(default)]
    pub content: Option<Vec<CursorMcpContentItem>>,
    #[serde(default, rename = "isError")]
    pub is_error: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMcpWrappedResult {
    #[serde(default)]
    pub success: Option<CursorMcpOutcome>,
    #[serde(default)]
    pub failure: Option<CursorMcpOutcome>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum CursorMcpResult {
    Wrapped(CursorMcpWrappedResult),
    Flat(CursorMcpOutcome),
    Unknown(serde_json::Value),
}

impl CursorMcpResult {
    pub fn into_markdown(self) -> Option<String> {
        let outcome = match self {
            CursorMcpResult::Flat(o) => Some(o),
            CursorMcpResult::Wrapped(w) => w.success.or(w.failure),
            CursorMcpResult::Unknown(_) => None,
        }?;

        let items = outcome.content.unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        for item in items {
            if let Some(t) = item.text {
                parts.push(t.text);
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorShellArgs {
    pub command: String,
    #[serde(default, alias = "working_directory", alias = "workingDirectory")]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorLsArgs {
    pub path: String,
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorGlobArgs {
    #[serde(default, alias = "globPattern", alias = "glob_pattern")]
    pub glob_pattern: Option<String>,
    #[serde(default, alias = "targetDirectory")]
    pub path: Option<String>,
    #[serde(default, alias = "target_directory")]
    pub target_directory: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorGrepArgs {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default, alias = "glob")]
    pub glob_filter: Option<String>,
    #[serde(default, alias = "outputMode", alias = "output_mode")]
    pub output_mode: Option<String>,
    #[serde(default, alias = "-i", alias = "caseInsensitive")]
    pub case_insensitive: Option<bool>,
    #[serde(default)]
    pub multiline: Option<bool>,
    #[serde(default, alias = "headLimit", alias = "head_limit")]
    pub head_limit: Option<u64>,
    #[serde(default)]
    pub r#type: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorSemSearchArgs {
    pub query: String,
    #[serde(default, alias = "targetDirectories")]
    pub target_directories: Option<Vec<String>>,
    #[serde(default)]
    pub explanation: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorWriteArgs {
    pub path: String,
    #[serde(
        default,
        alias = "fileText",
        alias = "file_text",
        alias = "contents",
        alias = "content"
    )]
    pub contents: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorReadArgs {
    pub path: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorEditArgs {
    pub path: String,
    #[serde(default, rename = "applyPatch")]
    pub apply_patch: Option<CursorApplyPatch>,
    #[serde(default, rename = "strReplace")]
    pub str_replace: Option<CursorStrReplace>,
    #[serde(default, rename = "multiStrReplace")]
    pub multi_str_replace: Option<CursorMultiStrReplace>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CursorEditResult {
    Success(CursorEditSuccessResult),
    #[serde(untagged)]
    Unknown {
        #[serde(flatten)]
        data: HashMap<String, serde_json::Value>,
    },
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorEditSuccessResult {
    pub path: String,
    #[serde(default, rename = "resultForModel")]
    pub result_for_model: Option<String>,
    #[serde(default, rename = "linesAdded")]
    pub lines_added: Option<u64>,
    #[serde(default, rename = "linesRemoved")]
    pub lines_removed: Option<u64>,
    #[serde(default, rename = "diffString")]
    pub diff_string: Option<String>,
    #[serde(default, rename = "afterFullFileContent")]
    pub after_full_file_content: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorApplyPatch {
    #[serde(rename = "patchContent")]
    pub patch_content: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorStrReplace {
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
    #[serde(default, rename = "replaceAll")]
    pub replace_all: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMultiStrReplace {
    pub edits: Vec<CursorMultiEditItem>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMultiEditItem {
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
    #[serde(default, rename = "replaceAll")]
    pub replace_all: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorDeleteArgs {
    pub path: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorUpdateTodosArgs {
    #[serde(default)]
    pub todos: Option<Vec<CursorTodoItem>>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorMcpArgs {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(default, alias = "providerIdentifier")]
    pub provider_identifier: Option<String>,
    #[serde(default, alias = "toolName")]
    pub tool_name: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorWebSearchArgs {
    #[serde(alias = "searchTerm", alias = "query")]
    pub search_term: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorWebFetchArgs {
    pub url: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorTaskArgs {
    pub description: String,
    #[serde(default)]
    pub prompt: Option<String>,
    /// e.g. `{"explore": {}}`; `{"unspecified": {}}` means the default agent.
    #[serde(default, alias = "subagentType")]
    pub subagent_type: Option<Value>,
}

impl CursorTaskArgs {
    pub fn subagent_label(&self) -> Option<String> {
        let label = match self.subagent_type.as_ref()? {
            Value::String(s) => s.clone(),
            Value::Object(map) => map.keys().next()?.clone(),
            _ => return None,
        };
        (!label.is_empty() && label != "unspecified").then_some(label)
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorGetMcpToolsArgs {
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default, alias = "toolName")]
    pub tool_name: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct CursorTodoItem {
    #[serde(default)]
    pub id: Option<String>,
    pub content: String,
    pub status: String,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub dependencies: Option<Vec<String>>,
}

/* ===========================
Tests
=========================== */

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use workspace_utils::msg_store::MsgStore;

    use super::*;

    #[tokio::test]
    async fn test_cursor_streaming_patch_generation() {
        // Avoid relying on feature flag in tests; construct with a dummy command
        let executor = CursorAgent {
            append_prompt: AppendPrompt::default(),
            force: None,
            model: None,
            reasoning: None,
            cmd: Default::default(),
        };
        let msg_store = Arc::new(MsgStore::new());
        let current_dir = std::path::PathBuf::from("/tmp/test-worktree");

        // A minimal synthetic init + assistant micro-chunks (as Cursor would emit)
        msg_store.push_stdout(format!(
            "{}\n",
            r#"{"type":"system","subtype":"init","session_id":"sess-123","model":"OpenAI GPT-5"}"#
        ));
        msg_store.push_stdout(format!(
            "{}\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}]}}"#
        ));
        msg_store.push_stdout(format!(
            "{}\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":" world"}]}}"#
        ));
        msg_store.push_finished();

        executor.normalize_logs(msg_store.clone(), &current_dir);

        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Verify patches were emitted (system init + assistant add/replace)
        let history = msg_store.get_history();
        let patch_count = history
            .iter()
            .filter(|m| matches!(m, workspace_utils::log_msg::LogMsg::JsonPatch(_)))
            .count();
        assert!(
            patch_count >= 2,
            "Expected at least 2 patches, got {patch_count}"
        );
    }

    #[test]
    fn test_session_id_extraction_from_system_line() {
        // System messages no longer extract session_id
        let system_line = r#"{"type":"system","subtype":"init","session_id":"abc-xyz","model":"Claude 4 Sonnet"}"#;
        let parsed: CursorJson = serde_json::from_str(system_line).unwrap();
        assert_eq!(parsed.extract_session_id().as_deref(), None);
    }

    #[test]
    fn test_cursor_tool_call_parsing() {
        // Test known variant (from reference JSONL)
        let shell_tool_json = r#"{"shellToolCall":{"args":{"command":"wc -l drill.md","workingDirectory":"","timeout":0}}}"#;
        let parsed: CursorToolCall = serde_json::from_str(shell_tool_json).unwrap();

        match parsed {
            CursorToolCall::Shell { args, result } => {
                assert_eq!(args.command, "wc -l drill.md");
                assert_eq!(args.working_directory, Some("".to_string()));
                assert_eq!(args.timeout, Some(0));
                assert_eq!(result, None);
            }
            _ => panic!("Expected Shell variant"),
        }

        // Test unknown variant (captures raw data)
        let unknown_tool_json =
            r#"{"unknownTool":{"args":{"someData":"value"},"result":{"status":"success"}}}"#;
        let parsed = CursorToolCall::from_value(serde_json::from_str(unknown_tool_json).unwrap());

        match parsed {
            CursorToolCall::Unknown { name, args, result } => {
                assert_eq!(name, "unknownTool");
                assert_eq!(args.unwrap()["someData"], "value");
                assert_eq!(result.unwrap()["status"], "success");
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    /// Captured from `agent -p --output-format=stream-json` (CLI 2026.10.22);
    /// paths scrubbed to `FIXTURE_WORKTREE`.
    const FIXTURE: &str = include_str!("cursor/fixtures/stream_json_tools.jsonl");
    const FIXTURE_WORKTREE: &str = "/tmp/vk-fixture-repo";

    fn test_executor() -> CursorAgent {
        CursorAgent {
            append_prompt: AppendPrompt::default(),
            force: None,
            model: None,
            reasoning: None,
            cmd: Default::default(),
        }
    }

    /// Runs the normalizer over JSONL stdout and returns the final entries.
    async fn normalize(stdout: &str, stderr: &str) -> Vec<NormalizedEntry> {
        let msg_store = Arc::new(MsgStore::new());
        for line in stdout.lines() {
            msg_store.push_stdout(format!("{line}\n"));
        }
        if !stderr.is_empty() {
            msg_store.push(workspace_utils::log_msg::LogMsg::Stderr(stderr.to_string()));
        }
        msg_store.push_finished();

        let handles =
            test_executor().normalize_logs(msg_store.clone(), Path::new(FIXTURE_WORKTREE));
        for handle in handles {
            handle.await.unwrap();
        }

        let mut entries = std::collections::BTreeMap::new();
        for msg in msg_store.get_history() {
            let workspace_utils::log_msg::LogMsg::JsonPatch(patch) = msg else {
                continue;
            };
            for op in serde_json::to_value(&patch).unwrap().as_array().unwrap() {
                let Some(index) = op["path"]
                    .as_str()
                    .and_then(|p| p.strip_prefix("/entries/"))
                    .and_then(|i| i.parse::<usize>().ok())
                else {
                    continue;
                };
                match op["op"].as_str() {
                    Some("add" | "replace") => {
                        entries.insert(index, op["value"]["content"].clone());
                    }
                    Some("remove") => {
                        entries.remove(&index);
                    }
                    _ => {}
                }
            }
        }
        entries
            .into_values()
            .map(|v| serde_json::from_value(v).unwrap())
            .collect()
    }

    fn tool_uses(entries: &[NormalizedEntry]) -> Vec<(String, ActionType, ToolStatus, String)> {
        entries
            .iter()
            .filter_map(|e| match &e.entry_type {
                NormalizedEntryType::ToolUse {
                    tool_name,
                    action_type,
                    status,
                } => Some((
                    tool_name.clone(),
                    action_type.clone(),
                    status.clone(),
                    e.content.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_captured_tool_call_maps_to_a_known_kind() {
        let mut kinds = Vec::new();
        for line in FIXTURE.lines() {
            let parsed: CursorJson = serde_json::from_str(line).unwrap();
            if let CursorJson::ToolCall { tool_call, .. } = parsed {
                assert!(
                    !matches!(tool_call, CursorToolCall::Unknown { .. }),
                    "unmapped tool call: {line}"
                );
                kinds.push(tool_call.get_name().to_string());
            }
        }
        for expected in [
            "tool_lookup",
            "todo",
            "shell",
            "read",
            "glob",
            "grep",
            "edit",
            "delete",
            "task",
            "web_search",
            "web_fetch",
        ] {
            assert!(kinds.iter().any(|k| k == expected), "missing {expected}");
        }
    }

    #[tokio::test]
    async fn captured_transcript_has_readable_tool_entries_without_internals() {
        let entries = normalize(FIXTURE, "").await;

        for entry in &entries {
            let rendered = serde_json::to_string(entry).unwrap();
            for internal in [
                "toolCallId",
                "startedAtMs",
                "completedAtMs",
                "hookAdditionalContexts",
                "ToolCall",
                "interaction_query",
            ] {
                assert!(
                    !rendered.contains(internal),
                    "entry leaks `{internal}`: {rendered}"
                );
            }
        }

        let tools = tool_uses(&entries);
        assert!(
            tools
                .iter()
                .all(|(_, _, status, _)| !matches!(status, ToolStatus::Created)),
            "every started tool call should be completed"
        );

        let find = |pred: &dyn Fn(&ActionType) -> bool| {
            tools
                .iter()
                .find(|(_, action, _, _)| pred(action))
                .cloned()
                .unwrap_or_else(|| panic!("tool entry not found in {tools:#?}"))
        };

        find(&|a| matches!(a, ActionType::FileRead { path } if path == "README.md"));
        find(&|a| matches!(a, ActionType::Search { query } if query == "*.py"));
        find(&|a| matches!(a, ActionType::Search { query } if query == "TODO"));
        find(&|a| matches!(a, ActionType::Search { query } if query == "rust serde untagged enum"));
        find(&|a| matches!(a, ActionType::WebFetch { url } if url == "https://example.com"));
        find(&|a| matches!(a, ActionType::TodoManagement { todos, .. } if !todos.is_empty()));

        let (_, edit, _, _) =
            find(&|a| matches!(a, ActionType::FileEdit { path, .. } if path == "src/app.py"));
        let ActionType::FileEdit { changes, .. } = edit else {
            unreachable!()
        };
        assert!(matches!(
            changes.as_slice(),
            [FileChange::Edit { unified_diff, .. }] if unified_diff.contains("+    return \"hello\"")
        ));

        let (_, created, _, _) =
            find(&|a| matches!(a, ActionType::FileEdit { path, .. } if path == "out.txt"));
        let ActionType::FileEdit { changes, .. } = created else {
            unreachable!()
        };
        assert!(matches!(
            changes.as_slice(),
            [FileChange::Edit { unified_diff, .. }] if unified_diff.contains("+done")
        ));

        find(&|a| {
            matches!(a, ActionType::FileEdit { path, changes }
                if path == "notes.txt" && matches!(changes.as_slice(), [FileChange::Delete]))
        });

        let (_, listing, status, _) = find(
            &|a| matches!(a, ActionType::CommandRun { command, .. } if command == "ls -la && cat out.txt"),
        );
        assert!(matches!(status, ToolStatus::Success));
        let ActionType::CommandRun { result, .. } = listing else {
            unreachable!()
        };
        let result = result.unwrap();
        assert!(result.output.unwrap().contains("done"));
        assert!(matches!(
            result.exit_status,
            Some(crate::logs::CommandExitStatus::ExitCode { code: 0 })
        ));

        let (_, failing, status, _) =
            find(&|a| matches!(a, ActionType::CommandRun { command, .. } if command == "exit 3"));
        assert!(matches!(status, ToolStatus::Failed));
        let ActionType::CommandRun { result, .. } = failing else {
            unreachable!()
        };
        assert!(matches!(
            result.unwrap().exit_status,
            Some(crate::logs::CommandExitStatus::ExitCode { code: 3 })
        ));

        let (_, task, _, _) = find(
            &|a| matches!(a, ActionType::TaskCreate { description, .. } if description == "Read README first line"),
        );
        let ActionType::TaskCreate { result, .. } = task else {
            unreachable!()
        };
        let result = result.expect("subagent result");
        assert!(result.value.as_str().unwrap().contains("# Demo"));

        let (_, lookup, _, content) = find(
            &|a| matches!(a, ActionType::Other { description } if description.starts_with("Look up tool")),
        );
        assert_eq!(content, "Look up tool: TodoWrite");
        drop(lookup);

        assert!(entries.iter().any(|e| {
            matches!(e.entry_type, NormalizedEntryType::AssistantMessage)
                && e.content
                    .contains("All requested steps have been completed")
        }));
        assert!(
            !entries.iter().any(|e| {
                matches!(e.entry_type, NormalizedEntryType::SystemMessage)
                    && e.content.trim_start().starts_with('{')
            }),
            "raw JSON should never be shown as a system message"
        );
    }

    #[test]
    fn unknown_tool_kind_renders_name_and_concise_args() {
        let value = serde_json::json!({
            "readLintsToolCall": {
                "args": {
                    "paths": ["/tmp/vk-fixture-repo/src/app.py"],
                    "toolCallId": "tool_1",
                    "conversationId": "conv_1",
                    "requestedAtMs": "1793397921813"
                }
            },
            "hookAdditionalContexts": [],
            "toolCallId": "tool_1",
            "startedAtMs": "1793397921813"
        });
        let tool_call = CursorToolCall::from_value(value);
        assert_eq!(tool_call.get_name(), "readLints");
        let (action, content) = tool_call.to_action_and_content(FIXTURE_WORKTREE);
        assert_eq!(content, "Read lints: paths: src/app.py");
        assert!(matches!(action, ActionType::Other { description } if description == content));
    }

    #[test]
    fn known_kind_with_unexpected_args_falls_back_without_panicking() {
        let value = serde_json::json!({
            "readToolCall": { "args": { "file": "a.txt" }, "result": { "error": { "message": "nope" } } },
            "toolCallId": "tool_2",
        });
        let tool_call = CursorToolCall::from_value(value);
        assert!(matches!(&tool_call, CursorToolCall::Unknown { name, .. } if name == "read"));
        assert!(tool_call.is_failure());
        let (_, content) = tool_call.to_action_and_content(FIXTURE_WORKTREE);
        assert_eq!(content, "Read: file: a.txt");
    }

    #[tokio::test]
    async fn protocol_events_do_not_split_or_pollute_assistant_text() {
        let stdout = [
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}]},"session_id":"s"}"#,
            r#"{"type":"interaction_query","subtype":"request","query_type":"webFetchRequestQuery","query":{"id":0},"session_id":"s"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":" world"}]},"session_id":"s"}"#,
        ]
        .join("\n");
        let entries = normalize(&stdout, "").await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Hello world");
    }

    #[tokio::test]
    async fn model_rejection_is_explained_in_the_transcript() {
        let stderr =
            "Cannot use this model: composer-2.5. Available models: auto, gpt-5.2, gpt-5-mini\n";
        let entries = normalize("", stderr).await;
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::ErrorMessage { .. }
        ));
        assert!(
            entries[0]
                .content
                .starts_with("Cursor rejected model `composer-2.5` for this account.")
        );
        assert!(entries[0].content.contains("Available models: auto"));
    }

    #[tokio::test]
    async fn fallback_notice_and_error_results_are_shown() {
        let stdout = format!(
            "{}{}",
            notice_line(
                "Model `composer-2.5` is not available to this Cursor account, so this run uses Auto instead."
            ),
            r#"{"type":"result","subtype":"error","is_error":true,"result":"Rate limit exceeded","session_id":"s"}"#
        );
        let entries = normalize(&stdout, "").await;
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::SystemMessage
        ));
        assert!(entries[0].content.contains("uses Auto instead"));
        assert!(matches!(
            entries[1].entry_type,
            NormalizedEntryType::ErrorMessage { .. }
        ));
        assert_eq!(entries[1].content, "Rate limit exceeded");
    }

    #[test]
    fn command_uses_explicit_model_only_when_given() {
        let executor = test_executor();
        let with_model = executor
            .build_command_builder(Some("auto"))
            .unwrap()
            .build_initial()
            .unwrap();
        let args = format!("{with_model:?}");
        assert!(args.contains("\"--model\", \"auto\""), "{args}");
        let without = executor
            .build_command_builder(None)
            .unwrap()
            .build_initial()
            .unwrap();
        assert!(!format!("{without:?}").contains("--model"));
    }
}
