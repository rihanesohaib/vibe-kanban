use api_types::{ListProjectsResponse, Project};
use axum::{
    Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::get,
};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError, routes::kanban::service};

pub(super) fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/projects", get(list_remote_projects))
        .route("/projects/{project_id}", get(get_remote_project))
}

/// Every local project belongs to the one local organization, so the
/// `organization_id` query parameter is accepted and ignored.
async fn list_remote_projects(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ListProjectsResponse>>, ApiError> {
    let projects = service(&deployment).list_projects().await?;
    Ok(ResponseJson(ApiResponse::success(ListProjectsResponse {
        projects,
    })))
}

async fn get_remote_project(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<Project>>, ApiError> {
    let project = service(&deployment).get_project(project_id).await?;
    Ok(ResponseJson(ApiResponse::success(project)))
}
