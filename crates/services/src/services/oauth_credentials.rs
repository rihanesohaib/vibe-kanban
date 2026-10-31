use std::path::PathBuf;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// OAuth credentials containing the JWT tokens issued by the remote OAuth service.
/// The `access_token` is short-lived; `refresh_token` allows minting a new pair.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub access_token: Option<String>,
    pub refresh_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "<redacted>"),
            )
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Credentials {
    pub fn expires_soon(&self, leeway: ChronoDuration) -> bool {
        match (self.access_token.as_ref(), self.expires_at.as_ref()) {
            (Some(_), Some(exp)) => Utc::now() + leeway >= *exp,
            _ => true,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredCredentials {
    refresh_token: String,
}

impl From<StoredCredentials> for Credentials {
    fn from(value: StoredCredentials) -> Self {
        Self {
            access_token: None,
            refresh_token: value.refresh_token,
            expires_at: None,
        }
    }
}

/// Service for managing OAuth credentials (JWT tokens) in memory and persistent storage.
/// The token is loaded into memory on startup and persisted to disk on save.
pub struct OAuthCredentials {
    path: PathBuf,
    inner: RwLock<Option<Credentials>>,
}

impl OAuthCredentials {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            inner: RwLock::new(None),
        }
    }

    pub async fn load(&self) -> std::io::Result<()> {
        let creds = self.load_from_file().await?.map(Credentials::from);
        *self.inner.write().await = creds;
        Ok(())
    }

    pub async fn save(&self, creds: &Credentials) -> std::io::Result<()> {
        let stored = StoredCredentials {
            refresh_token: creds.refresh_token.clone(),
        };
        self.save_to_file(&stored).await?;
        *self.inner.write().await = Some(creds.clone());
        Ok(())
    }

    pub async fn clear(&self) -> std::io::Result<()> {
        let _ = std::fs::remove_file(&self.path);
        *self.inner.write().await = None;
        Ok(())
    }

    pub async fn get(&self) -> Option<Credentials> {
        self.inner.read().await.clone()
    }

    async fn load_from_file(&self) -> std::io::Result<Option<StoredCredentials>> {
        if !self.path.exists() {
            return Ok(None);
        }

        let bytes = std::fs::read(&self.path)?;
        match serde_json::from_slice::<StoredCredentials>(&bytes) {
            Ok(creds) => Ok(Some(creds)),
            Err(e) => {
                tracing::warn!(?e, "failed to parse credentials file, renaming to .bad");
                let bad = self.path.with_extension("bad");
                let _ = std::fs::rename(&self.path, bad);
                Ok(None)
            }
        }
    }

    async fn save_to_file(&self, creds: &StoredCredentials) -> std::io::Result<()> {
        let raw = serde_json::to_vec_pretty(creds)?;
        utils::private_fs::write_private_atomic(&self.path, &raw)
    }
}
