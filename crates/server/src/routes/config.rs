use std::collections::HashMap;

use api_types::LoginStatus;
use axum::{
    Json, Router,
    body::Body,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http,
    response::{IntoResponse, Json as ResponseJson, Response},
    routing::{get, put},
};
use deployment::{Deployment, DeploymentError};
use executors::{
    executors::{
        AvailabilityInfo, BaseAgentCapability, BaseCodingAgent, StandardCodingAgentExecutor,
    },
    mcp_config::{McpConfig, read_agent_config, write_agent_config},
    profile::{ExecutorConfigs, ExecutorProfileId},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use services::services::{
    config::{
        Config, ConfigError, SoundFile,
        editor::{EditorConfig, EditorType},
        save_config_to_file,
    },
    container::ContainerService,
    remote_client::RemoteClientError,
};
use tokio::fs;
use ts_rs::TS;
use utils::{assets::config_path, log_msg::LogMsg, response::ApiResponse};
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/info", get(get_user_system_info))
        .route("/config", put(update_config))
        .route("/sounds/{sound}", get(get_sound))
        .route("/mcp-config", get(get_mcp_servers).post(update_mcp_servers))
        .route("/profiles", get(get_profiles).put(update_profiles))
        .route(
            "/editors/check-availability",
            get(check_editor_availability),
        )
        .route("/agents/check-availability", get(check_agent_availability))
        .route("/agents/availability", get(get_all_agent_availability))
        .route("/agents/preset-options", get(get_agent_preset_options))
        .route(
            "/agents/discovered-options/ws",
            get(stream_executor_discovered_options_ws),
        )
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct Environment {
    pub os_type: String,
    pub os_version: String,
    pub os_architecture: String,
    pub bitness: String,
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        let info = os_info::get();
        Environment {
            os_type: info.os_type().to_string(),
            os_version: info.version().to_string(),
            os_architecture: info.architecture().unwrap_or("unknown").to_string(),
            bitness: info.bitness().to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct UserSystemInfo {
    pub version: String,
    pub config: Config,
    pub machine_id: String,
    pub login_status: LoginStatus,
    pub remote_auth_degraded: Option<String>,
    #[serde(flatten)]
    pub profiles: ExecutorConfigs,
    pub environment: Environment,
    /// Capabilities supported per executor (e.g., { "CLAUDE_CODE": ["SESSION_FORK"] })
    pub capabilities: HashMap<String, Vec<BaseAgentCapability>>,
    pub shared_api_base: Option<String>,
    pub preview_proxy_port: Option<u16>,
    /// Whether a GitHub personal access token is stored. The token itself is never returned.
    pub has_github_pat: bool,
}

impl UserSystemInfo {
    /// Strips every credential before the value leaves the server.
    fn sanitized(mut self) -> Self {
        self.has_github_pat = self.config.github.has_pat();
        self.config = public_config(&self.config);
        self.profiles = redact_profiles(&self.profiles);
        self
    }
}

/// Placeholder sent instead of executor environment variable values. Saving a
/// profile that still contains it keeps the stored value.
const REDACTED_ENV_VALUE: &str = "<redacted>";

const NO_STORE: [(http::HeaderName, &str); 1] = [(http::header::CACHE_CONTROL, "no-store")];

fn public_config(config: &Config) -> Config {
    let mut config = config.clone();
    config.github = config.github.redacted();
    config
}

/// Calls `f(executor, variant, agent, env)` for every `env` map in serialized profiles.
fn visit_profile_env(
    root: &mut Value,
    mut f: impl FnMut(&str, &str, &str, &mut serde_json::Map<String, Value>),
) {
    let Some(executors) = root.get_mut("executors").and_then(Value::as_object_mut) else {
        return;
    };
    for (executor, profile) in executors.iter_mut() {
        let Some(profile) = profile.as_object_mut() else {
            continue;
        };
        for (variant, config) in profile.iter_mut() {
            let Some(config) = config.as_object_mut() else {
                continue;
            };
            for (agent, settings) in config.iter_mut() {
                if let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) {
                    f(executor, variant, agent, env);
                }
            }
        }
    }
}

fn redact_profiles(profiles: &ExecutorConfigs) -> ExecutorConfigs {
    let redacted = serde_json::to_value(profiles).and_then(|mut value| {
        visit_profile_env(&mut value, |_, _, _, env| {
            for v in env.values_mut() {
                *v = Value::String(REDACTED_ENV_VALUE.to_string());
            }
        });
        serde_json::from_value(value)
    });
    redacted.unwrap_or_else(|e| {
        tracing::error!("Failed to redact executor profiles: {}", e);
        ExecutorConfigs::from_defaults()
    })
}

/// Replaces redaction placeholders in profiles received from a client with the
/// currently stored values; placeholders with no stored value are dropped.
fn restore_redacted_env(incoming: &mut Value, current: &ExecutorConfigs) {
    let current = serde_json::to_value(current).unwrap_or(Value::Null);
    visit_profile_env(incoming, |executor, variant, agent, env| {
        let stored = current
            .pointer(&format!(
                "/executors/{}/{}/{}/env",
                escape_pointer(executor),
                escape_pointer(variant),
                escape_pointer(agent)
            ))
            .and_then(Value::as_object);
        env.retain(|key, value| {
            if value.as_str() != Some(REDACTED_ENV_VALUE) {
                return true;
            }
            match stored.and_then(|s| s.get(key)) {
                Some(original) => {
                    *value = original.clone();
                    true
                }
                None => false,
            }
        });
    });
}

fn escape_pointer(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

// TODO: update frontend, BE schema has changed, this replaces GET /config and /config/constants
#[axum::debug_handler]
async fn get_user_system_info(State(deployment): State<DeploymentImpl>) -> impl IntoResponse {
    let config = deployment.config().read().await.clone();
    let login_status = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        deployment.get_login_status(),
    )
    .await
    {
        Ok(status) => status,
        Err(_) => {
            tracing::warn!("timed out determining login status for /api/info");

            let auth_context = deployment.auth_context();
            let cached_profile = auth_context.cached_profile().await;

            match auth_context.get_credentials().await {
                Some(_) => {
                    if auth_context.remote_auth_degraded_slug().await.is_none() {
                        auth_context
                            .set_remote_auth_degraded_slug(
                                RemoteClientError::generic_degraded_slug(),
                            )
                            .await;
                    }

                    deployment
                        .track_if_analytics_allowed(
                            "login_status_timeout",
                            serde_json::json!({
                                "has_cached_profile": cached_profile.is_some(),
                            }),
                        )
                        .await;

                    LoginStatus::LoggedIn {
                        profile: cached_profile,
                    }
                }
                None => {
                    auth_context.clear_profile().await;
                    auth_context.clear_remote_auth_degraded_slug().await;
                    LoginStatus::LoggedOut
                }
            }
        }
    };

    let user_system_info = UserSystemInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        config,
        machine_id: deployment.user_id().to_string(),
        login_status,
        remote_auth_degraded: deployment.auth_context().remote_auth_degraded_slug().await,
        profiles: ExecutorConfigs::get_cached(),
        environment: Environment::new(),
        capabilities: {
            let mut caps: HashMap<String, Vec<BaseAgentCapability>> = HashMap::new();
            let profs = ExecutorConfigs::get_cached();
            for key in profs.executors.keys() {
                if let Some(agent) = profs.get_coding_agent(&ExecutorProfileId::new(*key)) {
                    caps.insert(key.to_string(), agent.capabilities());
                }
            }
            caps
        },
        shared_api_base: deployment.shared_api_base().map(str::to_owned),
        preview_proxy_port: deployment.client_info().get_preview_proxy_port(),
        has_github_pat: false,
    }
    .sanitized();

    (
        NO_STORE,
        ResponseJson(ApiResponse::<_, ()>::success(user_system_info)),
    )
}

async fn update_config(
    State(deployment): State<DeploymentImpl>,
    Json(new_config): Json<Config>,
) -> impl IntoResponse {
    (
        NO_STORE,
        ResponseJson(apply_config_update(&deployment, new_config).await),
    )
}

async fn apply_config_update(
    deployment: &DeploymentImpl,
    mut new_config: Config,
) -> ApiResponse<Config> {
    let config_path = config_path();

    // Validate git branch prefix
    if !git::is_valid_branch_prefix(&new_config.git_branch_prefix) {
        return ApiResponse::error(
            "Invalid git branch prefix. Must be a valid git branch name component without slashes.",
        );
    }

    // Hold the write lock so concurrent saves cannot drop each other's credentials.
    let mut config = deployment.config().write().await;
    let old_config = config.clone();
    new_config.github = old_config.github.merge_update(new_config.github);

    match save_config_to_file(&new_config, &config_path).await {
        Ok(_) => {
            *config = new_config.clone();
            drop(config);

            // Track config events when fields transition from false → true
            track_config_events(deployment, &old_config, &new_config).await;

            ApiResponse::success(public_config(&new_config))
        }
        Err(e) => ApiResponse::error(&format!("Failed to save config: {}", e)),
    }
}

/// Track config events when fields transition from false → true
async fn track_config_events(deployment: &DeploymentImpl, old: &Config, new: &Config) {
    let events = [
        (
            !old.disclaimer_acknowledged && new.disclaimer_acknowledged,
            "onboarding_disclaimer_accepted",
            serde_json::json!({}),
        ),
        (
            !old.onboarding_acknowledged && new.onboarding_acknowledged,
            "onboarding_completed",
            serde_json::json!({
                "profile": new.executor_profile,
                "editor": new.editor
            }),
        ),
        (
            !old.analytics_enabled && new.analytics_enabled,
            "analytics_session_start",
            serde_json::json!({}),
        ),
    ];

    for (should_track, event_name, properties) in events {
        if should_track {
            deployment
                .track_if_analytics_allowed(event_name, properties)
                .await;
        }
    }
}

async fn get_sound(Path(sound): Path<SoundFile>) -> Result<Response, ApiError> {
    let sound = sound.serve().await.map_err(DeploymentError::Other)?;
    let response = Response::builder()
        .status(http::StatusCode::OK)
        .header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("audio/wav"),
        )
        .body(Body::from(sound.data.into_owned()))
        .unwrap();
    Ok(response)
}

