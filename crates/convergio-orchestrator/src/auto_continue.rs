//! Auto-continuation — orchestrator-side checkpoint + respawn integration.
//!
//! Plan Zero T3-01: detects when agents exit with checkpoints and ensures
//! the plan executor picks up the continuation automatically.
//!
//! The agent-runtime crate handles the actual respawn (respawn.rs).
//! This module handles the orchestrator side: detecting incomplete tasks
//! whose agents have stopped, and re-queuing them for the executor.

use convergio_db::pool::ConnPool;
use rusqlite::params;

/// Check for tasks where the agent stopped but the task is still in_progress.
/// Re-queues them for the plan executor to pick up (with or without respawn).
pub fn check_and_requeue(pool: &ConnPool) -> Result<u32, rusqlite::Error> {
    let conn = pool.get().map_err(|e| {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
    })?;

    // Find tasks where executor_agent is set, status is in_progress,
    // but the agent is no longer running (stage = stopped/failed/reaped)
    let mut stmt = conn.prepare(
        "SELECT t.id, t.executor_agent \
         FROM tasks t \
         JOIN art_agents a ON a.id = t.executor_agent \
         WHERE t.status = 'in_progress' \
           AND a.stage IN ('stopped', 'failed', 'reaped') \
           AND NOT EXISTS ( \
               SELECT 1 FROM art_agents c \
               WHERE c.parent_agent_id = a.id \
               AND c.stage IN ('spawning', 'running') \
           )",
    )?;

    let orphans: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let mut count = 0;
    for (task_id, agent_id) in &orphans {
        // Check if the agent has a checkpoint (continuation will use it)
        let has_checkpoint: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM art_context \
                 WHERE agent_id = ?1 AND key = 'checkpoint_state')",
                params![agent_id],
                |r| r.get(0),
            )
            .unwrap_or(false);

        if has_checkpoint {
            tracing::info!(
                "auto_continue: task {task_id} agent {agent_id} \
                 has checkpoint — respawn expected from agent-runtime"
            );
        } else {
            // No checkpoint, no continuation running — revert to pending
            tracing::warn!(
                "auto_continue: task {task_id} agent {agent_id} \
                 stopped without checkpoint — reverting to pending"
            );
            conn.execute(
                "UPDATE tasks SET status = 'pending', executor_agent = NULL, \
                 started_at = NULL WHERE id = ?1",
                params![task_id],
            )?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_pool() -> ConnPool {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        let mut all = crate::schema::migrations();
        all.extend(crate::schema_merge::merge_guardian_migrations());
        all.extend(crate::schema_wave_branch::wave_branch_migrations());
        all.sort_by_key(|m| m.version);
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
        // Create agent-runtime tables needed for joins
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS art_agents ( \
             id TEXT PRIMARY KEY, agent_name TEXT, org_id TEXT, task_id INTEGER, \
             stage TEXT DEFAULT 'spawning', workspace_path TEXT, model TEXT, \
             node TEXT, budget_usd REAL DEFAULT 10, spent_usd REAL DEFAULT 0, \
             priority INTEGER DEFAULT 0, parent_agent_id TEXT, \
             respawn_count INTEGER DEFAULT 0, max_respawns INTEGER DEFAULT 3, \
             created_at TEXT, updated_at TEXT); \
             CREATE TABLE IF NOT EXISTS art_context ( \
             agent_id TEXT, key TEXT, value TEXT, version INTEGER DEFAULT 1, \
             set_by TEXT, PRIMARY KEY(agent_id, key));",
        )
        .unwrap();
        pool
    }

    #[test]
    fn no_orphans_returns_zero() {
        let pool = setup_pool();
        let count = check_and_requeue(&pool).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn requeues_task_with_dead_agent_no_checkpoint() {
        let pool = setup_pool();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO plans (id, project_id, name, status) \
                 VALUES (1, 'test', 'P', 'in_progress')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO waves (id, wave_id, plan_id, name, status) \
                 VALUES (1, 'W1', 1, 'W', 'in_progress')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO art_agents (id, agent_name, org_id, node, stage) \
                 VALUES ('agent-1', 'test', 'org', 'n1', 'stopped')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, status, \
                 executor_agent, started_at) \
                 VALUES (1, 'T1', 1, 1, 'Task', 'in_progress', 'agent-1', \
                 datetime('now'))",
                [],
            )
            .unwrap();
        }
        let count = check_and_requeue(&pool).unwrap();
        assert_eq!(count, 1);
        let conn = pool.get().unwrap();
        let status: String = conn
            .query_row("SELECT status FROM tasks WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "pending");
    }
}
