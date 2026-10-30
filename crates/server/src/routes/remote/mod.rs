//! Issue-tracking API used by the MCP task server and the local web app,
//! served from the local kanban in SQLite.

use axum::Router;
use services::services::local_kanban::LocalKanbanError;

use crate::{DeploymentImpl, error::ApiError};

mod issue_assignees;
mod issue_relationships;
mod issue_tags;
mod issues;
mod project_statuses;
mod projects;
pub mod pull_requests;
mod tags;
mod workspaces;

impl From<LocalKanbanError> for ApiError {
    fn from(err: LocalKanbanError) -> Self {
        match err {
            LocalKanbanError::Database(err) => ApiError::Database(err),
            LocalKanbanError::NotFound(_) | LocalKanbanError::Invalid(_) => {
                ApiError::BadRequest(err.to_string())
            }
        }
    }
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .merge(issue_assignees::router())
        .merge(issue_relationships::router())
        .merge(issue_tags::router())
        .merge(issues::router())
        .merge(projects::router())
        .merge(project_statuses::router())
        .merge(pull_requests::router())
        .merge(tags::router())
        .merge(workspaces::router())
}