#[derive(TS, Debug, Deserialize)]
pub struct McpServerQuery {
    executor: BaseCodingAgent,
}

#[derive(TS, Debug, Serialize, Deserialize)]
pub struct GetMcpServerResponse {
    // servers: HashMap<String, Value>,
    mcp_config: McpConfig,
    config_path: String,
}

#[derive(TS, Debug, Serialize, Deserialize)]
pub struct UpdateMcpServersBody {
    servers: HashMap<String, Value>,
}

async fn get_mcp_servers(
    State(_deployment): State<DeploymentImpl>,
    Query(query): Query<McpServerQuery>,
) -> Result<ResponseJson<ApiResponse<GetMcpServerResponse>>, ApiError> {
    let coding_agent = ExecutorConfigs::get_cached()
        .get_coding_agent(&ExecutorProfileId::new(query.executor))
        .ok_or(ConfigError::ValidationError(
            "Executor not found".to_string(),
        ))?;

    if !coding_agent.supports_mcp() {
        return Ok(ResponseJson(ApiResponse::error(
            "MCP not supported by this executor",
        )));
    }

    // Resolve supplied config path or agent default
    let config_path = match coding_agent.default_mcp_config_path() {
        Some(path) => path,
        None => {
            return Ok(ResponseJson(ApiResponse::error(
                "Could not determine config file path",
            )));
        }
    };

    let mut mcpc = coding_agent.get_mcp_config();
    let raw_config = read_agent_config(&config_path, &mcpc).await?;
    let servers = get_mcp_servers_from_config_path(&raw_config, &mcpc.servers_path);
    mcpc.set_servers(servers);
    Ok(ResponseJson(ApiResponse::success(GetMcpServerResponse {
        mcp_config: mcpc,
        config_path: config_path.to_string_lossy().to_string(),
    })))
}

