use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use ts_rs::TS;
use utils::{self, command_ext::NoWindowExt};
use uuid::Uuid;

use crate::services::config::{Config, SoundFile};

/// What happened to an agent run that the user may want to know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentAlertKind {
    Finished,
    Failed,
    NeedsApproval,
    NeedsInput,
    Merged,
}

/// A live agent event delivered to open browser tabs. `id` matches the id of
/// the corresponding agent inbox item.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentAlert {
    pub id: String,
    pub kind: AgentAlertKind,
    pub title: String,
    pub body: String,
    pub workspace_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// Set by `NotificationService::alert`: the alert is listed but must not
    /// play a sound or show a notification.
    #[serde(default)]
    pub silent: bool,
}

impl AgentAlert {
    pub fn run_id(execution_process_id: Uuid) -> String {
        format!("run:{execution_process_id}")
    }

    pub fn approval_id(approval_id: &str) -> String {
        format!("approval:{approval_id}")
    }

    pub fn merge_id(merge_id: Uuid) -> String {
        format!("merge:{merge_id}")
    }
}

const ALERT_CHANNEL_CAPACITY: usize = 64;

/// Trait for sending push notifications. Implementations can use
/// platform-specific OS commands, Tauri's notification plugin, etc.
#[async_trait]
pub trait PushNotifier: Send + Sync + 'static {
    async fn send(&self, title: &str, message: &str, workspace_id: Option<Uuid>);
}

/// Global push notifier set before server startup (e.g., by the Tauri app).
/// Falls back to `DefaultPushNotifier` if not set.
static GLOBAL_PUSH_NOTIFIER: OnceLock<Arc<dyn PushNotifier>> = OnceLock::new();

/// Register a custom push notifier globally. Must be called before the server
/// starts (i.e., before `LocalDeployment::new()`). Typically called from the
/// Tauri app to inject a `TauriNotifier` that uses the native notification API.
pub fn set_global_push_notifier(notifier: Arc<dyn PushNotifier>) {
    let _ = GLOBAL_PUSH_NOTIFIER.set(notifier);
}

/// Get the global push notifier, or `DefaultPushNotifier` if none was set.
pub fn get_global_push_notifier() -> Arc<dyn PushNotifier> {
    GLOBAL_PUSH_NOTIFIER
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(DefaultPushNotifier))
}

/// Default push notifier using platform-specific OS commands.
/// Used as a fallback when no Tauri app handle is available.
pub struct DefaultPushNotifier;

/// Cache for WSL root path from PowerShell
static WSL_ROOT_PATH_CACHE: OnceLock<Option<String>> = OnceLock::new();

/// Whether OS-level sound/notification commands have anywhere to go. A
/// headless Linux host (no X11/Wayland session) has neither a notification
/// daemon nor an audio sink for the user.
fn has_local_desktop() -> bool {
    if cfg!(target_os = "linux") && !utils::is_wsl2() {
        return std::env::var_os("DISPLAY").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some();
    }
    true
}

#[async_trait]
impl PushNotifier for DefaultPushNotifier {
    async fn send(&self, title: &str, message: &str, _workspace_id: Option<Uuid>) {
        if cfg!(target_os = "macos") {
            send_macos_notification(title, message).await;
        } else if cfg!(target_os = "linux") && !utils::is_wsl2() {
            send_linux_notification(title, message).await;
        } else if cfg!(target_os = "windows") || (cfg!(target_os = "linux") && utils::is_wsl2()) {
            send_windows_notification(title, message).await;
        }
    }
}

/// Service for handling cross-platform notifications including sound alerts and push notifications
#[derive(Clone)]
pub struct NotificationService {
    config: Arc<RwLock<Config>>,
    push_notifier: Arc<dyn PushNotifier>,
    alerts_tx: broadcast::Sender<AgentAlert>,
}

impl std::fmt::Debug for NotificationService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationService")
            .field("config", &self.config)
            .finish()
    }
}

