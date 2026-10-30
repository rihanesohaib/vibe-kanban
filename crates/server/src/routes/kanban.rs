//! Local kanban API for the web app. Speaks the same REST contract as the
//! retired cloud API (fallback reads under `/v1/fallback/<table>`, mutations
//! under `/v1/<table>`, raw JSON bodies) so the existing kanban UI and its
//! collections work unchanged against SQLite.

use api_types::{
    CreateIssueCommentRequest, CreateIssueRelationshipRequest, CreateIssueRequest,
    CreateIssueTagRequest, CreateProjectRequest, CreateProjectStatusRequest, CreateTagRequest,
    DeleteResponse, ListMembersResponse, ListOrganizationsResponse, MutationResponse,
    UpdateIssueCommentRequest, UpdateIssueRequest, UpdateProjectRequest,
    UpdateProjectStatusRequest, UpdateTagRequest,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use deployment::Deployment;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use services::services::local_kanban::{
    self, LocalKanbanError, LocalKanbanService, local_member, local_organization, local_user,
};
use uuid::Uuid;

use crate::DeploymentImpl;

pub(crate) fn service(deployment: &DeploymentImpl) -> LocalKanbanService {
    LocalKanbanService::new(deployment.db().pool.clone())
}

/// Tables the single-user kanban does not store. Reads return no rows and
/// writes are accepted without effect so shared UI code never errors.
const UNSUPPORTED_TABLES: &[&str] = &[
    "notifications",
    "issue_assignees",
    "issue_followers",
    "issue_comment_reactions",
    "pull_request_issues",
];

pub(crate) struct KanbanError(LocalKanbanError);

impl From<LocalKanbanError> for KanbanError {
    fn from(err: LocalKanbanError) -> Self {
        Self(err)
    }
}

impl IntoResponse for KanbanError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            LocalKanbanError::NotFound(_) => (StatusCode::NOT_FOUND, self.0.to_string()),
            LocalKanbanError::Invalid(_) => (StatusCode::BAD_REQUEST, self.0.to_string()),
            LocalKanbanError::Database(err) => {
                tracing::error!("Local kanban database error: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
        };
        (
            status,
            Json(json!({ "error": message, "message": message })),
        )
            .into_response()
    }
}

type KanbanResult<T> = Result<Json<T>, KanbanError>;

fn mutation<T>(data: T) -> Json<MutationResponse<T>> {
    Json(MutationResponse { data, txid: 0 })
}

fn deleted() -> Json<DeleteResponse> {
    Json(DeleteResponse { txid: 0 })
}

fn rows<T: Serialize>(table: &str, rows: Vec<T>) -> Json<Value> {
    let mut body = serde_json::Map::new();
    body.insert(table.to_string(), json!(rows));
    Json(Value::Object(body))
}