async fn update_mcp_servers(
    State(_deployment): State<DeploymentImpl>,
    Query(query): Query<McpServerQuery>,
    Json(payload): Json<UpdateMcpServersBody>,
) -> Result<ResponseJson<ApiResponse<String>>, ApiError> {
    let profiles = ExecutorConfigs::get_cached();
    let agent = profiles
        .get_coding_agent(&ExecutorProfileId::new(query.executor))
        .ok_or(ConfigError::ValidationError(
            "Executor not found".to_string(),
        ))?;

    if !agent.supports_mcp() {
        return Ok(ResponseJson(ApiResponse::error(
            "This executor does not support MCP servers",
        )));
    }

    // Resolve supplied config path or agent default
    let config_path = match agent.default_mcp_config_path() {
        Some(path) => path.to_path_buf(),
        None => {
            return Ok(ResponseJson(ApiResponse::error(
                "Could not determine config file path",
            )));
        }
    };

    let mcpc = agent.get_mcp_config();
    match update_mcp_servers_in_config(&config_path, &mcpc, payload.servers).await {
        Ok(message) => Ok(ResponseJson(ApiResponse::success(message))),
        Err(e) => Ok(ResponseJson(ApiResponse::error(&format!(
            "Failed to update MCP servers: {}",
            e
        )))),
    }
}

