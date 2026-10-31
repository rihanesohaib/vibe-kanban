use std::{fmt, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use subtle::ConstantTimeEq;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use utils::api_token::{api_token_path, generate_api_token, write_api_token_file};

use super::origin::{validate_host, validate_origin, validate_preview_proxy_host};

const COOKIE_PREFIX: &str = "vk_api_token";

/// Per-launch secret required on every `/api` request.
///
/// The cookie name carries the listening port because browsers share cookies
/// across ports on the same host, so two instances on `localhost` would
/// otherwise overwrite each other's cookie.
#[derive(Clone)]
pub struct ApiToken(Arc<ApiTokenInner>);

struct ApiTokenInner {
    secret: String,
    cookie_name: String,
}

impl ApiToken {
    pub fn new(secret: String, port: u16) -> Self {
        Self(Arc::new(ApiTokenInner {
            secret,
            cookie_name: format!("{COOKIE_PREFIX}_{port}"),
        }))
    }

    /// Generate a fresh token and write it to the `api-token` file in the
    /// asset directory so local clients can authenticate.
    pub fn generate_and_persist(port: u16) -> std::io::Result<Self> {
        let token = Self::new(generate_api_token(), port);
        let path = api_token_path();
        write_api_token_file(&path, &token.0.secret)?;
        tracing::info!("API token written to {}", path.display());
        Ok(token)
    }

    pub fn cookie_name(&self) -> &str {
        &self.0.cookie_name
    }

    fn matches(&self, candidate: &str) -> bool {
        candidate.as_bytes().ct_eq(self.0.secret.as_bytes()).into()
    }

    fn set_cookie_value(&self) -> Option<HeaderValue> {
        HeaderValue::from_str(&format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict",
            self.0.cookie_name, self.0.secret
        ))
        .ok()
    }

    /// Check the bearer header and cookie, then remove our credential from
    /// the headers so handlers (e.g. the preview proxy) never forward it.
    fn authenticate_and_strip(&self, headers: &mut HeaderMap) -> bool {
        let mut authenticated = false;

        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim());
        if bearer.is_some_and(|token| self.matches(token)) {
            authenticated = true;
            headers.remove(header::AUTHORIZATION);
        }

        if headers.contains_key(header::COOKIE) {
            let mut kept = Vec::new();
            for value in headers.get_all(header::COOKIE) {
                let Ok(value) = value.to_str() else {
                    continue;
                };
                for pair in value.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                    match pair.split_once('=') {
                        Some((name, token)) if name.trim() == self.0.cookie_name => {
                            authenticated |= self.matches(token.trim());
                        }
                        _ => kept.push(pair),
                    }
                }
            }
            let kept = kept.join("; ");
            headers.remove(header::COOKIE);
            if !kept.is_empty()
                && let Ok(value) = HeaderValue::from_str(&kept)
            {
                headers.insert(header::COOKIE, value);
            }
        }

        authenticated
    }
}

impl fmt::Debug for ApiToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiToken")
            .field("cookie_name", &self.0.cookie_name)
            .finish_non_exhaustive()
    }
}

pub async fn require_api_token(
    State(token): State<ApiToken>,
    mut req: Request,
    next: Next,
) -> Response {
    if !token.authenticate_and_strip(req.headers_mut()) {
        return unauthorized();
    }
    next.run(req).await
}