impl NotificationService {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        let (alerts_tx, _) = broadcast::channel(ALERT_CHANNEL_CAPACITY);
        Self {
            config,
            push_notifier: get_global_push_notifier(),
            alerts_tx,
        }
    }

    /// Subscribe to live agent alerts (used by the browser alert stream).
    pub fn subscribe(&self) -> broadcast::Receiver<AgentAlert> {
        self.alerts_tx.subscribe()
    }

    /// Deliver an agent alert. Open browser tabs receive it and apply the
    /// user's sound/push settings themselves; only when no tab is listening
    /// do we fall back to OS-level notifications on this machine. Alerts for
    /// workspaces the user's settings filter out are still delivered (so the
    /// inbox refreshes) but marked silent.
    pub async fn alert(&self, mut alert: AgentAlert, workspace_pinned: bool) {
        alert.silent = !self
            .config
            .read()
            .await
            .notifications
            .should_alert_for(workspace_pinned);
        if self.alerts_tx.send(alert.clone()).is_ok() || alert.silent {
            return;
        }
        self.notify(&alert.title, &alert.body, alert.workspace_id)
            .await;
    }

    /// Send both sound and push notifications on this machine if enabled.
    /// `workspace_id` is forwarded to the push notifier so Tauri can emit a
    /// navigation event when the notification is clicked.
    pub async fn notify(&self, title: &str, message: &str, workspace_id: Option<Uuid>) {
        let config = self.config.read().await.notifications.clone();
        let custom_notifier = GLOBAL_PUSH_NOTIFIER.get().is_some();

        if !custom_notifier && !has_local_desktop() {
            tracing::debug!("No local desktop session; skipping OS notification '{title}'");
            return;
        }

        if config.sound_enabled {
            Self::play_sound_notification(&config.sound_file).await;
        }

        if config.push_enabled {
            self.push_notifier.send(title, message, workspace_id).await;
        }
    }

    /// Play a system sound notification across platforms
    async fn play_sound_notification(sound_file: &SoundFile) {
        let file_path = match sound_file.get_path().await {
            Ok(path) => path,
            Err(e) => {
                tracing::error!("Failed to create cached sound file: {}", e);
                return;
            }
        };

        // Use platform-specific sound notification
        // Note: spawn() calls are intentionally not awaited - sound notifications should be fire-and-forget
        if cfg!(target_os = "macos") {
            let _ = tokio::process::Command::new("afplay")
                .arg(&file_path)
                .spawn();
        } else if cfg!(target_os = "linux") && !utils::is_wsl2() {
            // Try different Linux audio players
            if tokio::process::Command::new("paplay")
                .arg(&file_path)
                .spawn()
                .is_ok()
            {
                // Success with paplay
            } else if tokio::process::Command::new("aplay")
                .arg(&file_path)
                .spawn()
                .is_ok()
            {
                // Success with aplay
            } else {
                // Try system bell as fallback
                let _ = tokio::process::Command::new("echo")
                    .arg("-e")
                    .arg("\\a")
                    .spawn();
            }
        } else if cfg!(target_os = "windows") || (cfg!(target_os = "linux") && utils::is_wsl2()) {
            // Convert WSL path to Windows path if in WSL2
            let file_path = if utils::is_wsl2() {
                if let Some(windows_path) = wsl_to_windows_path(&file_path).await {
                    windows_path
                } else {
                    file_path.to_string_lossy().to_string()
                }
            } else {
                file_path.to_string_lossy().to_string()
            };

            let _ = tokio::process::Command::new("powershell.exe")
                .arg("-c")
                .arg(format!(
                    r#"(New-Object Media.SoundPlayer "{file_path}").PlaySync()"#
                ))
                .no_window()
                .spawn();
        }
    }
}

// --- Platform-specific push notification helpers (used by DefaultPushNotifier) ---

