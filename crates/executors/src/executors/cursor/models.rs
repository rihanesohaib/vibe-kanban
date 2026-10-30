//! Discovers the models available to the Cursor CLI account by running
//! `cursor-agent --list-models`, with an in-memory cache and a static fallback.

use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use tokio::process::Command;

use crate::{
    command::{CmdOverrides, CommandBuilder},
    model_selector::ModelInfo,
};

const LIST_MODELS_TIMEOUT: Duration = Duration::from_secs(20);
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Failed lookups are retried sooner so a transient CLI/network error does not
/// pin the fallback list for the full TTL.
const FAILURE_CACHE_TTL: Duration = Duration::from_secs(60);

pub const DEFAULT_MODEL_ID: &str = "auto";

/// Orders discovered models so the strongest options appear first. Only ids the
/// CLI actually lists are shown; these are never offered on their own.
const FEATURED_MODEL_IDS: &[&str] = &[
    "auto",
    "claude-opus-5-thinking-high",
    "claude-opus-5-thinking-high-fast",
    "gpt-5.6-sol-high",
    "gpt-5.6-sol-high-fast",
    "gpt-5.6-sol-xhigh",
    "gpt-5.6-sol-xhigh-fast",
    "claude-sonnet-5-thinking-high",
    "claude-sonnet-5-thinking-xhigh",
    "gpt-5.6-luna-high",
    "gemini-3.7-flash-high",
];

#[derive(Debug, Clone)]
pub struct CursorModelList {
    pub models: Vec<ModelInfo>,
    pub default_model: Option<String>,
    /// Set when the list is the static fallback because discovery failed.
    pub error: Option<String>,
}

fn model(id: &str, name: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: name.to_string(),
        provider_id: None,
        reasoning_options: vec![],
    }
}

/// Used while discovery is running or after it fails: only `auto`, which every
/// account accepts.
pub fn fallback_models() -> CursorModelList {
    CursorModelList {
        models: vec![model(DEFAULT_MODEL_ID, "Auto")],
        default_model: Some(DEFAULT_MODEL_ID.to_string()),
        error: None,
    }
}

/// True only when discovery succeeded and the CLI did not list `model_id`.
/// Parameterized ids (`id[effort=high]`) are never listed, so they pass.
pub fn is_known_unavailable(list: &CursorModelList, model_id: &str) -> bool {
    if list.error.is_some() || list.models.is_empty() || model_id.contains('[') {
        return false;
    }
    !list
        .models
        .iter()
        .any(|m| m.id.eq_ignore_ascii_case(model_id))
}

fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
    )
}

/// Cursor model ids are lowercase slugs such as `gpt-5.6-sol-high` or
/// `claude-opus-5-thinking-high-fast`. Rejecting uppercase letters filters out
/// headers ("Available models", "ID", "Tip: ...") without a keyword list.
fn is_model_id(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && token.chars().all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, '-' | '_' | '.' | ':' | '/' | '[' | ']' | '=' | ',')
        })
}

/// Strips `(default)` / `(current)` markers from a display name, returning
/// whether the default marker was present.
fn clean_name(raw: &str) -> (String, bool) {
    let mut name = raw.to_string();
    let mut is_default = false;
    for marker in ["(default)", "(current)"] {
        if let Some(pos) = name.to_ascii_lowercase().find(marker) {
            is_default |= marker == "(default)";
            name.replace_range(pos..pos + marker.len(), "");
        }
    }
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (name, is_default)
}

/// Parses the output of `cursor-agent --list-models` / `cursor-agent models`.
///
/// The CLI prints `id - Display Name` per line under an `Available models`
/// header, followed by a `Tip:` footer. Table-style output (`id  Name` or
/// `| id | Name |`) is accepted too: the first column is the id.
pub fn parse_models_output(output: &str) -> CursorModelList {
    let output = strip_ansi_escapes::strip_str(output);
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    let mut default_model = None;

    for raw_line in output.lines() {
        let line: String = raw_line.chars().filter(|c| !is_invisible(*c)).collect();
        let line = line.trim().trim_matches(|c| c == '|' || c == '│').trim();
        if line.is_empty() || line.ends_with(':') {
            continue;
        }

        let (id, rest) = if let Some((id, name)) = line.split_once(" - ") {
            (id.trim(), name.trim())
        } else if line.contains('|') || line.contains('│') {
            let mut cells = line
                .split(['|', '│'])
                .map(str::trim)
                .filter(|c| !c.is_empty());
            (cells.next().unwrap_or(""), cells.next().unwrap_or(""))
        } else {
            match line.split_once(char::is_whitespace) {
                Some((id, rest)) => (id, rest.trim()),
                None => (line, ""),
            }
        };

        if !is_model_id(id) || !seen.insert(id.to_string()) {
            continue;
        }

        let (name, is_default) = clean_name(rest);
        if is_default && default_model.is_none() {
            default_model = Some(id.to_string());
        }
        let name = if name.is_empty() {
            id.to_string()
        } else {
            name
        };
        models.push(model(id, &name));
    }

    CursorModelList {
        models: order_featured_first(models),
        default_model,
        error: None,
    }
}

