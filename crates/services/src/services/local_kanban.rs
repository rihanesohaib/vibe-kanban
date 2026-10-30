//! Single-user kanban (projects, statuses, issues, tags, relationships and
//! comments) stored in the local SQLite database. Workspaces link to issues
//! through `workspaces.task_id`.

use std::sync::{
    LazyLock,
    atomic::{AtomicU64, Ordering},
};

use api_types::{
    CreateIssueCommentRequest, CreateIssueRelationshipRequest, CreateIssueRequest,
    CreateIssueTagRequest, CreateProjectRequest, CreateProjectStatusRequest, CreateTagRequest,
    Issue, IssueComment, IssuePriority, IssueRelationship, IssueRelationshipType, IssueSortField,
    IssueTag, MemberRole, OrganizationMemberWithProfile, OrganizationWithRole, Project,
    ProjectStatus, PullRequest, PullRequestIssue, PullRequestStatus, SearchIssuesRequest,
    SortDirection, Tag, UpdateIssueCommentRequest, UpdateIssueRequest, UpdateProjectRequest,
    UpdateProjectStatusRequest, UpdateTagRequest, User, Workspace,
};
use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{FromRow, SqliteConnection, SqlitePool};
use thiserror::Error;
use uuid::Uuid;

/// The one organization every local project belongs to.
pub const LOCAL_ORGANIZATION_ID: Uuid = Uuid::from_u128(0x6c6f6361_6c00_4000_8000_000000000001);
/// The one user that owns local workspaces and authors local comments.
pub const LOCAL_USER_ID: Uuid = Uuid::from_u128(0x6c6f6361_6c00_4000_8000_000000000002);

/// Statuses seeded for every new project: (name, color, sort_order, hidden).
pub const DEFAULT_STATUSES: &[(&str, &str, i32, bool)] = &[
    ("Backlog", "220 9% 46%", 0, true),
    ("To do", "217 91% 60%", 1, false),
    ("In progress", "38 92% 50%", 2, false),
    ("In review", "258 90% 66%", 3, false),
    ("Done", "142 71% 45%", 4, false),
    ("Cancelled", "0 84% 60%", 5, true),
];

const BACKLOG: &str = "backlog";
const TODO: &str = "todo";
const IN_PROGRESS: &str = "inprogress";
const IN_REVIEW: &str = "inreview";
const DONE: &str = "done";

static CHANGE_VERSION: LazyLock<AtomicU64> =
    LazyLock::new(|| AtomicU64::new(Utc::now().timestamp_millis().max(0) as u64));

/// Changes whenever kanban data or workspace links change, so clients can
/// poll it cheaply and refetch only when needed.
pub fn change_version() -> u64 {
    CHANGE_VERSION.load(Ordering::Relaxed)
}

fn bump_version() {
    CHANGE_VERSION.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, Error)]
pub enum LocalKanbanError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, LocalKanbanError>;

/// Normalised status name used to recognise workflow columns regardless of
/// spelling ("In progress", "In Progress", "in-progress").
fn status_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

pub fn derive_issue_prefix(name: &str) -> String {
    let prefix: String = name
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .take(3)
        .collect::<String>()
        .to_uppercase();
    if prefix.is_empty() {
        "ISS".to_string()
    } else {
        prefix
    }
}

fn enum_to_str<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn enum_from_str<T: DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(Value::String(value.to_owned())).ok()
}

fn validate_name(name: &str, what: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(LocalKanbanError::Invalid(format!("{what} is required")));
    }
    Ok(trimmed.to_string())
}

pub fn local_organization() -> OrganizationWithRole {
    OrganizationWithRole {
        id: LOCAL_ORGANIZATION_ID,
        name: "Local".to_string(),
        slug: "local".to_string(),
        is_personal: true,
        issue_prefix: "LOC".to_string(),
        created_at: DateTime::<Utc>::default(),
        updated_at: DateTime::<Utc>::default(),
        user_role: MemberRole::Admin,
    }
}

pub fn local_member() -> OrganizationMemberWithProfile {
    OrganizationMemberWithProfile {
        user_id: LOCAL_USER_ID,
        role: MemberRole::Admin,
        joined_at: DateTime::<Utc>::default(),
        first_name: Some("Local".to_string()),
        last_name: Some("user".to_string()),
        username: Some("local".to_string()),
        email: None,
        avatar_url: None,
    }
}

pub fn local_user() -> User {
    User {
        id: LOCAL_USER_ID,
        email: "local@localhost".to_string(),
        first_name: Some("Local".to_string()),
        last_name: Some("user".to_string()),
        username: Some("local".to_string()),
        created_at: DateTime::<Utc>::default(),
        updated_at: DateTime::<Utc>::default(),
    }
}