/// Send macOS notification using osascript
async fn send_macos_notification(title: &str, message: &str) {
    let script = format!(
        r#"display notification "{message}" with title "{title}" sound name "Glass""#,
        message = message.replace('"', r#"\""#),
        title = title.replace('"', r#"\""#)
    );

    let _ = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn();
}

/// Send Linux notification using notify-rust
async fn send_linux_notification(title: &str, message: &str) {
    use notify_rust::Notification;

    let title = title.to_string();
    let message = message.to_string();

    let _handle = tokio::task::spawn_blocking(move || {
        match Notification::new()
            .summary(&title)
            .body(&message)
            .timeout(10000)
            .show()
        {
            Ok(_) => {}
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("ServiceUnknown")
                    || err_str.contains("org.freedesktop.Notifications")
                {
                    tracing::warn!("Linux notification daemon not available: {}", e);
                } else {
                    tracing::warn!("Failed to send Linux notification: {}", e);
                }
            }
        }
    });
    drop(_handle); // Don't await, fire-and-forget
}

/// Send Windows/WSL notification using PowerShell toast script
async fn send_windows_notification(title: &str, message: &str) {
    let script_path = match utils::get_powershell_script().await {
        Ok(path) => path,
        Err(e) => {
            tracing::error!("Failed to get PowerShell script: {}", e);
            return;
        }
    };

    // Convert WSL path to Windows path if in WSL2
    let script_path_str = if utils::is_wsl2() {
        if let Some(windows_path) = wsl_to_windows_path(&script_path).await {
            windows_path
        } else {
            script_path.to_string_lossy().to_string()
        }
    } else {
        script_path.to_string_lossy().to_string()
    };

    let _ = tokio::process::Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(script_path_str)
        .arg("-Title")
        .arg(title)
        .arg("-Message")
        .arg(message)
        .no_window()
        .spawn();
}

/// Get WSL root path via PowerShell (cached)
async fn get_wsl_root_path() -> Option<String> {
    if let Some(cached) = WSL_ROOT_PATH_CACHE.get() {
        return cached.clone();
    }

    match tokio::process::Command::new("powershell.exe")
        .arg("-c")
        .arg("(Get-Location).Path -replace '^.*::', ''")
        .current_dir("/")
        .no_window()
        .output()
        .await
    {
        Ok(output) => {
            match String::from_utf8(output.stdout) {
                Ok(pwd_str) => {
                    let pwd = pwd_str.trim();
                    tracing::info!("WSL root path detected: {}", pwd);

                    // Cache the result
                    let _ = WSL_ROOT_PATH_CACHE.set(Some(pwd.to_string()));
                    return Some(pwd.to_string());
                }
                Err(e) => {
                    tracing::error!("Failed to parse PowerShell pwd output as UTF-8: {}", e);
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to execute PowerShell pwd command: {}", e);
        }
    }

    // Cache the failure result
    let _ = WSL_ROOT_PATH_CACHE.set(None);
    None
}

/// Convert WSL path to Windows UNC path for PowerShell
async fn wsl_to_windows_path(wsl_path: &std::path::Path) -> Option<String> {
    let path_str = wsl_path.to_string_lossy();

    // Relative paths work fine as-is in PowerShell
    if !path_str.starts_with('/') {
        tracing::debug!("Using relative path as-is: {}", path_str);
        return Some(path_str.to_string());
    }

    // Get cached WSL root path from PowerShell
    if let Some(wsl_root) = get_wsl_root_path().await {
        // Simply concatenate WSL root with the absolute path - PowerShell doesn't mind /
        let windows_path = format!("{wsl_root}{path_str}");
        tracing::debug!("WSL path converted: {} -> {}", path_str, windows_path);
        Some(windows_path)
    } else {
        tracing::error!(
            "Failed to determine WSL root path for conversion: {}",
            path_str
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_alert() -> AgentAlert {
        AgentAlert {
            id: AgentAlert::run_id(Uuid::nil()),
            kind: AgentAlertKind::Finished,
            title: "Agent finished".to_string(),
            body: "Northwind Traders".to_string(),
            workspace_id: Some(Uuid::nil()),
            created_at: Utc::now(),
            silent: false,
        }
    }

    fn service_with_filter(alert_only_pinned_workspaces: bool) -> NotificationService {
        let mut config = Config::default();
        config.notifications.alert_only_pinned_workspaces = alert_only_pinned_workspaces;
        NotificationService::new(Arc::new(RwLock::new(config)))
    }

    async fn delivered_silent(alert_only_pinned_workspaces: bool, workspace_pinned: bool) -> bool {
        let service = service_with_filter(alert_only_pinned_workspaces);
        let mut rx = service.subscribe();
        service.alert(sample_alert(), workspace_pinned).await;
        rx.recv().await.expect("alert delivered").silent
    }

    #[tokio::test]
    async fn alert_reaches_every_subscriber() {
        let service = NotificationService::new(Arc::new(RwLock::new(Config::default())));
        let mut first = service.subscribe();
        let mut second = service.subscribe();

        service.alert(sample_alert(), true).await;

        for rx in [&mut first, &mut second] {
            let alert = rx.recv().await.expect("alert delivered");
            assert_eq!(alert.id, format!("run:{}", Uuid::nil()));
            assert_eq!(alert.kind, AgentAlertKind::Finished);
        }
    }

    #[tokio::test]
    async fn pinned_workspace_alert_is_audible_when_only_pinned() {
        assert!(!delivered_silent(true, true).await);
    }

    #[tokio::test]
    async fn unpinned_workspace_alert_is_silent_when_only_pinned() {
        assert!(delivered_silent(true, false).await);
    }

    #[tokio::test]
    async fn every_alert_is_audible_when_filter_off() {
        assert!(!delivered_silent(false, true).await);
        assert!(!delivered_silent(false, false).await);
    }

    #[tokio::test]
    async fn alert_without_subscribers_does_not_panic() {
        let mut config = Config::default();
        config.notifications.sound_enabled = false;
        config.notifications.push_enabled = false;
        let service = NotificationService::new(Arc::new(RwLock::new(config)));
        service.alert(sample_alert(), true).await;
        service.alert(sample_alert(), false).await;
    }

    #[test]
    fn alert_ids_are_prefixed_by_source() {
        let id = Uuid::nil();
        assert_eq!(AgentAlert::run_id(id), format!("run:{id}"));
        assert_eq!(AgentAlert::merge_id(id), format!("merge:{id}"));
        assert_eq!(AgentAlert::approval_id("abc"), "approval:abc");
    }

    #[test]
    fn alert_kind_serializes_snake_case() {
        let json = serde_json::to_string(&AgentAlertKind::NeedsApproval).unwrap();
        assert_eq!(json, "\"needs_approval\"");
    }
}