fn order_featured_first(models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let rank: HashMap<&str, usize> = FEATURED_MODEL_IDS
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    let (mut featured, rest): (Vec<_>, Vec<_>) = models
        .into_iter()
        .partition(|m| rank.contains_key(m.id.as_str()));
    featured.sort_by_key(|m| rank[m.id.as_str()]);
    featured.extend(rest);
    featured
}

struct CacheEntry {
    expires_at: Instant,
    value: CursorModelList,
}

static CACHE: LazyLock<Mutex<HashMap<String, CacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Serialises CLI invocations so concurrent discovery requests share one call.
static FETCH_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn cache_key(base_command: &str, cmd: &CmdOverrides) -> String {
    let mut key = cmd
        .base_command_override
        .clone()
        .unwrap_or_else(|| base_command.to_string());
    if let Some(env) = &cmd.env {
        let mut pairs: Vec<_> = env.iter().collect();
        pairs.sort();
        for (k, v) in pairs {
            key.push_str(&format!("\0{k}={v}"));
        }
    }
    key
}

pub fn cached_models(base_command: &str, cmd: &CmdOverrides) -> Option<CursorModelList> {
    let key = cache_key(base_command, cmd);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match cache.get(&key) {
        Some(entry) if entry.expires_at > Instant::now() => Some(entry.value.clone()),
        Some(_) => {
            cache.remove(&key);
            None
        }
        None => None,
    }
}

/// Returns the discovered model list, querying the CLI at most once per TTL.
/// Never fails: on error the static fallback is returned with `error` set.
pub async fn discover_models(base_command: &str, cmd: &CmdOverrides) -> CursorModelList {
    if let Some(cached) = cached_models(base_command, cmd) {
        return cached;
    }
    let _guard = FETCH_LOCK.lock().await;
    if let Some(cached) = cached_models(base_command, cmd) {
        return cached;
    }

    let (value, ttl) = match list_models_from_cli(base_command, cmd).await {
        Ok(list) => (list, CACHE_TTL),
        Err(err) => {
            tracing::warn!("Cursor model discovery failed, using fallback list: {err}");
            let mut fallback = fallback_models();
            fallback.error = Some(format!("Could not list Cursor models: {err}"));
            (fallback, FAILURE_CACHE_TTL)
        }
    };

    CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(
        cache_key(base_command, cmd),
        CacheEntry {
            expires_at: Instant::now() + ttl,
            value: value.clone(),
        },
    );
    value
}

