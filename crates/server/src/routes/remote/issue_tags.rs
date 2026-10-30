use api_types::{CreateIssueTagRequest, IssueTag, ListIssueTagsResponse, MutationResponse};
use axum::{
    Router,
    extract::{Json, Path, Query, State},
    response::Json as ResponseJson,
    routing::get,
};
use serde::Deserialize;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError, routes::kanban::service};

#[derive(Debug, Deserialize)]
pub(super) struct ListIssueTagsQuery {
    pub issue_id: Uuid,
}

pub(super) fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/issue-tags", get(list_issue_tags).post(create_issue_tag))
        .route(
            "/issue-tags/{issue_tag_id}",
            get(get_issue_tag).delete(delete_issue_tag),
        )
}

async fn list_issue_tags(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ListIssueTagsQuery>,
) -> Result<ResponseJson<ApiResponse<ListIssueTagsResponse>>, ApiError> {
    let issue_tags = service(&deployment).list_issue_tags(query.issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(ListIssueTagsResponse {
        issue_tags,
    })))
}

async fn get_issue_tag(
    State(deployment): State<DeploymentImpl>,
    Path(issue_tag_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<IssueTag>>, ApiError> {
    let issue_tag = service(&deployment).get_issue_tag(issue_tag_id).await?;
    Ok(ResponseJson(ApiResponse::success(issue_tag)))
}

async fn create_issue_tag(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateIssueTagRequest>,
) -> Result<ResponseJson<ApiResponse<MutationResponse<IssueTag>>>, ApiError> {
    let data = service(&deployment).create_issue_tag(request).await?;
    Ok(ResponseJson(ApiResponse::success(MutationResponse {
        data,
        txid: 0,
    })))
}

async fn delete_issue_tag(
    State(deployment): State<DeploymentImpl>,
    Path(issue_tag_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    service(&deployment).delete_issue_tag(issue_tag_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}
