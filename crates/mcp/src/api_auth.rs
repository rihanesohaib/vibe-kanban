use std::{
    fmt,
    sync::{Arc, RwLock},
};

use reqwest::{RequestBuilder, Response, StatusCode};

type TokenLoader = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Bearer token for the VK API.
///
/// The server rotates its token on every restart while MCP processes keep
/// running, so a 401 triggers one re-read of the token and a single retry.
#[derive(Clone)]
pub struct ApiAuth {
    token: Arc<RwLock<Option<String>>>,
    loader: TokenLoader,
}

impl ApiAuth {
    /// `VK_API_TOKEN` if set, otherwise the server's `api-token` file.
    pub fn from_env_or_file() -> Self {
        Self::with_loader(utils::api_token::read_api_token)
    }

    pub fn with_loader<F>(loader: F) -> Self
    where
        F: Fn() -> Option<String> + Send + Sync + 'static,
    {
        let token = loader();
        if token.is_none() {
            tracing::warn!(
                "No VK API token found (set {} or start the server first)",
                utils::api_token::API_TOKEN_ENV
            );
        }
        Self {
            token: Arc::new(RwLock::new(token)),
            loader: Arc::new(loader),
        }
    }

    pub async fn send(&self, request: RequestBuilder) -> reqwest::Result<Response> {
        let retry = request.try_clone();
        let response = self.authorize(request).send().await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(retry) = retry else {
            return Ok(response);
        };
        if !self.reload() {
            return Ok(response);
        }
        tracing::info!("VK API rejected the token; retrying with the re-read token");
        self.authorize(retry).send().await
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        match self.current() {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    fn current(&self) -> Option<String> {
        self.token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Re-read the token. Returns whether it changed.
    fn reload(&self) -> bool {
        let fresh = (self.loader)();
        let mut guard = self
            .token
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if fresh.is_none() || *guard == fresh {
            return false;
        }
        *guard = fresh;
        true
    }
}

impl fmt::Debug for ApiAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiAuth")
            .field("has_token", &self.current().is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Router, http::HeaderMap, routing::post};

    use super::*;
    use crate::install_rustls_provider_for_tests as install_rustls_provider;

    /// Server that only accepts `Bearer current`, counting requests.
    async fn spawn_server(hits: Arc<AtomicUsize>) -> String {
        let app = Router::new().route(
            "/api/thing",
            post(move |headers: HeaderMap, body: String| {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let authorized = headers
                        .get("authorization")
                        .is_some_and(|v| v == "Bearer current");
                    if authorized {
                        (axum::http::StatusCode::OK, body)
                    } else {
                        (axum::http::StatusCode::UNAUTHORIZED, String::new())
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/api/thing")
    }

    fn file_backed(initial: &str) -> (ApiAuth, Arc<Mutex<String>>) {
        let file = Arc::new(Mutex::new(initial.to_string()));
        let source = file.clone();
        let auth = ApiAuth::with_loader(move || Some(source.lock().unwrap().clone()));
        (auth, file)
    }

    #[tokio::test]
    async fn sends_bearer_token() {
        install_rustls_provider();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = spawn_server(hits.clone()).await;
        let (auth, _file) = file_backed("current");

        let response = auth
            .send(reqwest::Client::new().post(&url).body("hello"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "hello");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rereads_rotated_token_once_on_401_and_replays_body() {
        install_rustls_provider();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = spawn_server(hits.clone()).await;
        let (auth, file) = file_backed("stale");

        *file.lock().unwrap() = "current".to_string();
        let response = auth
            .send(reqwest::Client::new().post(&url).body("payload"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "payload");
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        let response = auth
            .send(reqwest::Client::new().post(&url).body("again"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn unchanged_token_returns_401_without_retrying() {
        install_rustls_provider();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = spawn_server(hits.clone()).await;
        let (auth, _file) = file_backed("stale");

        let response = auth
            .send(reqwest::Client::new().post(&url).body("x"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn debug_output_does_not_leak_token() {
        let (auth, _file) = file_backed("current");
        assert!(!format!("{auth:?}").contains("current"));
    }
}
