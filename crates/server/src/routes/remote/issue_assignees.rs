use api_types::{
    CreateIssueAssigneeRequest, IssueAssignee, ListIssueAssigneesResponse, MutationResponse,
};
use axum::{
    Router,
    extract::{Json, Path},
    response::Json as ResponseJson,
    routing::get,
};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

const NOT_SUPPORTED: &str = "Assignees are not used by the single-user local kanban";

pub(super) fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/issue-assignees",
            get(list_issue_assignees).post(create_issue_assignee),
        )
        .route(
            "/issue-assignees/{issue_assignee_id}",
            get(get_issue_assignee).delete(delete_issue_assignee),
        )
}

async fn list_issue_assignees()
-> Result<ResponseJson<ApiResponse<ListIssueAssigneesResponse>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        ListIssueAssigneesResponse {
            issue_assignees: Vec::new(),
        },
    )))
}

async fn get_issue_assignee(
    Path(_issue_assignee_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<IssueAssignee>>, ApiError> {
    Err(ApiError::BadRequest(NOT_SUPPORTED.to_string()))
}

async fn create_issue_assignee(
    Json(_request): Json<CreateIssueAssigneeRequest>,
) -> Result<ResponseJson<ApiResponse<MutationResponse<IssueAssignee>>>, ApiError> {
    Err(ApiError::BadRequest(NOT_SUPPORTED.to_string()))
}

async fn delete_issue_assignee(
    Path(_issue_assignee_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    Err(ApiError::BadRequest(NOT_SUPPORTED.to_string()))
}