async fn update_mcp_servers_in_config(
    config_path: &std::path::Path,
    mcpc: &McpConfig,
    new_servers: HashMap<String, Value>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    // Read existing config (JSON or TOML depending on agent)
    let mut config = read_agent_config(config_path, mcpc).await?;

    // Get the current server count for comparison
    let old_servers = get_mcp_servers_from_config_path(&config, &mcpc.servers_path).len();

    // Set the MCP servers using the correct attribute path
    set_mcp_servers_in_config_path(&mut config, &mcpc.servers_path, &new_servers)?;

    // Write the updated config back to file (JSON or TOML depending on agent)
    write_agent_config(config_path, mcpc, &config).await?;

    let new_count = new_servers.len();
    let message = match (old_servers, new_count) {
        (0, 0) => "No MCP servers configured".to_string(),
        (0, n) => format!("Added {} MCP server(s)", n),
        (old, new) if old == new => format!("Updated MCP server configuration ({} server(s))", new),
        (old, new) => format!(
            "Updated MCP server configuration (was {}, now {})",
            old, new
        ),
    };

    Ok(message)
}

/// Helper function to get MCP servers from config using a path
fn get_mcp_servers_from_config_path(raw_config: &Value, path: &[String]) -> HashMap<String, Value> {
    let mut current = raw_config;
    for part in path {
        current = match current.get(part) {
            Some(val) => val,
            None => return HashMap::new(),
        };
    }
    // Extract the servers object
    match current.as_object() {
        Some(servers) => servers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        None => HashMap::new(),
    }
}

