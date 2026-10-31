use axum::{
    Router,
    routing::{IntoMakeService, get},
};
use tower_http::compression::CompressionLayer;

use crate::{DeploymentImpl, middleware, middleware::ApiToken};

pub mod approvals;
pub mod config;
pub mod containers;
pub mod control_center;
pub mod filesystem;
// pub mod github;
pub mod attachments;
pub mod events;
pub mod execution_processes;
pub mod frontend;
pub mod health;
pub mod kanban;
pub mod notifications;
pub mod oauth;
pub mod organizations;
pub mod preview;
pub mod remote;
pub mod repo;
pub mod scratch;
pub mod search;
pub mod sessions;
pub mod tags;
pub mod terminal;
pub mod workspaces;

pub fn router(deployment: DeploymentImpl, api_token: ApiToken) -> IntoMakeService<Router> {
    let protected_api = Router::new()
        .merge(config::router())
        .merge(containers::router(&deployment))
        .merge(control_center::router())
        .merge(workspaces::router(&deployment))
        .merge(execution_processes::router(&deployment))
        .merge(tags::router(&deployment))
        .merge(oauth::router())
        .merge(organizations::router())
        .nest("/kanban", kanban::router())
        .merge(filesystem::router())
        .merge(repo::router())
        .merge(events::router(&deployment))
        .merge(approvals::router())
        .merge(notifications::router())
        .merge(scratch::router(&deployment))
        .merge(search::router(&deployment))
        .merge(preview::api_router())
        .merge(sessions::router(&deployment))
        .merge(terminal::router())
        .nest("/remote", remote::router())
        .nest("/attachments", attachments::routes())
        .layer(axum::middleware::from_fn(middleware::log_server_errors))
        .with_state(deployment);

    let public_api = Router::new().route("/health", get(health::health_check));

    let frontend = Router::new()
        .route("/", get(frontend::serve_frontend_root))
        .route("/{*path}", get(frontend::serve_frontend));

    middleware::secure_app(protected_api, public_api, frontend, api_token)
        .layer(CompressionLayer::new())
        .into_make_service()
}