async fn list_models_from_cli(
    base_command: &str,
    cmd: &CmdOverrides,
) -> Result<CursorModelList, String> {
    let mut builder = CommandBuilder::new(base_command).params(["--list-models"]);
    if let Some(base) = &cmd.base_command_override {
        builder = builder.override_base(base.clone());
    }
    let (program, args) = builder
        .build_initial()
        .map_err(|e| e.to_string())?
        .into_resolved()
        .await
        .map_err(|e| e.to_string())?;

    let mut command = Command::new(program);
    command
        .args(&args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = &cmd.env {
        command.envs(env);
    }

    let output = tokio::time::timeout(LIST_MODELS_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("timed out after {}s", LIST_MODELS_TIMEOUT.as_secs()))?
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = strip_ansi_escapes::strip_str(stderr.trim());
        return Err(format!(
            "exited with {}: {}",
            output.status,
            detail.lines().next().unwrap_or_default()
        ));
    }

    let parsed = parse_models_output(&String::from_utf8_lossy(&output.stdout));
    if parsed.models.is_empty() {
        return Err("no models found in CLI output".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_OUTPUT: &str = "Available models\n\
\n\
auto - Auto (default)\n\
gpt-5.3-codex-low - Codex 5.3 Low\n\
gpt-5.2 - GPT-5.2\n\
claude-opus-5-thinking-high - Claude Opus 5 1M Thinking\n\
gpt-5.6-sol-high - GPT-5.6 Sol 1M High\n\
grok-4.7-low-fast - Grok 4.7  Low Fast\u{200B}\u{200B}\n\
claude-haiku-5-5-low - Claude Haiku 5.5  Low No Thinking\n\
gpt-5-mini - GPT-5 Mini\n\
\n\
Tip: use --model <id> (or /model <id> in interactive mode) to switch. Parameterized models also accept quoted overrides, e.g. --model 'claude-opus-4-8[context=1m,effort=high,fast=false]'.\n";

    fn ids(list: &CursorModelList) -> Vec<&str> {
        list.models.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn parses_real_cli_output() {
        let list = parse_models_output(REAL_OUTPUT);
        assert_eq!(
            ids(&list),
            vec![
                "auto",
                "claude-opus-5-thinking-high",
                "gpt-5.6-sol-high",
                "gpt-5.3-codex-low",
                "gpt-5.2",
                "grok-4.7-low-fast",
                "claude-haiku-5-5-low",
                "gpt-5-mini",
            ]
        );
        assert_eq!(list.default_model.as_deref(), Some("auto"));
        assert_eq!(list.models[0].name, "Auto");
        let grok = list
            .models
            .iter()
            .find(|m| m.id == "grok-4.7-low-fast")
            .unwrap();
        assert_eq!(grok.name, "Grok 4.7 Low Fast");
        assert!(list.models.iter().all(|m| m.reasoning_options.is_empty()));
    }

    #[test]
    fn parses_bare_ids_one_per_line() {
        let list = parse_models_output("auto\ngpt-5.6-luna-high\n\ngemini-3.7-flash-high\n");
        assert_eq!(
            ids(&list),
            vec!["auto", "gpt-5.6-luna-high", "gemini-3.7-flash-high"]
        );
        assert_eq!(list.models[1].name, "gpt-5.6-luna-high");
        assert_eq!(list.default_model, None);
    }

    #[test]
    fn parses_table_output_and_skips_headers() {
        let output = "\
ID                          NAME
auto                        Auto (current)
claude-sonnet-5-thinking-xhigh  Claude Sonnet 5 1M Extra High Thinking
| gpt-5.6-sol-xhigh-fast | GPT-5.6 Sol 1M Extra High Fast |
+------------------------+------+
";
        let list = parse_models_output(output);
        assert_eq!(
            ids(&list),
            vec![
                "auto",
                "gpt-5.6-sol-xhigh-fast",
                "claude-sonnet-5-thinking-xhigh",
            ]
        );
        assert_eq!(list.models[0].name, "Auto");
        assert_eq!(list.default_model, None);
    }

    #[test]
    fn strips_ansi_and_dedupes() {
        let output =
            "\x1b[1mAvailable models\x1b[0m\n\x1b[32mauto\x1b[0m - Auto\nauto - Auto again\n";
        let list = parse_models_output(output);
        assert_eq!(ids(&list), vec!["auto"]);
        assert_eq!(list.models[0].name, "Auto");
    }

    #[test]
    fn empty_or_garbage_output_yields_no_models() {
        assert!(parse_models_output("").models.is_empty());
        assert!(
            parse_models_output("Error: Not authenticated\nPlease run login:\n")
                .models
                .is_empty()
        );
    }

    #[test]
    fn fallback_offers_only_auto() {
        let list = fallback_models();
        assert_eq!(list.default_model.as_deref(), Some(DEFAULT_MODEL_ID));
        assert_eq!(ids(&list), vec!["auto"]);
    }

    #[test]
    fn featured_ordering_never_adds_unlisted_ids() {
        let list = parse_models_output("gpt-5-mini - GPT-5 Mini\nauto - Auto\n");
        assert_eq!(ids(&list), vec!["auto", "gpt-5-mini"]);
    }

    #[test]
    fn unavailable_only_when_discovery_succeeded_without_the_id() {
        let discovered = parse_models_output(REAL_OUTPUT);
        assert!(is_known_unavailable(&discovered, "composer-2.5"));
        assert!(is_known_unavailable(&discovered, "kimi-k2.5"));
        assert!(!is_known_unavailable(&discovered, "auto"));
        assert!(!is_known_unavailable(&discovered, "GPT-5.2"));
        assert!(!is_known_unavailable(
            &discovered,
            "claude-opus-4-8[effort=high]"
        ));

        let mut failed = fallback_models();
        failed.error = Some("Could not list Cursor models: timed out".to_string());
        assert!(!is_known_unavailable(&failed, "composer-2.5"));
    }
}