#[derive(FromRow)]
struct ProjectRow {
    id: Uuid,
    name: String,
    color: String,
    sort_order: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<ProjectRow> for Project {
    fn from(row: ProjectRow) -> Self {
        Project {
            id: row.id,
            organization_id: LOCAL_ORGANIZATION_ID,
            name: row.name,
            color: row.color,
            sort_order: row.sort_order as i32,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(FromRow)]
struct StatusRow {
    id: Uuid,
    project_id: Uuid,
    name: String,
    color: String,
    sort_order: i64,
    hidden: bool,
    created_at: DateTime<Utc>,
}

impl From<StatusRow> for ProjectStatus {
    fn from(row: StatusRow) -> Self {
        ProjectStatus {
            id: row.id,
            project_id: row.project_id,
            name: row.name,
            color: row.color,
            sort_order: row.sort_order as i32,
            hidden: row.hidden,
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
struct IssueRow {
    id: Uuid,
    project_id: Uuid,
    issue_number: i64,
    simple_id: String,
    status_id: Uuid,
    title: String,
    description: Option<String>,
    priority: Option<String>,
    start_date: Option<DateTime<Utc>>,
    target_date: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    sort_order: f64,
    parent_issue_id: Option<Uuid>,
    parent_issue_sort_order: Option<f64>,
    extension_metadata: String,
    creator_user_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<IssueRow> for Issue {
    fn from(row: IssueRow) -> Self {
        Issue {
            id: row.id,
            project_id: row.project_id,
            issue_number: row.issue_number as i32,
            simple_id: row.simple_id,
            status_id: row.status_id,
            title: row.title,
            description: row.description,
            priority: row.priority.as_deref().and_then(enum_from_str),
            start_date: row.start_date,
            target_date: row.target_date,
            completed_at: row.completed_at,
            sort_order: row.sort_order,
            parent_issue_id: row.parent_issue_id,
            parent_issue_sort_order: row.parent_issue_sort_order,
            extension_metadata: serde_json::from_str(&row.extension_metadata)
                .unwrap_or_else(|_| Value::Object(Default::default())),
            creator_user_id: row.creator_user_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(FromRow)]
struct TagRow {
    id: Uuid,
    project_id: Uuid,
    name: String,
    color: String,
}

impl From<TagRow> for Tag {
    fn from(row: TagRow) -> Self {
        Tag {
            id: row.id,
            project_id: row.project_id,
            name: row.name,
            color: row.color,
        }
    }
}

#[derive(FromRow)]
struct IssueTagRow {
    id: Uuid,
    issue_id: Uuid,
    tag_id: Uuid,
}

impl From<IssueTagRow> for IssueTag {
    fn from(row: IssueTagRow) -> Self {
        IssueTag {
            id: row.id,
            issue_id: row.issue_id,
            tag_id: row.tag_id,
        }
    }
}

#[derive(FromRow)]
struct RelationshipRow {
    id: Uuid,
    issue_id: Uuid,
    related_issue_id: Uuid,
    relationship_type: String,
    created_at: DateTime<Utc>,
}

impl From<RelationshipRow> for IssueRelationship {
    fn from(row: RelationshipRow) -> Self {
        IssueRelationship {
            id: row.id,
            issue_id: row.issue_id,
            related_issue_id: row.related_issue_id,
            relationship_type: enum_from_str(&row.relationship_type)
                .unwrap_or(IssueRelationshipType::Related),
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
struct CommentRow {
    id: Uuid,
    issue_id: Uuid,
    author_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    message: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<CommentRow> for IssueComment {
    fn from(row: CommentRow) -> Self {
        IssueComment {
            id: row.id,
            issue_id: row.issue_id,
            author_id: row.author_id,
            parent_id: row.parent_id,
            message: row.message,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(FromRow)]
struct WorkspaceRow {
    id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    name: Option<String>,
    archived: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<WorkspaceRow> for Workspace {
    fn from(row: WorkspaceRow) -> Self {
        Workspace {
            id: row.id,
            project_id: row.project_id,
            owner_user_id: LOCAL_USER_ID,
            issue_id: Some(row.issue_id),
            local_workspace_id: Some(row.id),
            name: row.name,
            archived: row.archived,
            files_changed: None,
            lines_added: None,
            lines_removed: None,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(FromRow)]
struct PullRequestRow {
    id: String,
    pr_url: String,
    pr_number: i64,
    pr_status: String,
    merged_at: Option<DateTime<Utc>>,
    merge_commit_sha: Option<String>,
    target_branch_name: String,
    workspace_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl PullRequestRow {
    fn pull_request_id(&self) -> Uuid {
        Uuid::parse_str(&self.id).unwrap_or_default()
    }

    fn status(&self) -> PullRequestStatus {
        enum_from_str(&self.pr_status).unwrap_or(PullRequestStatus::Open)
    }

    #[allow(deprecated)]
    fn into_api(self) -> PullRequest {
        PullRequest {
            id: self.pull_request_id(),
            status: self.status(),
            url: self.pr_url,
            number: self.pr_number as i32,
            merged_at: self.merged_at,
            merge_commit_sha: self.merge_commit_sha,
            target_branch_name: self.target_branch_name,
            project_id: self.project_id,
            issue_id: self.issue_id,
            workspace_id: Some(self.workspace_id),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

const PROJECT_COLUMNS: &str = "id, name, color, sort_order, created_at, updated_at";
const STATUS_COLUMNS: &str = "id, project_id, name, color, sort_order, hidden, created_at";
const ISSUE_COLUMNS: &str = "id, project_id, issue_number, simple_id, status_id, title, \
     description, priority, start_date, target_date, completed_at, sort_order, parent_issue_id, \
     parent_issue_sort_order, extension_metadata, creator_user_id, created_at, updated_at";
const COMMENT_COLUMNS: &str = "id, issue_id, author_id, parent_id, message, created_at, updated_at";
const WORKSPACE_VIEW: &str = "SELECT w.id, i.project_id, i.id AS issue_id, w.name, w.archived, \
     w.created_at, w.updated_at FROM workspaces w JOIN issues i ON i.id = w.task_id";
const PULL_REQUEST_VIEW: &str = "SELECT pr.id, pr.pr_url, pr.pr_number, pr.pr_status, \
     pr.merged_at, pr.merge_commit_sha, pr.target_branch_name, pr.workspace_id, i.project_id, \
     i.id AS issue_id, pr.created_at, pr.updated_at FROM pull_requests pr \
     JOIN workspaces w ON w.id = pr.workspace_id JOIN issues i ON i.id = w.task_id";

#[derive(Clone)]
pub struct LocalKanbanService {
    pool: SqlitePool,
}

impl LocalKanbanService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ---------------------------------------------------------------------
    // Projects
    // ---------------------------------------------------------------------

    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let rows: Vec<ProjectRow> = sqlx::query_as(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY sort_order, created_at"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_project(&self, id: Uuid) -> Result<Project> {
        let row: Option<ProjectRow> = sqlx::query_as(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Into::into)
            .ok_or(LocalKanbanError::NotFound("project"))
    }

    pub async fn create_project(&self, request: CreateProjectRequest) -> Result<Project> {
        let name = validate_name(&request.name, "Project name")?;
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        let mut tx = self.pool.begin().await?;
        let (next_sort,): (i64,) =
            sqlx::query_as("SELECT COALESCE(MAX(sort_order) + 1, 0) FROM projects")
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query(
            "INSERT INTO projects (id, name, color, sort_order, issue_prefix) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&name)
        .bind(&request.color)
        .bind(next_sort)
        .bind(derive_issue_prefix(&name))
        .execute(&mut *tx)
        .await?;
        seed_default_statuses(&mut tx, id).await?;
        tx.commit().await?;
        bump_version();
        self.get_project(id).await
    }

    pub async fn update_project(&self, id: Uuid, changes: UpdateProjectRequest) -> Result<Project> {
        let mut conn = self.pool.acquire().await?;
        update_project_in(&mut conn, id, changes).await?;
        drop(conn);
        bump_version();
        self.get_project(id).await
    }

    pub async fn bulk_update_projects(
        &self,
        updates: Vec<(Uuid, UpdateProjectRequest)>,
    ) -> Result<Vec<Project>> {
        let mut tx = self.pool.begin().await?;
        let ids: Vec<Uuid> = updates.iter().map(|(id, _)| *id).collect();
        for (id, changes) in updates {
            update_project_in(&mut tx, id, changes).await?;
        }
        tx.commit().await?;
        bump_version();
        let mut projects = Vec::with_capacity(ids.len());
        for id in ids {
            projects.push(self.get_project(id).await?);
        }
        Ok(projects)
    }

    pub async fn delete_project(&self, id: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE workspaces SET task_id = NULL \
             WHERE task_id IN (SELECT id FROM issues WHERE project_id = ?)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM issues WHERE project_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("project"));
        }
        tx.commit().await?;
        bump_version();
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Project statuses
    // ---------------------------------------------------------------------

    pub async fn list_statuses(&self, project_id: Uuid) -> Result<Vec<ProjectStatus>> {
        let rows: Vec<StatusRow> = sqlx::query_as(&format!(
            "SELECT {STATUS_COLUMNS} FROM project_statuses WHERE project_id = ? \
             ORDER BY sort_order, created_at"
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_status(&self, id: Uuid) -> Result<ProjectStatus> {
        fetch_status(&self.pool, id)
            .await?
            .map(Into::into)
            .ok_or(LocalKanbanError::NotFound("status"))
    }

    pub async fn create_status(
        &self,
        request: CreateProjectStatusRequest,
    ) -> Result<ProjectStatus> {
        let name = validate_name(&request.name, "Status name")?;
        self.get_project(request.project_id).await?;
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        sqlx::query(
            "INSERT INTO project_statuses (id, project_id, name, color, sort_order, hidden) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(request.project_id)
        .bind(name)
        .bind(&request.color)
        .bind(request.sort_order)
        .bind(request.hidden)
        .execute(&self.pool)
        .await?;
        bump_version();
        self.get_status(id).await
    }

    pub async fn update_status(
        &self,
        id: Uuid,
        changes: UpdateProjectStatusRequest,
    ) -> Result<ProjectStatus> {
        let mut conn = self.pool.acquire().await?;
        update_status_in(&mut conn, id, changes).await?;
        drop(conn);
        bump_version();
        self.get_status(id).await
    }

    pub async fn bulk_update_statuses(
        &self,
        updates: Vec<(Uuid, UpdateProjectStatusRequest)>,
    ) -> Result<Vec<ProjectStatus>> {
        let mut tx = self.pool.begin().await?;
        let ids: Vec<Uuid> = updates.iter().map(|(id, _)| *id).collect();
        for (id, changes) in updates {
            update_status_in(&mut tx, id, changes).await?;
        }
        tx.commit().await?;
        bump_version();
        let mut statuses = Vec::with_capacity(ids.len());
        for id in ids {
            statuses.push(self.get_status(id).await?);
        }
        Ok(statuses)
    }

    pub async fn delete_status(&self, id: Uuid) -> Result<()> {
        let (in_use,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM issues WHERE status_id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        if in_use > 0 {
            return Err(LocalKanbanError::Invalid(
                "Move the issues out of this status before deleting it".to_string(),
            ));
        }
        let result = sqlx::query("DELETE FROM project_statuses WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("status"));
        }
        bump_version();
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Issues
    // ---------------------------------------------------------------------

    pub async fn list_issues(&self, project_id: Uuid) -> Result<Vec<Issue>> {
        let rows: Vec<IssueRow> = sqlx::query_as(&format!(
            "SELECT {ISSUE_COLUMNS} FROM issues WHERE project_id = ? \
             ORDER BY sort_order, issue_number"
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_issue(&self, id: Uuid) -> Result<Issue> {
        fetch_issue(&self.pool, id)
            .await?
            .map(Into::into)
            .ok_or(LocalKanbanError::NotFound("issue"))
    }

    pub async fn create_issue(&self, request: CreateIssueRequest) -> Result<Issue> {
        let title = validate_name(&request.title, "Issue title")?;
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        let mut tx = self.pool.begin().await?;

        let status = fetch_status(&mut *tx, request.status_id)
            .await?
            .filter(|s| s.project_id == request.project_id)
            .ok_or_else(|| {
                LocalKanbanError::Invalid("Status does not belong to the project".to_string())
            })?;

        let (number, prefix): (i64, String) = sqlx::query_as(
            "UPDATE projects SET issue_counter = issue_counter + 1 WHERE id = ? \
             RETURNING issue_counter, issue_prefix",
        )
        .bind(request.project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(LocalKanbanError::NotFound("project"))?;

        let completed_at = request
            .completed_at
            .or_else(|| (status_key(&status.name) == DONE).then(Utc::now));
        let metadata = match &request.extension_metadata {
            Value::Null => "{}".to_string(),
            other => other.to_string(),
        };

        sqlx::query(
            "INSERT INTO issues (id, project_id, issue_number, simple_id, status_id, title, \
             description, priority, start_date, target_date, completed_at, sort_order, \
             parent_issue_id, parent_issue_sort_order, extension_metadata, creator_user_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(request.project_id)
        .bind(number)
        .bind(format!("{prefix}-{number}"))
        .bind(request.status_id)
        .bind(title)
        .bind(&request.description)
        .bind(request.priority.as_ref().map(enum_to_str))
        .bind(request.start_date)
        .bind(request.target_date)
        .bind(completed_at)
        .bind(request.sort_order)
        .bind(request.parent_issue_id)
        .bind(request.parent_issue_sort_order)
        .bind(metadata)
        .bind(LOCAL_USER_ID)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        bump_version();
        self.get_issue(id).await
    }

    pub async fn update_issue(&self, id: Uuid, changes: UpdateIssueRequest) -> Result<Issue> {
        let mut conn = self.pool.acquire().await?;
        update_issue_in(&mut conn, id, changes).await?;
        drop(conn);
        bump_version();
        self.get_issue(id).await
    }

    pub async fn bulk_update_issues(
        &self,
        updates: Vec<(Uuid, UpdateIssueRequest)>,
    ) -> Result<Vec<Issue>> {
        let mut tx = self.pool.begin().await?;
        let ids: Vec<Uuid> = updates.iter().map(|(id, _)| *id).collect();
        for (id, changes) in updates {
            update_issue_in(&mut tx, id, changes).await?;
        }
        tx.commit().await?;
        bump_version();
        let mut issues = Vec::with_capacity(ids.len());
        for id in ids {
            issues.push(self.get_issue(id).await?);
        }
        Ok(issues)
    }

    pub async fn delete_issue(&self, id: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE workspaces SET task_id = NULL WHERE task_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM issues WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("issue"));
        }
        tx.commit().await?;
        bump_version();
        Ok(())
    }

    /// Filters, sorts and pages a project's issues. Returns the page and the
    /// total number of matches.
    pub async fn search_issues(
        &self,
        request: &SearchIssuesRequest,
    ) -> Result<(Vec<Issue>, usize)> {
        let mut issues = self.list_issues(request.project_id).await?;

        let mut status_ids: Vec<Uuid> = request.status_ids.clone().unwrap_or_default();
        status_ids.extend(request.status_id);
        if !status_ids.is_empty() {
            issues.retain(|i| status_ids.contains(&i.status_id));
        }
        if let Some(priority) = request.priority {
            issues.retain(|i| i.priority == Some(priority));
        }
        if let Some(parent) = request.parent_issue_id {
            issues.retain(|i| i.parent_issue_id == Some(parent));
        }
        if let Some(simple_id) = &request.simple_id {
            issues.retain(|i| i.simple_id.eq_ignore_ascii_case(simple_id));
        }
        if let Some(search) = request.search.as_deref().map(str::to_lowercase)
            && !search.is_empty()
        {
            issues.retain(|i| {
                i.title.to_lowercase().contains(&search)
                    || i.simple_id.to_lowercase().contains(&search)
                    || i.description
                        .as_deref()
                        .is_some_and(|d| d.to_lowercase().contains(&search))
            });
        }
        if let Some(assignee) = request.assignee_user_id
            && assignee != LOCAL_USER_ID
        {
            issues.clear();
        }
        let mut tag_ids: Vec<Uuid> = request.tag_ids.clone().unwrap_or_default();
        tag_ids.extend(request.tag_id);
        if !tag_ids.is_empty() {
            let tagged: Vec<IssueTag> =
                self.list_issue_tags_for_project(request.project_id).await?;
            issues.retain(|i| {
                tagged
                    .iter()
                    .any(|t| t.issue_id == i.id && tag_ids.contains(&t.tag_id))
            });
        }

        let field = request.sort_field.unwrap_or(IssueSortField::SortOrder);
        issues.sort_by(|a, b| match field {
            IssueSortField::SortOrder => a.sort_order.total_cmp(&b.sort_order),
            IssueSortField::Priority => priority_rank(a.priority).cmp(&priority_rank(b.priority)),
            IssueSortField::CreatedAt => a.created_at.cmp(&b.created_at),
            IssueSortField::UpdatedAt => a.updated_at.cmp(&b.updated_at),
            IssueSortField::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        });
        if request.sort_direction == Some(SortDirection::Desc) {
            issues.reverse();
        }

        let total = issues.len();
        let offset = request.offset.unwrap_or(0).max(0) as usize;
        let limit = request
            .limit
            .map(|l| l.max(0) as usize)
            .unwrap_or(usize::MAX);
        let page = issues.into_iter().skip(offset).take(limit).collect();
        Ok((page, total))
    }

    // ---------------------------------------------------------------------
    // Tags
    // ---------------------------------------------------------------------

    pub async fn list_tags(&self, project_id: Uuid) -> Result<Vec<Tag>> {
        let rows: Vec<TagRow> = sqlx::query_as(
            "SELECT id, project_id, name, color FROM kanban_tags WHERE project_id = ? \
             ORDER BY name",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_tag(&self, id: Uuid) -> Result<Tag> {
        let row: Option<TagRow> =
            sqlx::query_as("SELECT id, project_id, name, color FROM kanban_tags WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(Into::into).ok_or(LocalKanbanError::NotFound("tag"))
    }

    pub async fn create_tag(&self, request: CreateTagRequest) -> Result<Tag> {
        let name = validate_name(&request.name, "Tag name")?;
        self.get_project(request.project_id).await?;
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        sqlx::query("INSERT INTO kanban_tags (id, project_id, name, color) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(request.project_id)
            .bind(name)
            .bind(&request.color)
            .execute(&self.pool)
            .await?;
        bump_version();
        self.get_tag(id).await
    }

    pub async fn update_tag(&self, id: Uuid, changes: UpdateTagRequest) -> Result<Tag> {
        let current = self.get_tag(id).await?;
        let name = match changes.name {
            Some(name) => validate_name(&name, "Tag name")?,
            None => current.name,
        };
        sqlx::query("UPDATE kanban_tags SET name = ?, color = ? WHERE id = ?")
            .bind(name)
            .bind(changes.color.unwrap_or(current.color))
            .bind(id)
            .execute(&self.pool)
            .await?;
        bump_version();
        self.get_tag(id).await
    }

    pub async fn delete_tag(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM kanban_tags WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("tag"));
        }
        bump_version();
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Issue tags
    // ---------------------------------------------------------------------

    pub async fn list_issue_tags_for_project(&self, project_id: Uuid) -> Result<Vec<IssueTag>> {
        let rows: Vec<IssueTagRow> = sqlx::query_as(
            "SELECT it.id, it.issue_id, it.tag_id FROM issue_tags it \
             JOIN issues i ON i.id = it.issue_id WHERE i.project_id = ?",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_issue_tags(&self, issue_id: Uuid) -> Result<Vec<IssueTag>> {
        let rows: Vec<IssueTagRow> =
            sqlx::query_as("SELECT id, issue_id, tag_id FROM issue_tags WHERE issue_id = ?")
                .bind(issue_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_issue_tag(&self, id: Uuid) -> Result<IssueTag> {
        let row: Option<IssueTagRow> =
            sqlx::query_as("SELECT id, issue_id, tag_id FROM issue_tags WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(Into::into)
            .ok_or(LocalKanbanError::NotFound("issue tag"))
    }

    pub async fn create_issue_tag(&self, request: CreateIssueTagRequest) -> Result<IssueTag> {
        let issue = self.get_issue(request.issue_id).await?;
        let tag = self.get_tag(request.tag_id).await?;
        if issue.project_id != tag.project_id {
            return Err(LocalKanbanError::Invalid(
                "Tag belongs to a different project".to_string(),
            ));
        }
        let existing: Option<IssueTagRow> = sqlx::query_as(
            "SELECT id, issue_id, tag_id FROM issue_tags WHERE issue_id = ? AND tag_id = ?",
        )
        .bind(request.issue_id)
        .bind(request.tag_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(existing) = existing {
            return Ok(existing.into());
        }
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        sqlx::query("INSERT INTO issue_tags (id, issue_id, tag_id) VALUES (?, ?, ?)")
            .bind(id)
            .bind(request.issue_id)
            .bind(request.tag_id)
            .execute(&self.pool)
            .await?;
        bump_version();
        self.get_issue_tag(id).await
    }

    pub async fn delete_issue_tag(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM issue_tags WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("issue tag"));
        }
        bump_version();
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Issue relationships
    // ---------------------------------------------------------------------

    pub async fn list_relationships_for_project(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<IssueRelationship>> {
        let rows: Vec<RelationshipRow> = sqlx::query_as(
            "SELECT r.id, r.issue_id, r.related_issue_id, r.relationship_type, r.created_at \
             FROM issue_relationships r JOIN issues i ON i.id = r.issue_id \
             WHERE i.project_id = ? ORDER BY r.created_at",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_relationships(&self, issue_id: Uuid) -> Result<Vec<IssueRelationship>> {
        let rows: Vec<RelationshipRow> = sqlx::query_as(
            "SELECT id, issue_id, related_issue_id, relationship_type, created_at \
             FROM issue_relationships WHERE issue_id = ? OR related_issue_id = ? \
             ORDER BY created_at",
        )
        .bind(issue_id)
        .bind(issue_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn create_relationship(
        &self,
        request: CreateIssueRelationshipRequest,
    ) -> Result<IssueRelationship> {
        if request.issue_id == request.related_issue_id {
            return Err(LocalKanbanError::Invalid(
                "An issue cannot be related to itself".to_string(),
            ));
        }
        self.get_issue(request.issue_id).await?;
        self.get_issue(request.related_issue_id).await?;
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        sqlx::query(
            "INSERT INTO issue_relationships (id, issue_id, related_issue_id, relationship_type) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(request.issue_id)
        .bind(request.related_issue_id)
        .bind(enum_to_str(&request.relationship_type))
        .execute(&self.pool)
        .await?;
        bump_version();
        let row: RelationshipRow = sqlx::query_as(
            "SELECT id, issue_id, related_issue_id, relationship_type, created_at \
             FROM issue_relationships WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    pub async fn delete_relationship(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM issue_relationships WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("relationship"));
        }
        bump_version();
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Comments
    // ---------------------------------------------------------------------

    pub async fn list_comments(&self, issue_id: Uuid) -> Result<Vec<IssueComment>> {
        let rows: Vec<CommentRow> = sqlx::query_as(&format!(
            "SELECT {COMMENT_COLUMNS} FROM issue_comments WHERE issue_id = ? ORDER BY created_at"
        ))
        .bind(issue_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn get_comment(&self, id: Uuid) -> Result<IssueComment> {
        let row: Option<CommentRow> = sqlx::query_as(&format!(
            "SELECT {COMMENT_COLUMNS} FROM issue_comments WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Into::into)
            .ok_or(LocalKanbanError::NotFound("comment"))
    }

    pub async fn create_comment(&self, request: CreateIssueCommentRequest) -> Result<IssueComment> {
        let message = validate_name(&request.message, "Comment")?;
        self.get_issue(request.issue_id).await?;
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        sqlx::query(
            "INSERT INTO issue_comments (id, issue_id, author_id, parent_id, message) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(request.issue_id)
        .bind(LOCAL_USER_ID)
        .bind(request.parent_id)
        .bind(message)
        .execute(&self.pool)
        .await?;
        bump_version();
        self.get_comment(id).await
    }

    pub async fn update_comment(
        &self,
        id: Uuid,
        changes: UpdateIssueCommentRequest,
    ) -> Result<IssueComment> {
        let current = self.get_comment(id).await?;
        let message = match changes.message {
            Some(message) => validate_name(&message, "Comment")?,
            None => current.message,
        };
        sqlx::query(
            "UPDATE issue_comments SET message = ?, parent_id = ?, \
             updated_at = datetime('now', 'subsec') WHERE id = ?",
        )
        .bind(message)
        .bind(changes.parent_id.unwrap_or(current.parent_id))
        .bind(id)
        .execute(&self.pool)
        .await?;
        bump_version();
        self.get_comment(id).await
    }

    pub async fn delete_comment(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM issue_comments WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("comment"));
        }
        bump_version();
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Workspaces and pull requests (views over local data)
    // ---------------------------------------------------------------------

    /// Local workspaces linked to an existing issue, optionally limited to
    /// one project.
    pub async fn list_workspaces(&self, project_id: Option<Uuid>) -> Result<Vec<Workspace>> {
        let rows: Vec<WorkspaceRow> = match project_id {
            Some(project_id) => {
                sqlx::query_as(&format!(
                    "{WORKSPACE_VIEW} WHERE i.project_id = ? ORDER BY w.created_at DESC"
                ))
                .bind(project_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(&format!("{WORKSPACE_VIEW} ORDER BY w.created_at DESC"))
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_workspace(&self, local_workspace_id: Uuid) -> Result<Workspace> {
        let row: Option<WorkspaceRow> = sqlx::query_as(&format!("{WORKSPACE_VIEW} WHERE w.id = ?"))
            .bind(local_workspace_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(Into::into)
            .ok_or(LocalKanbanError::NotFound("linked workspace"))
    }

    pub async fn list_pull_requests_for_project(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<PullRequest>> {
        let rows: Vec<PullRequestRow> = sqlx::query_as(&format!(
            "{PULL_REQUEST_VIEW} WHERE i.project_id = ? ORDER BY pr.created_at"
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(PullRequestRow::into_api).collect())
    }

    pub async fn list_pull_requests_for_issue(&self, issue_id: Uuid) -> Result<Vec<PullRequest>> {
        let rows: Vec<PullRequestRow> = sqlx::query_as(&format!(
            "{PULL_REQUEST_VIEW} WHERE i.id = ? ORDER BY pr.created_at"
        ))
        .bind(issue_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(PullRequestRow::into_api).collect())
    }

    pub async fn list_pull_request_issues(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<PullRequestIssue>> {
        let rows: Vec<PullRequestRow> =
            sqlx::query_as(&format!("{PULL_REQUEST_VIEW} WHERE i.project_id = ?"))
                .bind(project_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let pull_request_id = row.pull_request_id();
                PullRequestIssue {
                    id: pull_request_id,
                    pull_request_id,
                    issue_id: row.issue_id,
                }
            })
            .collect())
    }

    // ---------------------------------------------------------------------
    // Workspace links and automatic status changes
    // ---------------------------------------------------------------------

    /// Links a local workspace to an issue. If an agent is already running in
    /// the workspace, the issue moves to In progress.
    pub async fn link_workspace(&self, workspace_id: Uuid, issue_id: Uuid) -> Result<()> {
        self.get_issue(issue_id).await?;
        let result = sqlx::query("UPDATE workspaces SET task_id = ? WHERE id = ?")
            .bind(issue_id)
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(LocalKanbanError::NotFound("workspace"));
        }
        bump_version();

        let (running,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM execution_processes ep \
             JOIN sessions s ON s.id = ep.session_id \
             WHERE s.workspace_id = ? AND ep.status = 'running' AND ep.run_reason = 'codingagent'",
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await?;
        if running > 0 {
            self.on_coding_agent_started(workspace_id).await?;
        }
        Ok(())
    }

    pub async fn unlink_workspace(&self, workspace_id: Uuid) -> Result<()> {
        sqlx::query("UPDATE workspaces SET task_id = NULL WHERE id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        bump_version();
        Ok(())
    }

    async fn linked_issue(&self, workspace_id: Uuid) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT i.id FROM workspaces w JOIN issues i ON i.id = w.task_id WHERE w.id = ?",
        )
        .bind(workspace_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    /// A coding agent started in a workspace: its issue moves to In progress
    /// unless it is already finished.
    pub async fn on_coding_agent_started(&self, workspace_id: Uuid) -> Result<()> {
        if let Some(issue_id) = self.linked_issue(workspace_id).await? {
            self.move_issue(issue_id, IN_PROGRESS, &[BACKLOG, TODO, IN_REVIEW])
                .await?;
        }
        Ok(())
    }

    /// A coding agent finished: the issue moves to In review once no agent is
    /// still running for it in any linked workspace.
    pub async fn on_coding_agent_finished(&self, workspace_id: Uuid) -> Result<()> {
        let Some(issue_id) = self.linked_issue(workspace_id).await? else {
            return Ok(());
        };
        let (running,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM execution_processes ep \
             JOIN sessions s ON s.id = ep.session_id \
             JOIN workspaces w ON w.id = s.workspace_id \
             WHERE w.task_id = ? AND ep.status = 'running' AND ep.run_reason = 'codingagent'",
        )
        .bind(issue_id)
        .fetch_one(&self.pool)
        .await?;
        if running == 0 {
            self.move_issue(issue_id, IN_REVIEW, &[BACKLOG, TODO, IN_PROGRESS])
                .await?;
        }
        Ok(())
    }

    /// The workspace branch was merged locally: the issue is done.
    pub async fn on_workspace_merged(&self, workspace_id: Uuid) -> Result<()> {
        if let Some(issue_id) = self.linked_issue(workspace_id).await? {
            self.move_issue(issue_id, DONE, &[BACKLOG, TODO, IN_PROGRESS, IN_REVIEW])
                .await?;
        }
        Ok(())
    }

    /// A pull request of the workspace was opened or changed state. Open moves
    /// the issue to In review; once every linked pull request is merged it is
    /// done.
    pub async fn on_pull_request_status(
        &self,
        workspace_id: Uuid,
        status: PullRequestStatus,
    ) -> Result<()> {
        let Some(issue_id) = self.linked_issue(workspace_id).await? else {
            return Ok(());
        };
        match status {
            PullRequestStatus::Open => {
                self.move_issue(issue_id, IN_REVIEW, &[BACKLOG, TODO, IN_PROGRESS])
                    .await?;
            }
            PullRequestStatus::Merged => {
                let prs = self.list_pull_requests_for_issue(issue_id).await?;
                if prs.iter().all(|pr| pr.status == PullRequestStatus::Merged) {
                    self.move_issue(issue_id, DONE, &[BACKLOG, TODO, IN_PROGRESS, IN_REVIEW])
                        .await?;
                }
            }
            PullRequestStatus::Closed => {}
        }
        Ok(())
    }

    /// Moves an issue to the status whose normalised name is `target` when its
    /// current status is one of `from`. Returns whether it moved.
    async fn move_issue(&self, issue_id: Uuid, target: &str, from: &[&str]) -> Result<bool> {
        let Some(issue) = fetch_issue(&self.pool, issue_id).await? else {
            return Ok(false);
        };
        let Some(current) = fetch_status(&self.pool, issue.status_id).await? else {
            return Ok(false);
        };
        if !from.contains(&status_key(&current.name).as_str()) {
            return Ok(false);
        }
        let statuses = self.list_statuses(issue.project_id).await?;
        let Some(target_status) = statuses.iter().find(|s| status_key(&s.name) == target) else {
            return Ok(false);
        };
        let completed_at = if target == DONE {
            Some(issue.completed_at.unwrap_or_else(Utc::now))
        } else {
            None
        };
        sqlx::query(
            "UPDATE issues SET status_id = ?, completed_at = ?, \
             updated_at = datetime('now', 'subsec') WHERE id = ?",
        )
        .bind(target_status.id)
        .bind(completed_at)
        .bind(issue_id)
        .execute(&self.pool)
        .await?;
        bump_version();
        tracing::info!(
            "Moved issue {} from {} to {}",
            issue.simple_id,
            current.name,
            target_status.name
        );
        Ok(true)
    }
}

fn priority_rank(priority: Option<IssuePriority>) -> u8 {
    match priority {
        Some(IssuePriority::Urgent) => 0,
        Some(IssuePriority::High) => 1,
        Some(IssuePriority::Medium) => 2,
        Some(IssuePriority::Low) => 3,
        None => 4,
    }
}

async fn seed_default_statuses(conn: &mut SqliteConnection, project_id: Uuid) -> Result<()> {
    for (name, color, sort_order, hidden) in DEFAULT_STATUSES {
        sqlx::query(
            "INSERT INTO project_statuses (id, project_id, name, color, sort_order, hidden) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(*name)
        .bind(*color)
        .bind(*sort_order)
        .bind(*hidden)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn fetch_status<'e, E>(executor: E, id: Uuid) -> Result<Option<StatusRow>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query_as(&format!(
        "SELECT {STATUS_COLUMNS} FROM project_statuses WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(executor)
    .await?)
}

async fn fetch_issue<'e, E>(executor: E, id: Uuid) -> Result<Option<IssueRow>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(
        sqlx::query_as(&format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id = ?"))
            .bind(id)
            .fetch_optional(executor)
            .await?,
    )
}

async fn update_project_in(
    conn: &mut SqliteConnection,
    id: Uuid,
    changes: UpdateProjectRequest,
) -> Result<()> {
    let current: ProjectRow = sqlx::query_as(&format!(
        "SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(LocalKanbanError::NotFound("project"))?;
    let name = match changes.name {
        Some(name) => validate_name(&name, "Project name")?,
        None => current.name,
    };
    sqlx::query(
        "UPDATE projects SET name = ?, color = ?, sort_order = ?, \
         updated_at = datetime('now', 'subsec') WHERE id = ?",
    )
    .bind(name)
    .bind(changes.color.unwrap_or(current.color))
    .bind(
        changes
            .sort_order
            .map(i64::from)
            .unwrap_or(current.sort_order),
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn update_status_in(
    conn: &mut SqliteConnection,
    id: Uuid,
    changes: UpdateProjectStatusRequest,
) -> Result<()> {
    let current = fetch_status(&mut *conn, id)
        .await?
        .ok_or(LocalKanbanError::NotFound("status"))?;
    let name = match changes.name {
        Some(name) => validate_name(&name, "Status name")?,
        None => current.name,
    };
    sqlx::query(
        "UPDATE project_statuses SET name = ?, color = ?, sort_order = ?, hidden = ? WHERE id = ?",
    )
    .bind(name)
    .bind(changes.color.unwrap_or(current.color))
    .bind(
        changes
            .sort_order
            .map(i64::from)
            .unwrap_or(current.sort_order),
    )
    .bind(changes.hidden.unwrap_or(current.hidden))
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn update_issue_in(
    conn: &mut SqliteConnection,
    id: Uuid,
    changes: UpdateIssueRequest,
) -> Result<()> {
    let mut issue = fetch_issue(&mut *conn, id)
        .await?
        .ok_or(LocalKanbanError::NotFound("issue"))?;

    if let Some(status_id) = changes.status_id
        && status_id != issue.status_id
    {
        let status = fetch_status(&mut *conn, status_id)
            .await?
            .filter(|s| s.project_id == issue.project_id)
            .ok_or_else(|| {
                LocalKanbanError::Invalid("Status does not belong to the project".to_string())
            })?;
        if changes.completed_at.is_none() {
            issue.completed_at = if status_key(&status.name) == DONE {
                Some(issue.completed_at.unwrap_or_else(Utc::now))
            } else {
                None
            };
        }
        issue.status_id = status_id;
    }
    if let Some(title) = changes.title {
        issue.title = validate_name(&title, "Issue title")?;
    }
    if let Some(description) = changes.description {
        issue.description = description;
    }
    if let Some(priority) = changes.priority {
        issue.priority = priority.as_ref().map(enum_to_str);
    }
    if let Some(start_date) = changes.start_date {
        issue.start_date = start_date;
    }
    if let Some(target_date) = changes.target_date {
        issue.target_date = target_date;
    }
    if let Some(completed_at) = changes.completed_at {
        issue.completed_at = completed_at;
    }
    if let Some(sort_order) = changes.sort_order {
        issue.sort_order = sort_order;
    }
    if let Some(parent_issue_id) = changes.parent_issue_id {
        if parent_issue_id == Some(id) {
            return Err(LocalKanbanError::Invalid(
                "An issue cannot be its own parent".to_string(),
            ));
        }
        issue.parent_issue_id = parent_issue_id;
    }
    if let Some(parent_issue_sort_order) = changes.parent_issue_sort_order {
        issue.parent_issue_sort_order = parent_issue_sort_order;
    }
    if let Some(metadata) = changes.extension_metadata {
        issue.extension_metadata = metadata.to_string();
    }

    sqlx::query(
        "UPDATE issues SET status_id = ?, title = ?, description = ?, priority = ?, \
         start_date = ?, target_date = ?, completed_at = ?, sort_order = ?, \
         parent_issue_id = ?, parent_issue_sort_order = ?, extension_metadata = ?, \
         updated_at = datetime('now', 'subsec') WHERE id = ?",
    )
    .bind(issue.status_id)
    .bind(&issue.title)
    .bind(&issue.description)
    .bind(&issue.priority)
    .bind(issue.start_date)
    .bind(issue.target_date)
    .bind(issue.completed_at)
    .bind(issue.sort_order)
    .bind(issue.parent_issue_id)
    .bind(issue.parent_issue_sort_order)
    .bind(&issue.extension_metadata)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use db::DBService;
    use serde_json::json;

    use super::*;

    async fn service() -> LocalKanbanService {
        let db = DBService::new_in_memory().await.expect("in-memory db");
        LocalKanbanService::new(db.pool)
    }

    async fn project(service: &LocalKanbanService, name: &str) -> Project {
        service
            .create_project(CreateProjectRequest {
                id: None,
                organization_id: LOCAL_ORGANIZATION_ID,
                name: name.to_string(),
                color: "217 91% 60%".to_string(),
            })
            .await
            .expect("create project")
    }

    async fn status_named(service: &LocalKanbanService, project_id: Uuid, name: &str) -> Uuid {
        service
            .list_statuses(project_id)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("status {name}"))
            .id
    }

    async fn issue(service: &LocalKanbanService, project_id: Uuid, status_id: Uuid) -> Issue {
        service
            .create_issue(CreateIssueRequest {
                id: None,
                project_id,
                status_id,
                title: "Sample issue".to_string(),
                description: None,
                priority: Some(IssuePriority::High),
                start_date: None,
                target_date: None,
                completed_at: None,
                sort_order: 1.0,
                parent_issue_id: None,
                parent_issue_sort_order: None,
                extension_metadata: json!({}),
            })
            .await
            .expect("create issue")
    }

    async fn workspace(service: &LocalKanbanService) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces (id, branch) VALUES (?, 'jarvis/test')")
            .bind(id)
            .execute(&service.pool)
            .await
            .unwrap();
        id
    }

    async fn coding_agent(service: &LocalKanbanService, workspace_id: Uuid, status: &str) -> Uuid {
        let session_id = Uuid::new_v4();
        sqlx::query("INSERT INTO sessions (id, workspace_id) VALUES (?, ?)")
            .bind(session_id)
            .bind(workspace_id)
            .execute(&service.pool)
            .await
            .unwrap();
        let process_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO execution_processes (id, session_id, status, run_reason) \
             VALUES (?, ?, ?, 'codingagent')",
        )
        .bind(process_id)
        .bind(session_id)
        .bind(status)
        .execute(&service.pool)
        .await
        .unwrap();
        process_id
    }

    async fn status_name(service: &LocalKanbanService, issue_id: Uuid) -> String {
        let issue = service.get_issue(issue_id).await.unwrap();
        service.get_status(issue.status_id).await.unwrap().name
    }

    #[tokio::test]
    async fn new_project_gets_default_statuses() {
        let service = service().await;
        let project = project(&service, "Northwind Traders").await;

        let statuses = service.list_statuses(project.id).await.unwrap();
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Backlog",
                "To do",
                "In progress",
                "In review",
                "Done",
                "Cancelled"
            ]
        );
        assert!(statuses[0].hidden && statuses[5].hidden);
        assert!(statuses[1..5].iter().all(|s| !s.hidden));
        assert_eq!(project.organization_id, LOCAL_ORGANIZATION_ID);

        let second = self::project(&service, "Second board").await;
        assert_eq!(second.sort_order, project.sort_order + 1);
    }

    #[tokio::test]
    async fn issues_get_sequential_simple_ids_per_project() {
        let service = service().await;
        let first = project(&service, "Northwind Traders").await;
        let second = project(&service, "42 Sample app").await;
        let first_todo = status_named(&service, first.id, "To do").await;
        let second_todo = status_named(&service, second.id, "To do").await;

        let a = issue(&service, first.id, first_todo).await;
        let b = issue(&service, first.id, first_todo).await;
        let c = issue(&service, second.id, second_todo).await;

        assert_eq!((a.issue_number, a.simple_id.as_str()), (1, "NOR-1"));
        assert_eq!((b.issue_number, b.simple_id.as_str()), (2, "NOR-2"));
        assert_eq!((c.issue_number, c.simple_id.as_str()), (1, "SAM-1"));

        service.delete_issue(b.id).await.unwrap();
        let d = issue(&service, first.id, first_todo).await;
        assert_eq!(d.simple_id, "NOR-3", "numbers are never reused");
        assert_eq!(a.priority, Some(IssuePriority::High));
    }

    #[tokio::test]
    async fn issue_status_must_belong_to_project() {
        let service = service().await;
        let first = project(&service, "Alpha").await;
        let second = project(&service, "Beta").await;
        let other_status = status_named(&service, second.id, "To do").await;

        let result = service
            .create_issue(CreateIssueRequest {
                id: None,
                project_id: first.id,
                status_id: other_status,
                title: "Wrong board".to_string(),
                description: None,
                priority: None,
                start_date: None,
                target_date: None,
                completed_at: None,
                sort_order: 0.0,
                parent_issue_id: None,
                parent_issue_sort_order: None,
                extension_metadata: Value::Null,
            })
            .await;
        assert!(matches!(result, Err(LocalKanbanError::Invalid(_))));
    }

    #[tokio::test]
    async fn moving_to_done_sets_completed_at() {
        let service = service().await;
        let project = project(&service, "Alpha").await;
        let todo = status_named(&service, project.id, "To do").await;
        let done = status_named(&service, project.id, "Done").await;
        let issue = issue(&service, project.id, todo).await;

        let moved = service
            .update_issue(
                issue.id,
                serde_json::from_value(json!({ "status_id": done })).unwrap(),
            )
            .await
            .unwrap();
        assert!(moved.completed_at.is_some());

        let reopened = service
            .update_issue(
                issue.id,
                serde_json::from_value(json!({ "status_id": todo, "description": null })).unwrap(),
            )
            .await
            .unwrap();
        assert!(reopened.completed_at.is_none());
        assert_eq!(reopened.description, None);
    }

    #[tokio::test]
    async fn agent_runs_drive_issue_status() {
        let service = service().await;
        let project = project(&service, "Alpha").await;
        let todo = status_named(&service, project.id, "To do").await;
        let issue = issue(&service, project.id, todo).await;
        let ws = workspace(&service).await;

        // Unlinked workspaces never touch issues.
        service.on_coding_agent_started(ws).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "To do");

        service.link_workspace(ws, issue.id).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "To do");

        let process = coding_agent(&service, ws, "running").await;
        service.on_coding_agent_started(ws).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "In progress");

        // Still running somewhere: stays In progress.
        service.on_coding_agent_finished(ws).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "In progress");

        sqlx::query("UPDATE execution_processes SET status = 'completed' WHERE id = ?")
            .bind(process)
            .execute(&service.pool)
            .await
            .unwrap();
        service.on_coding_agent_finished(ws).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "In review");

        // A follow-up run reopens the work.
        service.on_coding_agent_started(ws).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "In progress");

        service.on_workspace_merged(ws).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "Done");
        assert!(
            service
                .get_issue(issue.id)
                .await
                .unwrap()
                .completed_at
                .is_some()
        );

        // Finished issues are not reopened by later runs.
        service.on_coding_agent_started(ws).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "Done");
    }

    #[tokio::test]
    async fn linking_a_running_workspace_starts_the_issue() {
        let service = service().await;
        let project = project(&service, "Alpha").await;
        let backlog = status_named(&service, project.id, "Backlog").await;
        let issue = issue(&service, project.id, backlog).await;
        let ws = workspace(&service).await;
        coding_agent(&service, ws, "running").await;

        service.link_workspace(ws, issue.id).await.unwrap();
        assert_eq!(status_name(&service, issue.id).await, "In progress");

        let linked = service.list_workspaces(Some(project.id)).await.unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].issue_id, Some(issue.id));
        assert_eq!(linked[0].owner_user_id, LOCAL_USER_ID);
        assert_eq!(
            service.get_workspace(ws).await.unwrap().project_id,
            project.id
        );
    }

    #[tokio::test]
    async fn pull_requests_drive_issue_status() {
        let service = service().await;
        let project = project(&service, "Alpha").await;
        let in_progress = status_named(&service, project.id, "In progress").await;
        let issue = issue(&service, project.id, in_progress).await;
        let ws = workspace(&service).await;
        service.link_workspace(ws, issue.id).await.unwrap();

        for (n, status) in [(1, "open"), (2, "open")] {
            sqlx::query(
                "INSERT INTO pull_requests (id, workspace_id, pr_url, pr_number, pr_status, \
                 target_branch_name) VALUES (?, ?, ?, ?, ?, 'main')",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(ws)
            .bind(format!("https://example.com/pr/{n}"))
            .bind(n)
            .bind(status)
            .execute(&service.pool)
            .await
            .unwrap();
        }

        service
            .on_pull_request_status(ws, PullRequestStatus::Open)
            .await
            .unwrap();
        assert_eq!(status_name(&service, issue.id).await, "In review");
        assert_eq!(
            service
                .list_pull_requests_for_project(project.id)
                .await
                .unwrap()
                .len(),
            2
        );

        sqlx::query("UPDATE pull_requests SET pr_status = 'merged' WHERE pr_number = 1")
            .execute(&service.pool)
            .await
            .unwrap();
        service
            .on_pull_request_status(ws, PullRequestStatus::Merged)
            .await
            .unwrap();
        assert_eq!(
            status_name(&service, issue.id).await,
            "In review",
            "one PR is still open"
        );

        sqlx::query("UPDATE pull_requests SET pr_status = 'merged'")
            .execute(&service.pool)
            .await
            .unwrap();
        service
            .on_pull_request_status(ws, PullRequestStatus::Merged)
            .await
            .unwrap();
        assert_eq!(status_name(&service, issue.id).await, "Done");
    }

    #[tokio::test]
    async fn deleting_a_project_removes_its_board_and_unlinks_workspaces() {
        let service = service().await;
        let project = project(&service, "Alpha").await;
        let todo = status_named(&service, project.id, "To do").await;
        let issue = issue(&service, project.id, todo).await;
        let tag = service
            .create_tag(CreateTagRequest {
                id: None,
                project_id: project.id,
                name: "bug".to_string(),
                color: "0 84% 60%".to_string(),
            })
            .await
            .unwrap();
        service
            .create_issue_tag(CreateIssueTagRequest {
                id: None,
                issue_id: issue.id,
                tag_id: tag.id,
            })
            .await
            .unwrap();
        let ws = workspace(&service).await;
        service.link_workspace(ws, issue.id).await.unwrap();

        service.delete_project(project.id).await.unwrap();

        assert!(service.list_projects().await.unwrap().is_empty());
        assert!(service.list_statuses(project.id).await.unwrap().is_empty());
        assert!(service.list_workspaces(None).await.unwrap().is_empty());
        let (task_id,): (Option<Uuid>,) =
            sqlx::query_as("SELECT task_id FROM workspaces WHERE id = ?")
                .bind(ws)
                .fetch_one(&service.pool)
                .await
                .unwrap();
        assert_eq!(task_id, None);
    }

    #[tokio::test]
    async fn search_filters_sorts_and_pages() {
        let service = service().await;
        let project = project(&service, "Alpha").await;
        let todo = status_named(&service, project.id, "To do").await;
        let done = status_named(&service, project.id, "Done").await;
        let first = issue(&service, project.id, todo).await;
        let second = issue(&service, project.id, done).await;
        service
            .update_issue(
                second.id,
                serde_json::from_value(json!({ "title": "Fix login", "sort_order": 0.5 })).unwrap(),
            )
            .await
            .unwrap();

        let request: SearchIssuesRequest =
            serde_json::from_value(json!({ "project_id": project.id, "search": "login" })).unwrap();
        let (found, total) = service.search_issues(&request).await.unwrap();
        assert_eq!((total, found[0].id), (1, second.id));

        let request: SearchIssuesRequest = serde_json::from_value(
            json!({ "project_id": project.id, "status_ids": [todo], "limit": 5 }),
        )
        .unwrap();
        let (found, _) = service.search_issues(&request).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, first.id);

        let request: SearchIssuesRequest =
            serde_json::from_value(json!({ "project_id": project.id })).unwrap();
        let (found, total) = service.search_issues(&request).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(found[0].id, second.id, "sorted by sort_order");
    }
}
