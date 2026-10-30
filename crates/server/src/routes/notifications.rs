//! Local agent inbox and live alert stream. Everything here is derived from
//! this machine's own data; alerts reach the browser over SSE because the
//! server may run on a headless host.

use std::{collections::HashSet, convert::Infallible};

use axum::{
    Json, Router,
    extract::{Query, State},
    response::{
        Json as ResponseJson, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use db::models::{
    agent_inbox::{AgentInbox, InboxMergeRow, InboxRunRow},
    execution_process::{ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus},
};
use deployment::Deployment;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use services::services::{
    approvals::ApprovalInfo,
    container::ContainerService,
    notification::{AgentAlert, AgentAlertKind},
};
use tokio::sync::broadcast::error::RecvError;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
/// Must stay below `INBOX_RETENTION_DAYS` so read markers outlive their items.
const INBOX_WINDOW_DAYS: i64 = 7;

#[derive(Debug, Clone, Serialize, TS)]
pub struct AgentInboxItem {
    pub id: String,
    pub kind: AgentAlertKind,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    /// Short human-readable detail (tool name, PR number, summary...).
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct AgentInboxResponse {
    /// Newest first.
    pub items: Vec<AgentInboxItem>,
    pub unread_count: usize,
}

#[derive(Debug, Deserialize, TS)]
pub struct MarkAgentInboxReadRequest {
    /// Item ids to mark read. Ignored when `all` is true.
    #[serde(default)]
    pub ids: Vec<String>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Deserialize)]
pub struct AgentInboxQuery {
    pub limit: Option<usize>,
}

/// A pending approval joined with its workspace.
pub(crate) struct PendingApprovalRow {
    pub info: ApprovalInfo,
    pub workspace_id: Uuid,
    pub workspace_name: String,
}

fn run_item(row: InboxRunRow, read_ids: &HashSet<String>) -> AgentInboxItem {
    let id = AgentAlert::run_id(row.execution_process_id);
    let failed = row.status == ExecutionProcessStatus::Failed;
    let detail = match row.run_reason {
        ExecutionProcessRunReason::SetupScript => Some("Setup script failed".to_string()),
        ExecutionProcessRunReason::CleanupScript => Some("Cleanup script failed".to_string()),
        _ => row.summary.filter(|s| !s.trim().is_empty()),
    };
    let read = read_ids.contains(&id) || row.seen.is_some_and(|seen| seen != 0);
    AgentInboxItem {
        id,
        kind: if failed {
            AgentAlertKind::Failed
        } else {
            AgentAlertKind::Finished
        },
        workspace_id: row.workspace_id,
        workspace_name: row.workspace_name,
        detail,
        created_at: row.completed_at,
        read,
    }
}

fn merge_item(row: InboxMergeRow, read_ids: &HashSet<String>) -> Option<AgentInboxItem> {
    let id = match (row.merge_id, &row.pull_request_id) {
        (Some(merge_id), _) => AgentAlert::merge_id(merge_id),
        (None, Some(pr_id)) => format!("pr:{pr_id}"),
        (None, None) => return None,
    };
    let detail = match row.pr_number {
        Some(number) => format!("PR #{number} merged into {}", row.target_branch_name),
        None => format!("Merged into {}", row.target_branch_name),
    };
    Some(AgentInboxItem {
        read: read_ids.contains(&id),
        id,
        kind: AgentAlertKind::Merged,
        workspace_id: row.workspace_id,
        workspace_name: row.workspace_name,
        detail: Some(detail),
        created_at: row.merged_at,
    })
}

fn approval_item(row: PendingApprovalRow, read_ids: &HashSet<String>) -> AgentInboxItem {
    let id = AgentAlert::approval_id(&row.info.approval_id);
    let (kind, detail) = if row.info.is_question {
        (
            AgentAlertKind::NeedsInput,
            "Question waiting for an answer".to_string(),
        )
    } else {
        (
            AgentAlertKind::NeedsApproval,
            format!("Tool '{}' requires approval", row.info.tool_name),
        )
    };
    AgentInboxItem {
        read: read_ids.contains(&id),
        id,
        kind,
        workspace_id: row.workspace_id,
        workspace_name: row.workspace_name,
        detail: Some(detail),
        created_at: row.info.created_at,
    }
}

pub(crate) fn build_inbox(
    runs: Vec<InboxRunRow>,
    merges: Vec<InboxMergeRow>,
    approvals: Vec<PendingApprovalRow>,
    read_ids: &HashSet<String>,
    limit: usize,
) -> AgentInboxResponse {
    let mut items: Vec<AgentInboxItem> = approvals
        .into_iter()
        .map(|row| approval_item(row, read_ids))
        .chain(runs.into_iter().map(|row| run_item(row, read_ids)))
        .chain(
            merges
                .into_iter()
                .filter_map(|row| merge_item(row, read_ids)),
        )
        .collect();
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items.truncate(limit);
    let unread_count = items.iter().filter(|item| !item.read).count();
    AgentInboxResponse {
        items,
        unread_count,
    }
}

async fn pending_approvals(deployment: &DeploymentImpl) -> Vec<PendingApprovalRow> {
    let pool = &deployment.db().pool;
    let mut rows = Vec::new();
    for info in deployment.approvals().pending_infos() {
        let Ok(ctx) = ExecutionProcess::load_context(pool, info.execution_process_id).await else {
            continue;
        };
        if ctx.workspace.archived {
            continue;
        }
        let workspace_name = ctx
            .workspace
            .name
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| ctx.workspace.branch.clone());
        rows.push(PendingApprovalRow {
            info,
            workspace_id: ctx.workspace.id,
            workspace_name,
        });
    }
    rows
}

async fn load_inbox(
    deployment: &DeploymentImpl,
    limit: usize,
) -> Result<AgentInboxResponse, ApiError> {
    let pool = &deployment.db().pool;
    let since = Utc::now() - Duration::days(INBOX_WINDOW_DAYS);
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX);
    let runs = AgentInbox::recent_runs(pool, since, fetch).await?;
    let merges = AgentInbox::recent_merges(pool, since, fetch).await?;
    let approvals = pending_approvals(deployment).await;
    let read_ids = AgentInbox::read_ids(pool).await?;
    Ok(build_inbox(runs, merges, approvals, &read_ids, limit))
}