#[derive(Debug, Deserialize)]
struct ProjectQuery {
    project_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct IssueQuery {
    issue_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(bound = "T: DeserializeOwned")]
struct BulkRequest<T> {
    updates: Vec<BulkItem<T>>,
}

#[derive(Debug, Deserialize)]
#[serde(bound = "T: DeserializeOwned")]
struct BulkItem<T> {
    id: Uuid,
    #[serde(flatten)]
    changes: T,
}

impl<T> BulkRequest<T> {
    fn into_pairs(self) -> Vec<(Uuid, T)> {
        self.updates
            .into_iter()
            .map(|item| (item.id, item.changes))
            .collect()
    }
}

pub fn router() -> Router<DeploymentImpl> {
    let mut router = Router::new()
        .route("/v1/changes", get(changes))
        .route("/v1/organizations", get(list_organizations))
        .route("/v1/organizations/{id}/members", get(list_members))
        .route("/v1/fallback/projects", get(fallback_projects))
        .route("/v1/fallback/project_statuses", get(fallback_statuses))
        .route("/v1/fallback/issues", get(fallback_issues))
        .route("/v1/fallback/tags", get(fallback_tags))
        .route("/v1/fallback/issue_tags", get(fallback_issue_tags))
        .route(
            "/v1/fallback/issue_relationships",
            get(fallback_relationships),
        )
        .route("/v1/fallback/issue_comments", get(fallback_comments))
        .route(
            "/v1/fallback/project_workspaces",
            get(fallback_project_workspaces),
        )
        .route(
            "/v1/fallback/user_workspaces",
            get(fallback_user_workspaces),
        )
        .route("/v1/fallback/pull_requests", get(fallback_pull_requests))
        .route(
            "/v1/fallback/pull_request_issues",
            get(fallback_pull_request_issues),
        )
        .route("/v1/fallback/users", get(fallback_users))
        .route("/v1/fallback/organization_members", get(fallback_members))
        .route("/v1/projects", post(create_project))
        .route("/v1/projects/bulk", post(bulk_update_projects))
        .route(
            "/v1/projects/{id}",
            patch(update_project).delete(delete_project),
        )
        .route("/v1/project_statuses", post(create_status))
        .route("/v1/project_statuses/bulk", post(bulk_update_statuses))
        .route(
            "/v1/project_statuses/{id}",
            patch(update_status).delete(delete_status),
        )
        .route("/v1/issues", post(create_issue))
        .route("/v1/issues/bulk", post(bulk_update_issues))
        .route("/v1/issues/{id}", patch(update_issue).delete(delete_issue))
        .route("/v1/tags", post(create_tag))
        .route("/v1/tags/{id}", patch(update_tag).delete(delete_tag))
        .route("/v1/issue_tags", post(create_issue_tag))
        .route(
            "/v1/issue_tags/{id}",
            axum::routing::delete(delete_issue_tag),
        )
        .route("/v1/issue_relationships", post(create_relationship))
        .route(
            "/v1/issue_relationships/{id}",
            axum::routing::delete(delete_relationship),
        )
        .route("/v1/issue_comments", post(create_comment))
        .route(
            "/v1/issue_comments/{id}",
            patch(update_comment).delete(delete_comment),
        );

    for table in UNSUPPORTED_TABLES {
        if *table != "pull_request_issues" {
            router = router.route(
                &format!("/v1/fallback/{table}"),
                get(move || async move { rows::<Value>(table, Vec::new()) }),
            );
        }
        router = router
            .route(&format!("/v1/{table}"), post(accept_ignored))
            .route(
                &format!("/v1/{table}/{{id}}"),
                patch(accept_ignored).delete(|| async { deleted() }),
            );
    }

    router
}

async fn accept_ignored(Json(body): Json<Value>) -> Json<MutationResponse<Value>> {
    mutation(body)
}

async fn changes() -> Json<Value> {
    Json(json!({ "version": local_kanban::change_version() }))
}

async fn list_organizations() -> Json<ListOrganizationsResponse> {
    Json(ListOrganizationsResponse {
        organizations: vec![local_organization()],
    })
}

async fn list_members() -> Json<ListMembersResponse> {
    Json(ListMembersResponse {
        members: vec![local_member()],
    })
}

async fn fallback_users() -> Json<Value> {
    rows("users", vec![local_user()])
}

async fn fallback_members() -> Json<Value> {
    let member = local_member();
    rows(
        "organization_member_metadata",
        vec![json!({
            "organization_id": local_kanban::LOCAL_ORGANIZATION_ID,
            "user_id": member.user_id,
            "role": member.role,
            "joined_at": member.joined_at,
            "last_seen_at": null,
        })],
    )
}

async fn fallback_projects(
    State(deployment): State<DeploymentImpl>,
) -> Result<Json<Value>, KanbanError> {
    Ok(rows(
        "projects",
        service(&deployment).list_projects().await?,
    ))
}

async fn fallback_statuses(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    let statuses = service(&deployment).list_statuses(query.project_id).await?;
    Ok(rows("project_statuses", statuses))
}

async fn fallback_issues(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    let issues = service(&deployment).list_issues(query.project_id).await?;
    Ok(rows("issues", issues))
}

async fn fallback_tags(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    Ok(rows(
        "tags",
        service(&deployment).list_tags(query.project_id).await?,
    ))
}

async fn fallback_issue_tags(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    let issue_tags = service(&deployment)
        .list_issue_tags_for_project(query.project_id)
        .await?;
    Ok(rows("issue_tags", issue_tags))
}

async fn fallback_relationships(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    let relationships = service(&deployment)
        .list_relationships_for_project(query.project_id)
        .await?;
    Ok(rows("issue_relationships", relationships))
}

async fn fallback_comments(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<IssueQuery>,
) -> Result<Json<Value>, KanbanError> {
    let comments = service(&deployment).list_comments(query.issue_id).await?;
    Ok(rows("issue_comments", comments))
}

async fn fallback_project_workspaces(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    let workspaces = service(&deployment)
        .list_workspaces(Some(query.project_id))
        .await?;
    Ok(rows("workspaces", workspaces))
}

async fn fallback_user_workspaces(
    State(deployment): State<DeploymentImpl>,
) -> Result<Json<Value>, KanbanError> {
    let workspaces = service(&deployment).list_workspaces(None).await?;
    Ok(rows("workspaces", workspaces))
}

async fn fallback_pull_requests(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    let pull_requests = service(&deployment)
        .list_pull_requests_for_project(query.project_id)
        .await?;
    Ok(rows("pull_requests", pull_requests))
}

async fn fallback_pull_request_issues(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Value>, KanbanError> {
    let links = service(&deployment)
        .list_pull_request_issues(query.project_id)
        .await?;
    Ok(rows("pull_request_issues", links))
}

async fn create_project(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateProjectRequest>,
) -> KanbanResult<MutationResponse<api_types::Project>> {
    Ok(mutation(
        service(&deployment).create_project(request).await?,
    ))
}

async fn update_project(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateProjectRequest>,
) -> KanbanResult<MutationResponse<api_types::Project>> {
    Ok(mutation(
        service(&deployment).update_project(id, request).await?,
    ))
}

async fn bulk_update_projects(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<BulkRequest<UpdateProjectRequest>>,
) -> KanbanResult<MutationResponse<Vec<api_types::Project>>> {
    Ok(mutation(
        service(&deployment)
            .bulk_update_projects(request.into_pairs())
            .await?,
    ))
}

async fn delete_project(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> KanbanResult<DeleteResponse> {
    service(&deployment).delete_project(id).await?;
    Ok(deleted())
}

async fn create_status(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateProjectStatusRequest>,
) -> KanbanResult<MutationResponse<api_types::ProjectStatus>> {
    Ok(mutation(service(&deployment).create_status(request).await?))
}

async fn update_status(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateProjectStatusRequest>,
) -> KanbanResult<MutationResponse<api_types::ProjectStatus>> {
    Ok(mutation(
        service(&deployment).update_status(id, request).await?,
    ))
}

async fn bulk_update_statuses(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<BulkRequest<UpdateProjectStatusRequest>>,
) -> KanbanResult<MutationResponse<Vec<api_types::ProjectStatus>>> {
    Ok(mutation(
        service(&deployment)
            .bulk_update_statuses(request.into_pairs())
            .await?,
    ))
}

async fn delete_status(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> KanbanResult<DeleteResponse> {
    service(&deployment).delete_status(id).await?;
    Ok(deleted())
}

async fn create_issue(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateIssueRequest>,
) -> KanbanResult<MutationResponse<api_types::Issue>> {
    Ok(mutation(service(&deployment).create_issue(request).await?))
}

async fn update_issue(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateIssueRequest>,
) -> KanbanResult<MutationResponse<api_types::Issue>> {
    Ok(mutation(
        service(&deployment).update_issue(id, request).await?,
    ))
}

async fn bulk_update_issues(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<BulkRequest<UpdateIssueRequest>>,
) -> KanbanResult<MutationResponse<Vec<api_types::Issue>>> {
    Ok(mutation(
        service(&deployment)
            .bulk_update_issues(request.into_pairs())
            .await?,
    ))
}

async fn delete_issue(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> KanbanResult<DeleteResponse> {
    service(&deployment).delete_issue(id).await?;
    Ok(deleted())
}

async fn create_tag(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTagRequest>,
) -> KanbanResult<MutationResponse<api_types::Tag>> {
    Ok(mutation(service(&deployment).create_tag(request).await?))
}

async fn update_tag(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateTagRequest>,
) -> KanbanResult<MutationResponse<api_types::Tag>> {
    Ok(mutation(
        service(&deployment).update_tag(id, request).await?,
    ))
}

async fn delete_tag(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> KanbanResult<DeleteResponse> {
    service(&deployment).delete_tag(id).await?;
    Ok(deleted())
}

async fn create_issue_tag(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateIssueTagRequest>,
) -> KanbanResult<MutationResponse<api_types::IssueTag>> {
    Ok(mutation(
        service(&deployment).create_issue_tag(request).await?,
    ))
}

async fn delete_issue_tag(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> KanbanResult<DeleteResponse> {
    service(&deployment).delete_issue_tag(id).await?;
    Ok(deleted())
}

async fn create_relationship(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateIssueRelationshipRequest>,
) -> KanbanResult<MutationResponse<api_types::IssueRelationship>> {
    Ok(mutation(
        service(&deployment).create_relationship(request).await?,
    ))
}

async fn delete_relationship(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> KanbanResult<DeleteResponse> {
    service(&deployment).delete_relationship(id).await?;
    Ok(deleted())
}

async fn create_comment(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateIssueCommentRequest>,
) -> KanbanResult<MutationResponse<api_types::IssueComment>> {
    Ok(mutation(
        service(&deployment).create_comment(request).await?,
    ))
}

async fn update_comment(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateIssueCommentRequest>,
) -> KanbanResult<MutationResponse<api_types::IssueComment>> {
    Ok(mutation(
        service(&deployment).update_comment(id, request).await?,
    ))
}

async fn delete_comment(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> KanbanResult<DeleteResponse> {
    service(&deployment).delete_comment(id).await?;
    Ok(deleted())
}
