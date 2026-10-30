use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use super::execution_process::{ExecutionProcessRunReason, ExecutionProcessStatus};

/// Read markers older than this are pruned; inbox items never look back further.
pub const INBOX_RETENTION_DAYS: i64 = 30;

/// A finished or failed run in a non-archived workspace.
#[derive(Debug, Clone, FromRow)]
pub struct InboxRunRow {
    pub execution_process_id: Uuid,
    pub status: ExecutionProcessStatus,
    pub run_reason: ExecutionProcessRunReason,
    pub completed_at: DateTime<Utc>,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    /// `None` when the run has no coding agent turn (scripts).
    pub seen: Option<i64>,
    pub summary: Option<String>,
}

/// A direct merge (`merge_id`) or a merged pull request (`pull_request_id`).
#[derive(Debug, Clone, FromRow)]
pub struct InboxMergeRow {
    pub merge_id: Option<Uuid>,
    pub pull_request_id: Option<String>,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub pr_number: Option<i64>,
    pub target_branch_name: String,
    pub merged_at: DateTime<Utc>,
}

pub struct AgentInbox;

fn sqlite_time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M:%S").to_string()
}

impl AgentInbox {
    pub async fn recent_runs(
        pool: &SqlitePool,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<InboxRunRow>, sqlx::Error> {
        sqlx::query_as(
            r#"SELECT ep.id AS execution_process_id,
                      ep.status AS status,
                      ep.run_reason AS run_reason,
                      ep.completed_at AS completed_at,
                      w.id AS workspace_id,
                      COALESCE(NULLIF(w.name, ''), w.branch) AS workspace_name,
                      (SELECT MIN(cat.seen) FROM coding_agent_turns cat
                        WHERE cat.execution_process_id = ep.id) AS seen,
                      (SELECT cat.summary FROM coding_agent_turns cat
                        WHERE cat.execution_process_id = ep.id
                        ORDER BY cat.created_at DESC LIMIT 1) AS summary
               FROM execution_processes ep
               JOIN sessions s ON s.id = ep.session_id
               JOIN workspaces w ON w.id = s.workspace_id
               WHERE w.archived = 0
                 AND ep.dropped = 0
                 AND ep.completed_at IS NOT NULL
                 AND datetime(ep.completed_at) >= datetime(?)
                 AND (
                     (ep.run_reason = 'codingagent' AND ep.status IN ('completed', 'failed'))
                     OR (ep.run_reason IN ('setupscript', 'cleanupscript') AND ep.status = 'failed')
                 )
               ORDER BY datetime(ep.completed_at) DESC
               LIMIT ?"#,
        )
        .bind(sqlite_time(since))
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    pub async fn recent_merges(
        pool: &SqlitePool,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<InboxMergeRow>, sqlx::Error> {
        sqlx::query_as(
            r#"SELECT merge_id, pull_request_id, workspace_id, workspace_name, pr_number,
                      target_branch_name, merged_at
               FROM (
                   SELECT m.id AS merge_id,
                          NULL AS pull_request_id,
                          w.id AS workspace_id,
                          COALESCE(NULLIF(w.name, ''), w.branch) AS workspace_name,
                          NULL AS pr_number,
                          m.target_branch_name AS target_branch_name,
                          m.created_at AS merged_at
                   FROM merges m
                   JOIN workspaces w ON w.id = m.workspace_id
                   WHERE m.merge_type = 'direct'
                   UNION ALL
                   SELECT NULL AS merge_id,
                          pr.id AS pull_request_id,
                          w.id AS workspace_id,
                          COALESCE(NULLIF(w.name, ''), w.branch) AS workspace_name,
                          pr.pr_number AS pr_number,
                          pr.target_branch_name AS target_branch_name,
                          COALESCE(pr.merged_at, pr.updated_at) AS merged_at
                   FROM pull_requests pr
                   JOIN workspaces w ON w.id = pr.workspace_id
                   WHERE pr.pr_status = 'merged'
               )
               WHERE datetime(merged_at) >= datetime(?)
               ORDER BY datetime(merged_at) DESC
               LIMIT ?"#,
        )
        .bind(sqlite_time(since))
        .bind(limit)
        .fetch_all(pool)
        .await
    }

    pub async fn read_ids(pool: &SqlitePool) -> Result<HashSet<String>, sqlx::Error> {
        let ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM agent_inbox_reads")
            .fetch_all(pool)
            .await?;
        Ok(ids.into_iter().map(|(id,)| id).collect())
    }

    /// Mark items read. Run items also mark their coding agent turns seen so
    /// the workspace list stops flagging them.
    pub async fn mark_read(pool: &SqlitePool, ids: &[String]) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        let now = Utc::now();
        for id in ids {
            sqlx::query("INSERT OR IGNORE INTO agent_inbox_reads (id) VALUES (?)")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            if let Some(process_id) = id
                .strip_prefix("run:")
                .and_then(|raw| Uuid::parse_str(raw).ok())
            {
                sqlx::query(
                    "UPDATE coding_agent_turns SET seen = 1, updated_at = ?
                     WHERE execution_process_id = ? AND seen = 0",
                )
                .bind(now)
                .bind(process_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query("DELETE FROM agent_inbox_reads WHERE datetime(read_at) < datetime(?)")
            .bind(sqlite_time(now - Duration::days(INBOX_RETENTION_DAYS)))
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        pool
    }

    async fn insert_workspace(pool: &SqlitePool, name: &str, archived: bool) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces (id, branch, name, archived) VALUES (?, ?, ?, ?)")
            .bind(workspace_id)
            .bind(format!("jarvis/{name}"))
            .bind(name)
            .bind(archived)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions (id, workspace_id) VALUES (?, ?)")
            .bind(session_id)
            .bind(workspace_id)
            .execute(pool)
            .await
            .unwrap();
        (workspace_id, session_id)
    }

    async fn insert_run(
        pool: &SqlitePool,
        session_id: Uuid,
        run_reason: &str,
        status: &str,
        completed_at: Option<DateTime<Utc>>,
        turn_seen: Option<bool>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO execution_processes (id, session_id, run_reason, status, completed_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(session_id)
        .bind(run_reason)
        .bind(status)
        .bind(completed_at)
        .execute(pool)
        .await
        .unwrap();
        if let Some(seen) = turn_seen {
            sqlx::query(
                "INSERT INTO coding_agent_turns (id, execution_process_id, summary, seen)
                 VALUES (?, ?, 'Updated the README', ?)",
            )
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(seen)
            .execute(pool)
            .await
            .unwrap();
        }
        id
    }

    #[tokio::test]
    async fn recent_runs_lists_finished_and_failed_runs_newest_first() {
        let pool = test_pool().await;
        let (workspace_id, session) = insert_workspace(&pool, "Northwind", false).await;
        let now = Utc::now();
        let older = insert_run(
            &pool,
            session,
            "codingagent",
            "completed",
            Some(now - Duration::minutes(10)),
            Some(true),
        )
        .await;
        let newer = insert_run(
            &pool,
            session,
            "codingagent",
            "failed",
            Some(now - Duration::minutes(1)),
            Some(false),
        )
        .await;
        let failed_setup = insert_run(
            &pool,
            session,
            "setupscript",
            "failed",
            Some(now - Duration::minutes(5)),
            None,
        )
        .await;
        // Excluded: still running, killed, successful script, dev server, too old.
        insert_run(&pool, session, "codingagent", "running", None, Some(false)).await;
        insert_run(
            &pool,
            session,
            "codingagent",
            "killed",
            Some(now),
            Some(false),
        )
        .await;
        insert_run(&pool, session, "setupscript", "completed", Some(now), None).await;
        insert_run(&pool, session, "devserver", "failed", Some(now), None).await;
        insert_run(
            &pool,
            session,
            "codingagent",
            "completed",
            Some(now - Duration::days(40)),
            Some(false),
        )
        .await;

        let rows = AgentInbox::recent_runs(&pool, now - Duration::days(7), 50)
            .await
            .unwrap();
        let ids: Vec<Uuid> = rows.iter().map(|r| r.execution_process_id).collect();
        assert_eq!(ids, vec![newer, failed_setup, older]);
        assert_eq!(rows[0].workspace_id, workspace_id);
        assert_eq!(rows[0].workspace_name, "Northwind");
        assert_eq!(rows[0].status, ExecutionProcessStatus::Failed);
        assert_eq!(rows[0].seen, Some(0));
        assert_eq!(rows[0].summary.as_deref(), Some("Updated the README"));
        assert_eq!(rows[1].seen, None);
        assert_eq!(rows[2].seen, Some(1));
    }

    #[tokio::test]
    async fn recent_runs_skips_archived_workspaces() {
        let pool = test_pool().await;
        let (_, session) = insert_workspace(&pool, "Archived", true).await;
        insert_run(
            &pool,
            session,
            "codingagent",
            "completed",
            Some(Utc::now()),
            Some(false),
        )
        .await;

        let rows = AgentInbox::recent_runs(&pool, Utc::now() - Duration::days(1), 50)
            .await
            .unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn recent_merges_include_direct_merges_and_merged_prs() {
        let pool = test_pool().await;
        let (workspace_id, _) = insert_workspace(&pool, "Northwind", false).await;
        let merge_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO merges (id, workspace_id, merge_type, merge_commit, target_branch_name)
             VALUES (?, ?, 'direct', 'abc123', 'main')",
        )
        .bind(merge_id)
        .bind(workspace_id)
        .execute(&pool)
        .await
        .unwrap();
        for (id, status, number) in [("pr-merged", "merged", 7), ("pr-open", "open", 8)] {
            sqlx::query(
                "INSERT INTO pull_requests (id, workspace_id, pr_url, pr_number, pr_status,
                                            target_branch_name, merged_at)
                 VALUES (?, ?, ?, ?, ?, 'main', ?)",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(format!("https://example.com/pr/{number}"))
            .bind(number)
            .bind(status)
            .bind(Utc::now() + Duration::seconds(5))
            .execute(&pool)
            .await
            .unwrap();
        }

        let rows = AgentInbox::recent_merges(&pool, Utc::now() - Duration::days(1), 50)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pull_request_id.as_deref(), Some("pr-merged"));
        assert_eq!(rows[0].pr_number, Some(7));
        assert_eq!(rows[0].merge_id, None);
        assert_eq!(rows[1].merge_id, Some(merge_id));
        assert_eq!(rows[1].pull_request_id, None);
        assert_eq!(rows[1].target_branch_name, "main");
    }

    #[tokio::test]
    async fn mark_read_records_ids_and_marks_run_turns_seen() {
        let pool = test_pool().await;
        let (_, session) = insert_workspace(&pool, "Northwind", false).await;
        let run = insert_run(
            &pool,
            session,
            "codingagent",
            "completed",
            Some(Utc::now()),
            Some(false),
        )
        .await;

        let ids = vec![format!("run:{run}"), "merge:other".to_string()];
        AgentInbox::mark_read(&pool, &ids).await.unwrap();
        // Idempotent.
        AgentInbox::mark_read(&pool, &ids).await.unwrap();

        let read = AgentInbox::read_ids(&pool).await.unwrap();
        assert!(read.contains(&ids[0]));
        assert!(read.contains(&ids[1]));

        let rows = AgentInbox::recent_runs(&pool, Utc::now() - Duration::days(1), 50)
            .await
            .unwrap();
        assert_eq!(rows[0].seen, Some(1));
    }
}
