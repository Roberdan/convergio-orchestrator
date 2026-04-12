//! Agent health monitoring — heartbeat check + timeout + stuck detection.
//!
//! Plan Zero T3-02: monitors in-progress tasks for agent health.
//! Works with the agent-runtime heartbeat + reaper systems.

use convergio_db::pool::ConnPool;
use rusqlite::params;

const HEALTH_CHECK_INTERVAL_SECS: u64 = 60;
const STUCK_THRESHOLD_SECS: i64 = 3600; // 1 hour without progress

/// Spawn the agent health monitoring loop.
pub fn spawn_health_monitor(pool: ConnPool) {
    tokio::spawn(async move {
        tracing::info!("agent_health: monitor started (interval={HEALTH_CHECK_INTERVAL_SECS}s)");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS)).await;
            if let Err(e) = health_tick(&pool) {
                tracing::warn!("agent_health: tick error: {e}");
            }
        }
    });
}

/// One tick of the health monitor.
fn health_tick(pool: &ConnPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check for orphaned tasks (agent stopped without completing)
    match crate::auto_continue::check_and_requeue(pool) {
        Ok(n) if n > 0 => tracing::info!("agent_health: requeued {n} orphaned tasks"),
        Err(e) => tracing::warn!("agent_health: auto_continue check failed: {e}"),
        _ => {}
    }

    let conn = pool.get()?;
    let stuck = find_stuck_tasks(&conn)?;
    if stuck.is_empty() {
        return Ok(());
    }
    tracing::warn!("agent_health: found {} stuck tasks", stuck.len());
    for (task_id, agent, elapsed) in &stuck {
        tracing::warn!("agent_health: task {task_id} agent={agent} stuck for {elapsed}s");
        handle_stuck_task(&conn, *task_id, agent)?;
    }
    Ok(())
}

/// Find tasks stuck in in_progress with no recent heartbeat.
fn find_stuck_tasks(
    conn: &rusqlite::Connection,
) -> Result<Vec<(i64, String, i64)>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT t.id, COALESCE(t.executor_agent, ''), \
         CAST((julianday('now') - julianday(t.started_at)) * 86400 AS INTEGER) AS elapsed \
         FROM tasks t \
         WHERE t.status = 'in_progress' \
           AND t.started_at IS NOT NULL \
           AND CAST((julianday('now') - julianday(t.started_at)) * 86400 AS INTEGER) > ?1 \
           AND NOT EXISTS ( \
               SELECT 1 FROM art_heartbeats h \
               WHERE h.agent_id = t.executor_agent \
               AND CAST((julianday('now') - julianday(h.last_seen)) * 86400 AS INTEGER) \
                   < (h.interval_s * 3) \
           )",
    )?;
    let rows = stmt
        .query_map(params![STUCK_THRESHOLD_SECS], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Handle a stuck task: revert to pending so the executor can retry.
fn handle_stuck_task(
    conn: &rusqlite::Connection,
    task_id: i64,
    agent: &str,
) -> Result<(), rusqlite::Error> {
    tracing::warn!("agent_health: reverting task {task_id} (agent={agent}) to pending for retry");
    conn.execute(
        "UPDATE tasks SET status = 'pending', executor_agent = NULL, \
         started_at = NULL WHERE id = ?1 AND status = 'in_progress'",
        params![task_id],
    )?;
    // Log the revert
    conn.execute(
        "INSERT INTO audit_log (action, entity_type, entity_id, actor, details) \
         VALUES ('stuck_revert', 'task', ?1, 'agent_health', ?2)",
        params![
            task_id,
            format!("agent {agent} stuck, reverting to pending")
        ],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "agent_health_tests.rs"]
mod tests;
