pub mod host_stats;
pub mod overview;

use axum::{Router, routing::get};

use crate::DeploymentImpl;

pub fn router() -> Router<DeploymentImpl> {
    Router::new().nest(
        "/control-center",
        Router::new()
            .route("/host", get(host_stats::get_host_stats))
            .route("/overview", get(overview::get_overview)),
    )
}
