use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{extract::State, response::Json as ResponseJson};
use chrono::{DateTime, Utc};
use db::models::{
    coding_agent_turn::CodingAgentTurn,
    execution_process::{ExecutionProcess, ExecutionProcessStatus},
    workspace::{Workspace, WorkspaceWithStatus},
};
use deployment::Deployment;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::workspaces::workspace_summary::{DiffStats, compute_workspace_diff_stats},
};

/// Attention state of a workspace, derived from its latest execution process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ControlCenterWorkspaceState {
    /// Agent (or setup/cleanup script) is running.
    Running,
    /// Agent is running but blocked on a tool approval or question.
    Waiting,
    /// Latest run completed and has turns the user has not seen yet.
    Review,
    /// Latest run failed or was killed.
    Failed,
    /// Nothing running and nothing new to look at.
    Idle,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
pub struct ControlCenterCounts {
    pub running: usize,
    pub waiting: usize,
    pub review: usize,
    pub failed: usize,
    pub idle: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ControlCenterRepoRef {
    pub repo_id: Uuid,
    pub name: String,
    pub target_branch: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ControlCenterWorkspace {
    pub workspace_id: Uuid,
    pub name: Option<String>,
    pub branch: String,
    pub repos: Vec<ControlCenterRepoRef>,
    pub state: ControlCenterWorkspaceState,
    pub executor: Option<String>,
    pub variant: Option<String>,
    pub model_id: Option<String>,
    pub pinned: bool,
    pub has_unseen_turns: bool,
    pub has_running_dev_server: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub latest_process_started_at: Option<DateTime<Utc>>,
    pub latest_process_completed_at: Option<DateTime<Utc>>,
    pub last_activity_at: DateTime<Utc>,
    /// Only computed for workspaces that are not idle.
    pub diff_stats: Option<DiffStats>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ControlCenterOverview {
    pub counts: ControlCenterCounts,
    /// Non-archived workspaces, most recently active first.
    pub workspaces: Vec<ControlCenterWorkspace>,
    pub generated_at: DateTime<Utc>,
}

pub(crate) fn derive_state(
    latest_status: Option<&ExecutionProcessStatus>,
    has_pending_approval: bool,
    has_unseen_turns: bool,
) -> ControlCenterWorkspaceState {
    match latest_status {
        Some(ExecutionProcessStatus::Running) if has_pending_approval => {
            ControlCenterWorkspaceState::Waiting
        }
        Some(ExecutionProcessStatus::Running) => ControlCenterWorkspaceState::Running,
        Some(ExecutionProcessStatus::Failed | ExecutionProcessStatus::Killed) => {
            ControlCenterWorkspaceState::Failed
        }
        Some(ExecutionProcessStatus::Completed) if has_unseen_turns => {
            ControlCenterWorkspaceState::Review
        }
        _ => ControlCenterWorkspaceState::Idle,
    }
}

impl ControlCenterCounts {
    fn add(&mut self, state: ControlCenterWorkspaceState) {
        self.total += 1;
        match state {
            ControlCenterWorkspaceState::Running => self.running += 1,
            ControlCenterWorkspaceState::Waiting => self.waiting += 1,
            ControlCenterWorkspaceState::Review => self.review += 1,
            ControlCenterWorkspaceState::Failed => self.failed += 1,
            ControlCenterWorkspaceState::Idle => self.idle += 1,
        }
    }
}

#[derive(Debug, FromRow)]
struct RepoRow {
    workspace_id: Uuid,
    repo_id: Uuid,
    name: String,
    target_branch: String,
}

#[derive(Debug, FromRow)]
struct AgentRow {
    workspace_id: Uuid,
    executor: Option<String>,
    variant: Option<String>,
    model_id: Option<String>,
}

async fn fetch_repo_refs(
    pool: &SqlitePool,
) -> Result<HashMap<Uuid, Vec<ControlCenterRepoRef>>, sqlx::Error> {
    let rows: Vec<RepoRow> = sqlx::query_as(
        r#"SELECT wr.workspace_id AS workspace_id,
                  r.id AS repo_id,
                  r.display_name AS name,
                  wr.target_branch AS target_branch
           FROM workspace_repos wr
           JOIN repos r ON r.id = wr.repo_id
           JOIN workspaces w ON w.id = wr.workspace_id
           WHERE w.archived = 0
           ORDER BY r.display_name ASC"#,
    )
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<Uuid, Vec<ControlCenterRepoRef>> = HashMap::new();
    for row in rows {
        map.entry(row.workspace_id)
            .or_default()
            .push(ControlCenterRepoRef {
                repo_id: row.repo_id,
                name: row.name,
                target_branch: row.target_branch,
            });
    }
    Ok(map)
}

/// Executor/variant/model of each workspace's latest coding agent run.
async fn fetch_agent_info(pool: &SqlitePool) -> Result<HashMap<Uuid, AgentRow>, sqlx::Error> {
    let rows: Vec<AgentRow> = sqlx::query_as(
        r#"SELECT workspace_id, executor, variant, model_id
           FROM (
               SELECT s.workspace_id AS workspace_id,
                      COALESCE(
                          json_extract(ep.executor_action, '$.typ.executor_config.executor'),
                          json_extract(ep.executor_action, '$.typ.executor_profile_id.executor'),
                          s.executor
                      ) AS executor,
                      COALESCE(
                          json_extract(ep.executor_action, '$.typ.executor_config.variant'),
                          json_extract(ep.executor_action, '$.typ.executor_profile_id.variant')
                      ) AS variant,
                      json_extract(ep.executor_action, '$.typ.executor_config.model_id') AS model_id,
                      ROW_NUMBER() OVER (
                          PARTITION BY s.workspace_id
                          ORDER BY ep.created_at DESC
                      ) AS rn
               FROM execution_processes ep
               JOIN sessions s ON ep.session_id = s.id
               JOIN workspaces w ON s.workspace_id = w.id
               WHERE w.archived = 0
                 AND ep.run_reason = 'codingagent'
                 AND ep.dropped = 0
           )
           WHERE rn = 1"#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| (r.workspace_id, r)).collect())
}

const DIFF_CACHE_TTL: Duration = Duration::from_secs(30);
const MAX_DIFF_COMPUTATIONS: usize = 24;

static DIFF_CACHE: LazyLock<Mutex<HashMap<Uuid, (Instant, DiffStats)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cached_diff(id: Uuid) -> Option<DiffStats> {
    let cache = DIFF_CACHE.lock().ok()?;
    cache
        .get(&id)
        .filter(|(at, _)| at.elapsed() < DIFF_CACHE_TTL)
        .map(|(_, stats)| stats.clone())
}

async fn diff_stats_for(
    deployment: &DeploymentImpl,
    workspaces: &[&Workspace],
) -> HashMap<Uuid, DiffStats> {
    let mut result = HashMap::new();
    let mut to_compute = Vec::new();
    for ws in workspaces {
        match cached_diff(ws.id) {
            Some(stats) => {
                result.insert(ws.id, stats);
            }
            None if ws.container_ref.is_some() && !ws.worktree_deleted => {
                to_compute.push((*ws).clone());
            }
            None => {}
        }
    }
    to_compute.truncate(MAX_DIFF_COMPUTATIONS);

    let computed = futures_util::future::join_all(to_compute.into_iter().map(|ws| {
        let deployment = deployment.clone();
        async move {
            compute_workspace_diff_stats(&deployment, &ws)
                .await
                .map(|stats| (ws.id, stats))
        }
    }))
    .await;

    if let Ok(mut cache) = DIFF_CACHE.lock() {
        cache.retain(|_, (at, _)| at.elapsed() < DIFF_CACHE_TTL);
        for (id, stats) in computed.iter().flatten() {
            cache.insert(*id, (Instant::now(), stats.clone()));
        }
    }
    result.extend(computed.into_iter().flatten());
    result
}

pub async fn get_overview(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ControlCenterOverview>>, ApiError> {
    let pool = &deployment.db().pool;

    let workspaces: Vec<WorkspaceWithStatus> =
        Workspace::find_all_with_status(pool, Some(false), None).await?;
    let latest_processes = ExecutionProcess::find_latest_for_workspaces(pool, false).await?;
    let dev_servers =
        ExecutionProcess::find_workspaces_with_running_dev_servers(pool, false).await?;
    let unseen: HashSet<Uuid> = CodingAgentTurn::find_workspaces_with_unseen(pool, false).await?;
    let repo_refs = fetch_repo_refs(pool).await?;
    let agent_info = fetch_agent_info(pool).await?;

    let running_ep_ids: Vec<Uuid> = latest_processes
        .values()
        .filter(|info| info.status == ExecutionProcessStatus::Running)
        .map(|info| info.execution_process_id)
        .collect();
    let pending_approval = deployment
        .approvals()
        .get_pending_execution_process_ids(&running_ep_ids);

    let mut started_at: HashMap<Uuid, DateTime<Utc>> = HashMap::new();
    for ep_id in &running_ep_ids {
        if let Some(ep) = ExecutionProcess::find_by_id(pool, *ep_id).await? {
            started_at.insert(*ep_id, ep.started_at);
        }
    }

    let states: HashMap<Uuid, ControlCenterWorkspaceState> = workspaces
        .iter()
        .map(|ws| {
            let latest = latest_processes.get(&ws.id);
            let state = derive_state(
                latest.map(|p| &p.status),
                latest.is_some_and(|p| pending_approval.contains(&p.execution_process_id)),
                unseen.contains(&ws.id),
            );
            (ws.id, state)
        })
        .collect();

    let active: Vec<&Workspace> = workspaces
        .iter()
        .filter(|ws| states.get(&ws.id) != Some(&ControlCenterWorkspaceState::Idle))
        .map(|ws| &ws.workspace)
        .collect();
    let diffs = diff_stats_for(&deployment, &active).await;

    let mut counts = ControlCenterCounts::default();
    let mut items: Vec<ControlCenterWorkspace> = workspaces
        .into_iter()
        .map(|ws| {
            let ws = ws.workspace;
            let state = states
                .get(&ws.id)
                .copied()
                .unwrap_or(ControlCenterWorkspaceState::Idle);
            counts.add(state);
            let latest = latest_processes.get(&ws.id);
            let completed_at = latest.and_then(|p| p.completed_at);
            let process_started =
                latest.and_then(|p| started_at.get(&p.execution_process_id).copied());
            let last_activity_at = [Some(ws.updated_at), completed_at, process_started]
                .into_iter()
                .flatten()
                .max()
                .unwrap_or(ws.updated_at);
            let agent = agent_info.get(&ws.id);

            ControlCenterWorkspace {
                workspace_id: ws.id,
                name: ws.name,
                branch: ws.branch,
                repos: repo_refs.get(&ws.id).cloned().unwrap_or_default(),
                state,
                executor: agent.and_then(|a| a.executor.clone()),
                variant: agent.and_then(|a| a.variant.clone()),
                model_id: agent.and_then(|a| a.model_id.clone()),
                pinned: ws.pinned,
                has_unseen_turns: unseen.contains(&ws.id),
                has_running_dev_server: dev_servers.contains(&ws.id),
                created_at: ws.created_at,
                updated_at: ws.updated_at,
                latest_process_started_at: process_started,
                latest_process_completed_at: completed_at,
                last_activity_at,
                diff_stats: diffs.get(&ws.id).cloned(),
            }
        })
        .collect();
    items.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));

    Ok(ResponseJson(ApiResponse::success(ControlCenterOverview {
        counts,
        workspaces: items,
        generated_at: Utc::now(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_states_from_latest_process() {
        use ControlCenterWorkspaceState::*;
        use ExecutionProcessStatus as S;

        assert_eq!(derive_state(Some(&S::Running), false, false), Running);
        assert_eq!(derive_state(Some(&S::Running), true, true), Waiting);
        assert_eq!(derive_state(Some(&S::Failed), false, true), Failed);
        assert_eq!(derive_state(Some(&S::Killed), false, false), Failed);
        assert_eq!(derive_state(Some(&S::Completed), false, true), Review);
        assert_eq!(derive_state(Some(&S::Completed), false, false), Idle);
        assert_eq!(derive_state(None, false, true), Idle);
    }

    #[test]
    fn counts_each_state() {
        let mut counts = ControlCenterCounts::default();
        for state in [
            ControlCenterWorkspaceState::Running,
            ControlCenterWorkspaceState::Running,
            ControlCenterWorkspaceState::Waiting,
            ControlCenterWorkspaceState::Failed,
            ControlCenterWorkspaceState::Idle,
        ] {
            counts.add(state);
        }
        assert_eq!(counts.running, 2);
        assert_eq!(counts.waiting, 1);
        assert_eq!(counts.review, 0);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.idle, 1);
        assert_eq!(counts.total, 5);
    }
}