/// Helper function to set MCP servers in config using a path
fn set_mcp_servers_in_config_path(
    raw_config: &mut Value,
    path: &[String],
    servers: &HashMap<String, Value>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Ensure config is an object
    if !raw_config.is_object() {
        *raw_config = serde_json::json!({});
    }

    let mut current = raw_config;
    // Navigate/create the nested structure (all parts except the last)
    for part in &path[..path.len() - 1] {
        if current.get(part).is_none() {
            current
                .as_object_mut()
                .unwrap()
                .insert(part.to_string(), serde_json::json!({}));
        }
        current = current.get_mut(part).unwrap();
        if !current.is_object() {
            *current = serde_json::json!({});
        }
    }

    // Set the final attribute
    let final_attr = path.last().unwrap();
    current
        .as_object_mut()
        .unwrap()
        .insert(final_attr.to_string(), serde_json::to_value(servers)?);

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfilesContent {
    pub content: String,
    pub path: String,
}

async fn get_profiles(State(_deployment): State<DeploymentImpl>) -> impl IntoResponse {
    let profiles_path = utils::assets::profiles_path();

    // Use cached data to ensure consistency with runtime and PUT updates
    let profiles = redact_profiles(&ExecutorConfigs::get_cached());

    let content = serde_json::to_string_pretty(&profiles).unwrap_or_else(|e| {
        tracing::error!("Failed to serialize profiles to JSON: {}", e);
        serde_json::to_string_pretty(&ExecutorConfigs::from_defaults())
            .unwrap_or_else(|_| "{}".to_string())
    });

    (
        NO_STORE,
        ResponseJson(ApiResponse::<_, ()>::success(ProfilesContent {
            content,
            path: profiles_path.display().to_string(),
        })),
    )
}

async fn update_profiles(
    State(_deployment): State<DeploymentImpl>,
    body: String,
) -> ResponseJson<ApiResponse<String>> {
    // Try to parse as ExecutorProfileConfigs format
    let parsed = serde_json::from_str::<Value>(&body).and_then(|mut value| {
        restore_redacted_env(&mut value, &ExecutorConfigs::get_cached());
        serde_json::from_value::<ExecutorConfigs>(value)
    });
    match parsed {
        Ok(executor_profiles) => {
            // Save the profiles to file
            match executor_profiles.save_overrides() {
                Ok(_) => {
                    tracing::info!("Executor profiles saved successfully");
                    // Reload the cached profiles
                    ExecutorConfigs::reload();
                    ResponseJson(ApiResponse::success(
                        "Executor profiles updated successfully".to_string(),
                    ))
                }
                Err(e) => {
                    tracing::error!("Failed to save executor profiles: {}", e);
                    ResponseJson(ApiResponse::error(&format!(
                        "Failed to save executor profiles: {}",
                        e
                    )))
                }
            }
        }
        Err(e) => ResponseJson(ApiResponse::error(&format!(
            "Invalid executor profiles format: {}",
            e
        ))),
    }
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CheckEditorAvailabilityQuery {
    editor_type: EditorType,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CheckEditorAvailabilityResponse {
    available: bool,
}

async fn check_editor_availability(
    State(_deployment): State<DeploymentImpl>,
    Query(query): Query<CheckEditorAvailabilityQuery>,
) -> ResponseJson<ApiResponse<CheckEditorAvailabilityResponse>> {
    // Construct a minimal EditorConfig for checking
    let editor_config = EditorConfig::new(
        query.editor_type,
        None,  // custom_command
        None,  // remote_ssh_host
        None,  // remote_ssh_user
        false, // auto_install_extension
    );

    let available = editor_config.check_availability().await;
    ResponseJson(ApiResponse::success(CheckEditorAvailabilityResponse {
        available,
    }))
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CheckAgentAvailabilityQuery {
    executor: BaseCodingAgent,
}

async fn check_agent_availability(
    State(_deployment): State<DeploymentImpl>,
    Query(query): Query<CheckAgentAvailabilityQuery>,
) -> ResponseJson<ApiResponse<AvailabilityInfo>> {
    let profiles = ExecutorConfigs::get_cached();
    let profile_id = ExecutorProfileId::new(query.executor);

    let info = match profiles.get_coding_agent(&profile_id) {
        Some(agent) => agent.get_availability_info(),
        None => AvailabilityInfo::NotFound,
    };

    ResponseJson(ApiResponse::success(info))
}

/// Availability of every configured executor, keyed by executor name.
async fn get_all_agent_availability(
    State(_deployment): State<DeploymentImpl>,
) -> ResponseJson<ApiResponse<HashMap<String, AvailabilityInfo>>> {
    let availability = tokio::task::spawn_blocking(|| {
        let profiles = ExecutorConfigs::get_cached();
        profiles
            .executors
            .keys()
            .map(|executor| {
                let info = profiles
                    .get_coding_agent(&ExecutorProfileId::new(*executor))
                    .map(|agent| agent.get_availability_info())
                    .unwrap_or(AvailabilityInfo::NotFound);
                (executor.to_string(), info)
            })
            .collect::<HashMap<_, _>>()
    })
    .await
    .unwrap_or_default();

    ResponseJson(ApiResponse::success(availability))
}

#[derive(Debug, Deserialize, TS)]
pub struct AgentPresetOptionsQuery {
    pub executor: BaseCodingAgent,
    pub variant: Option<String>,
}

async fn get_agent_preset_options(
    Query(query): Query<AgentPresetOptionsQuery>,
) -> ResponseJson<ApiResponse<executors::profile::ExecutorConfig>> {
    let profiles = ExecutorConfigs::get_cached();
    let profile_id = if let Some(variant) = query.variant {
        ExecutorProfileId::with_variant(query.executor, variant)
    } else {
        ExecutorProfileId::new(query.executor)
    };

    let options = match profiles.get_coding_agent(&profile_id) {
        Some(agent) => agent.get_preset_options(),
        None => {
            // Return a default config if not found
            executors::profile::ExecutorConfig::new(query.executor)
        }
    };

    ResponseJson(ApiResponse::success(options))
}

#[derive(Debug, Deserialize)]
pub struct ExecutorDiscoveredOptionsStreamQuery {
    executor: BaseCodingAgent,
    #[serde(default)]
    session_id: Option<Uuid>,
    #[serde(default)]
    workspace_id: Option<Uuid>,
    #[serde(default)]
    repo_id: Option<Uuid>,
}

pub async fn stream_executor_discovered_options_ws(
    ws: WebSocketUpgrade,
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ExecutorDiscoveredOptionsStreamQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_executor_discovered_options_ws(socket, deployment, query).await {
            tracing::warn!("discovered options WS closed: {}", e);
        }
    })
}

async fn handle_executor_discovered_options_ws(
    mut socket: WebSocket,
    deployment: DeploymentImpl,
    query: ExecutorDiscoveredOptionsStreamQuery,
) -> anyhow::Result<()> {
    use futures_util::StreamExt;

    match deployment
        .container()
        .discover_executor_options(
            ExecutorProfileId::new(query.executor),
            query.session_id,
            query.workspace_id,
            query.repo_id,
        )
        .await
    {
        Ok(Some(mut stream)) => {
            if let Some(patch) = stream.next().await {
                let _ = socket
                    .send(LogMsg::JsonPatch(patch).to_ws_message_unchecked())
                    .await;
            }

            let _ = socket.send(LogMsg::Ready.to_ws_message_unchecked()).await;

            loop {
                tokio::select! {
                    patch = stream.next() => {
                        let Some(patch) = patch else {
                            break;
                        };
                        if socket
                            .send(LogMsg::JsonPatch(patch).to_ws_message_unchecked())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    inbound = socket.recv() => {
                        match inbound {
                            Some(Ok(Message::Close(_))) => break,
                            Some(Ok(_)) => {}
                            None => break,
                            Some(Err(_)) => break,
                        }
                    }
                }
            }
        }
        Ok(None) => {
            let _ = socket.send(LogMsg::Ready.to_ws_message_unchecked()).await;
        }
        Err(e) => {
            tracing::warn!("Failed to start discovered options stream: {}", e);
        }
    }

    let _ = socket
        .send(LogMsg::Finished.to_ws_message_unchecked())
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAT: &str = "sentinel-github-pat";
    const OAUTH: &str = "sentinel-oauth-token";
    const ENV_SECRET: &str = "sentinel-env-api-key";

    fn config_with_secrets() -> Config {
        let mut config = Config::default();
        config.github.pat = Some(PAT.to_string());
        config.github.oauth_token = Some(OAUTH.to_string());
        config.github.username = Some("octocat".to_string());
        config
    }

    fn profiles_with_env_secret() -> ExecutorConfigs {
        let mut value = serde_json::to_value(ExecutorConfigs::from_defaults()).unwrap();
        let agent = value
            .pointer_mut("/executors/CLAUDE_CODE/DEFAULT/CLAUDE_CODE")
            .and_then(Value::as_object_mut)
            .expect("default Claude Code profile");
        agent.insert(
            "env".to_string(),
            serde_json::json!({ "ANTHROPIC_API_KEY": ENV_SECRET, "PLAIN": "x" }),
        );
        serde_json::from_value(value).unwrap()
    }

    fn info(config: Config, profiles: ExecutorConfigs) -> UserSystemInfo {
        UserSystemInfo {
            version: "test".to_string(),
            config,
            machine_id: "machine".to_string(),
            login_status: LoginStatus::LoggedOut,
            remote_auth_degraded: None,
            profiles,
            environment: Environment::new(),
            capabilities: HashMap::new(),
            shared_api_base: None,
            preview_proxy_port: None,
            has_github_pat: false,
        }
        .sanitized()
    }

    #[test]
    fn info_response_contains_no_secrets() {
        let info = info(config_with_secrets(), profiles_with_env_secret());
        let json = serde_json::to_string(&ApiResponse::<_, ()>::success(info)).unwrap();

        for secret in [PAT, OAUTH, ENV_SECRET] {
            assert!(!json.contains(secret), "response leaked {secret}");
        }
        let value: Value = serde_json::from_str(&json).unwrap();
        let github = &value["data"]["config"]["github"];
        assert!(github.get("pat").is_none());
        assert!(github.get("oauth_token").is_none());
        assert_eq!(github["username"], "octocat");
        assert_eq!(value["data"]["has_github_pat"], true);
        assert_eq!(
            value["data"]["executors"]["CLAUDE_CODE"]["DEFAULT"]["CLAUDE_CODE"]["env"]["ANTHROPIC_API_KEY"],
            REDACTED_ENV_VALUE
        );
    }

    #[test]
    fn info_reports_missing_pat() {
        let info = info(Config::default(), ExecutorConfigs::from_defaults());
        assert!(!info.has_github_pat);
    }

    #[test]
    fn settings_save_without_token_keeps_stored_token() {
        let stored = config_with_secrets();
        // The client only ever sees the public config and sends it back.
        let sent = serde_json::to_string(&public_config(&stored)).unwrap();
        let mut incoming: Config = serde_json::from_str(&sent).unwrap();
        incoming.git_branch_prefix = "feat".to_string();

        let merged = stored.github.merge_update(incoming.github);
        assert_eq!(merged.pat.as_deref(), Some(PAT));
        assert_eq!(merged.oauth_token.as_deref(), Some(OAUTH));
        assert_eq!(merged.username.as_deref(), Some("octocat"));
    }

    #[test]
    fn settings_save_can_replace_or_clear_token() {
        let stored = config_with_secrets().github;

        let mut replace = stored.redacted();
        replace.pat = Some("new-pat".to_string());
        assert_eq!(stored.merge_update(replace).pat.as_deref(), Some("new-pat"));

        let mut clear = stored.redacted();
        clear.pat = Some(String::new());
        assert_eq!(stored.merge_update(clear).pat, None);
    }

    #[test]
    fn redacted_profile_env_is_restored_on_save() {
        let stored = profiles_with_env_secret();
        let mut sent = serde_json::to_value(redact_profiles(&stored)).unwrap();
        let env_ptr = "/executors/CLAUDE_CODE/DEFAULT/CLAUDE_CODE/env";
        sent.pointer_mut(env_ptr)
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert("NEW_KEY".to_string(), Value::String("typed".to_string()));

        restore_redacted_env(&mut sent, &stored);

        let env = sent.pointer(env_ptr).unwrap();
        assert_eq!(env["ANTHROPIC_API_KEY"], ENV_SECRET);
        assert_eq!(env["PLAIN"], "x");
        assert_eq!(env["NEW_KEY"], "typed");
        serde_json::from_value::<ExecutorConfigs>(sent).unwrap();
    }

    #[test]
    fn unknown_redaction_placeholder_is_dropped() {
        let mut sent = serde_json::to_value(ExecutorConfigs::from_defaults()).unwrap();
        sent.pointer_mut("/executors/CLAUDE_CODE/DEFAULT/CLAUDE_CODE")
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert(
                "env".to_string(),
                serde_json::json!({ "GONE": REDACTED_ENV_VALUE }),
            );

        restore_redacted_env(&mut sent, &ExecutorConfigs::from_defaults());

        let env = sent
            .pointer("/executors/CLAUDE_CODE/DEFAULT/CLAUDE_CODE/env")
            .unwrap();
        assert!(env.get("GONE").is_none());
    }
}