async fn get_inbox(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<AgentInboxQuery>,
) -> Result<ResponseJson<ApiResponse<AgentInboxResponse>>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let inbox = load_inbox(&deployment, limit).await?;
    Ok(ResponseJson(ApiResponse::success(inbox)))
}

async fn mark_read(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<MarkAgentInboxReadRequest>,
) -> Result<ResponseJson<ApiResponse<AgentInboxResponse>>, ApiError> {
    let ids: Vec<String> = if request.all {
        load_inbox(&deployment, MAX_LIMIT)
            .await?
            .items
            .into_iter()
            .filter(|item| !item.read)
            .map(|item| item.id)
            .collect()
    } else {
        request.ids
    };
    AgentInbox::mark_read(&deployment.db().pool, &ids).await?;
    let inbox = load_inbox(&deployment, DEFAULT_LIMIT).await?;
    Ok(ResponseJson(ApiResponse::success(inbox)))
}

async fn stream_alerts(
    State(deployment): State<DeploymentImpl>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = deployment.container().notification_service().subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(alert) => match Event::default().event("alert").json_data(&alert) {
                    Ok(event) => return Some((Ok(event), rx)),
                    Err(e) => tracing::warn!("Failed to encode agent alert: {e}"),
                },
                Err(RecvError::Lagged(skipped)) => {
                    tracing::debug!("Agent alert stream lagged by {skipped}");
                }
                Err(RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub(super) fn router() -> Router<DeploymentImpl> {
    Router::new().nest(
        "/notifications",
        Router::new()
            .route("/", get(get_inbox))
            .route("/read", post(mark_read))
            .route("/stream", get(stream_alerts)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_row(status: ExecutionProcessStatus, seen: Option<i64>, minutes_ago: i64) -> InboxRunRow {
        InboxRunRow {
            execution_process_id: Uuid::new_v4(),
            status,
            run_reason: ExecutionProcessRunReason::CodingAgent,
            completed_at: Utc::now() - Duration::minutes(minutes_ago),
            workspace_id: Uuid::new_v4(),
            workspace_name: "Northwind Traders".to_string(),
            seen,
            summary: Some("Added tests".to_string()),
        }
    }

    fn approval_row(is_question: bool, minutes_ago: i64) -> PendingApprovalRow {
        let created_at = Utc::now() - Duration::minutes(minutes_ago);
        PendingApprovalRow {
            info: ApprovalInfo {
                approval_id: format!("approval-{minutes_ago}"),
                tool_name: "Bash".to_string(),
                execution_process_id: Uuid::new_v4(),
                is_question,
                created_at,
                timeout_at: created_at + Duration::hours(1),
            },
            workspace_id: Uuid::new_v4(),
            workspace_name: "Sample".to_string(),
        }
    }

    #[test]
    fn inbox_merges_sources_newest_first_and_counts_unread() {
        let finished_seen = run_row(ExecutionProcessStatus::Completed, Some(1), 30);
        let failed_unseen = run_row(ExecutionProcessStatus::Failed, Some(0), 5);
        let merge = InboxMergeRow {
            merge_id: None,
            pull_request_id: Some("pr-1".to_string()),
            workspace_id: Uuid::new_v4(),
            workspace_name: "Sample".to_string(),
            pr_number: Some(12),
            target_branch_name: "main".to_string(),
            merged_at: Utc::now() - Duration::minutes(10),
        };

        let inbox = build_inbox(
            vec![finished_seen, failed_unseen],
            vec![merge],
            vec![approval_row(false, 1), approval_row(true, 20)],
            &HashSet::new(),
            50,
        );

        let kinds: Vec<AgentAlertKind> = inbox.items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AgentAlertKind::NeedsApproval,
                AgentAlertKind::Failed,
                AgentAlertKind::Merged,
                AgentAlertKind::NeedsInput,
                AgentAlertKind::Finished,
            ]
        );
        assert_eq!(
            inbox.items[0].detail.as_deref(),
            Some("Tool 'Bash' requires approval")
        );
        assert_eq!(inbox.items[2].id, "pr:pr-1");
        assert_eq!(
            inbox.items[2].detail.as_deref(),
            Some("PR #12 merged into main")
        );
        assert!(inbox.items[4].read);
        assert_eq!(inbox.unread_count, 4);
    }

    #[test]
    fn read_markers_apply_to_every_kind() {
        let run = run_row(ExecutionProcessStatus::Completed, None, 3);
        let run_id = AgentAlert::run_id(run.execution_process_id);
        let approval = approval_row(false, 2);
        let approval_id = AgentAlert::approval_id(&approval.info.approval_id);
        let read: HashSet<String> = [run_id, approval_id].into_iter().collect();

        let inbox = build_inbox(vec![run], vec![], vec![approval], &read, 50);

        assert!(inbox.items.iter().all(|i| i.read));
        assert_eq!(inbox.unread_count, 0);
    }

    #[test]
    fn inbox_is_truncated_to_limit() {
        let runs = (0..5)
            .map(|i| run_row(ExecutionProcessStatus::Completed, Some(0), i))
            .collect();
        let inbox = build_inbox(runs, vec![], vec![], &HashSet::new(), 3);
        assert_eq!(inbox.items.len(), 3);
        assert_eq!(inbox.unread_count, 3);
    }

    #[test]
    fn failed_setup_script_has_explanatory_detail() {
        let mut row = run_row(ExecutionProcessStatus::Failed, None, 1);
        row.run_reason = ExecutionProcessRunReason::SetupScript;
        let item = run_item(row, &HashSet::new());
        assert_eq!(item.kind, AgentAlertKind::Failed);
        assert_eq!(item.detail.as_deref(), Some("Setup script failed"));
        assert!(!item.read);
    }
}
