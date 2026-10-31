use axum::{
    extract::{MatchedPath, OriginalUri, Request},
    middleware::Next,
    response::Response,
};

pub async fn log_server_errors(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    // Path only: query strings can carry one-time codes or tokens.
    let path = request
        .extensions()
        .get::<OriginalUri>()
        .map(|original| original.0.path().to_owned())
        .unwrap_or_else(|| request.uri().path().to_owned());
    let matched_path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned());

    let response = next.run(request).await;

    if response.status().is_server_error() {
        tracing::error!(
            method = %method,
            path = %path,
            matched_path = matched_path.as_deref().unwrap_or("<unmatched>"),
            status = %response.status(),
            "API request returned server error"
        );
    }

    response
}