/// Hand the token to the browser UI as an HttpOnly cookie whenever the SPA
/// HTML is served.
pub async fn set_api_token_cookie(
    State(token): State<ApiToken>,
    req: Request,
    next: Next,
) -> Response {
    let mut response = next.run(req).await;
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if is_html && let Some(cookie) = token.set_cookie_value() {
        let headers = response.headers_mut();
        headers.append(header::SET_COOKIE, cookie);
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn unauthorized() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, "Bearer")
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Assemble the main listener's router with all access controls:
/// - Host allowlist on every route (SPA, API, WebSockets);
/// - Origin check on `/api`;
/// - API token on every `/api` route in `protected_api`, including upgrades;
/// - `public_api` routes (health) are mounted under `/api` without the token;
/// - the token cookie on SPA HTML responses from `frontend`.
pub fn secure_app(
    protected_api: Router,
    public_api: Router,
    frontend: Router,
    token: ApiToken,
) -> Router {
    // Explicit fallback so unknown /api paths sit behind the token layer too;
    // a default fallback would be replaced by the un-layered one on merge.
    let api = protected_api
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(from_fn_with_state(token.clone(), require_api_token))
        .merge(public_api)
        .layer(ValidateRequestHeaderLayer::custom(validate_origin));

    frontend
        .layer(from_fn_with_state(token, set_api_token_cookie))
        .nest("/api", api)
        .layer(ValidateRequestHeaderLayer::custom(validate_host))
}

/// Access controls for the preview proxy listener. It only relays to loopback
/// dev-server ports and never carries the API token, so it gets the Host and
/// Origin checks but no token requirement.
pub fn secure_preview_proxy(router: Router) -> Router {
    router
        .layer(ValidateRequestHeaderLayer::custom(validate_origin))
        .layer(ValidateRequestHeaderLayer::custom(
            validate_preview_proxy_host,
        ))
}

#[cfg(test)]
mod tests {
    use axum::{
        extract::ws::WebSocketUpgrade,
        http::{Method, Request},
        response::{Html, IntoResponse},
        routing::{any, get},
    };
    use tower::ServiceExt;

    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const PORT: u16 = 3005;

    async fn echo_credentials(headers: HeaderMap) -> String {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-");
        let cookie = headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-");
        format!("auth={auth} cookie={cookie}")
    }

    async fn ws_handler(ws: WebSocketUpgrade) -> Response {
        ws.on_upgrade(|_socket| async {}).into_response()
    }

    fn app() -> Router {
        let protected_api = Router::new()
            .route("/info", get(|| async { "info" }))
            .route("/echo", get(echo_credentials))
            .route("/terminal/ws", any(ws_handler));
        let public_api = Router::new().route("/health", get(|| async { r#"{"ok":true}"# }));
        let frontend = Router::new()
            .route("/", get(|| async { Html("<html></html>") }))
            .route("/assets/app.js", get(|| async { "js" }));
        secure_app(
            protected_api,
            public_api,
            frontend,
            ApiToken::new(SECRET.to_string(), PORT),
        )
    }

    fn request(path: &str) -> axum::http::request::Builder {
        Request::builder()
            .uri(path)
            .method(Method::GET)
            .header(header::HOST, "localhost:3005")
    }

    async fn send(req: axum::http::request::Builder) -> Response {
        app()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn cookie(value: &str) -> String {
        format!("vk_api_token_{PORT}={value}")
    }

    #[tokio::test]
    async fn missing_token_is_unauthorized_with_empty_body() {
        let response = send(request("/api/info")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(body_string(response).await.is_empty());
    }

    #[tokio::test]
    async fn unknown_api_route_without_token_is_unauthorized() {
        let response = send(request("/api/does-not-exist")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = send(
            request("/api/does-not-exist")
                .header(header::AUTHORIZATION, format!("Bearer {SECRET}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn wrong_token_is_unauthorized() {
        for req in [
            request("/api/info").header(header::AUTHORIZATION, "Bearer wrong"),
            request("/api/info").header(header::AUTHORIZATION, format!("Bearer {SECRET}x")),
            request("/api/info").header(header::AUTHORIZATION, format!("Basic {SECRET}")),
            request("/api/info").header(header::AUTHORIZATION, SECRET),
            request("/api/info").header(header::COOKIE, cookie("wrong")),
            request("/api/info").header(header::COOKIE, format!("vk_api_token_1={SECRET}")),
        ] {
            assert_eq!(send(req).await.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn correct_bearer_header_passes() {
        for value in [format!("Bearer {SECRET}"), format!("bearer  {SECRET} ")] {
            let response = send(request("/api/info").header(header::AUTHORIZATION, value)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(body_string(response).await, "info");
        }
    }

    #[tokio::test]
    async fn correct_cookie_passes() {
        let response = send(
            request("/api/info").header(header::COOKIE, format!("theme=dark; {}", cookie(SECRET))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn credentials_are_stripped_before_reaching_handlers() {
        let response = send(
            request("/api/echo")
                .header(header::AUTHORIZATION, format!("Bearer {SECRET}"))
                .header(header::COOKIE, format!("a=1; {}; b=2", cookie(SECRET))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, "auth=- cookie=a=1; b=2");

        let response = send(request("/api/echo").header(header::COOKIE, cookie(SECRET))).await;
        assert_eq!(body_string(response).await, "auth=- cookie=-");
    }

    #[tokio::test]
    async fn health_passes_without_token() {
        let response = send(request("/api/health")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, r#"{"ok":true}"#);
    }

    #[tokio::test]
    async fn evil_host_is_forbidden_everywhere() {
        for path in ["/", "/assets/app.js", "/api/health", "/api/info"] {
            let response = send(
                Request::builder()
                    .uri(path)
                    .header(header::HOST, "evil.example")
                    .header(header::AUTHORIZATION, format!("Bearer {SECRET}")),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        }
    }

    #[tokio::test]
    async fn rebinding_request_is_forbidden() {
        for path in ["/", "/api/health", "/api/info"] {
            let response = send(
                Request::builder()
                    .uri(path)
                    .header(header::HOST, "attacker.example:3005")
                    .header(header::ORIGIN, "http://attacker.example:3005")
                    .header(header::COOKIE, cookie(SECRET)),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        }
    }

    #[tokio::test]
    async fn cross_origin_api_request_is_forbidden_even_with_token() {
        let response = send(
            request("/api/info")
                .header(header::ORIGIN, "http://evil.example")
                .header(header::AUTHORIZATION, format!("Bearer {SECRET}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    fn ws_upgrade(path: &str) -> axum::http::request::Builder {
        request(path)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
            .header(header::ORIGIN, "http://localhost:3005")
    }

    #[tokio::test]
    async fn websocket_upgrade_without_token_is_rejected() {
        let response = send(ws_upgrade("/api/terminal/ws")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn websocket_upgrade_with_cookie_reaches_handler() {
        let response = app()
            .oneshot(
                ws_upgrade("/api/terminal/ws")
                    .header(header::COOKIE, cookie(SECRET))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // A oneshot request has no real connection to upgrade, so the
        // extractor rejects it; reaching it at all means auth passed.
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
        assert_ne!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn spa_html_sets_strict_http_only_cookie() {
        let response = send(request("/")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap()
            .to_string();
        assert_eq!(
            set_cookie,
            format!("{}; Path=/; HttpOnly; SameSite=Strict", cookie(SECRET))
        );
        assert!(!set_cookie.contains("Secure"));
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let asset = send(request("/assets/app.js")).await;
        assert!(asset.headers().get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn token_debug_output_is_redacted() {
        let token = ApiToken::new(SECRET.to_string(), PORT);
        assert!(!format!("{token:?}").contains(SECRET));
    }

    #[tokio::test]
    async fn preview_proxy_rejects_foreign_hosts() {
        let proxy = secure_preview_proxy(Router::new().fallback(|| async { "proxied" }));
        let ok = proxy
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "3000.localhost:4000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);

        let evil = proxy
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "3005.attacker.example:4000")
                    .header(header::ORIGIN, "http://3005.attacker.example:4000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(evil.status(), StatusCode::FORBIDDEN);
    }
}
