//! Autonomous plan executor — background loop that picks up pending tasks
//! and spawns real agents to execute them.
//!
//! Safety features:
//! - On boot, stale in_progress plans (no heartbeat >15 min) are paused
//! - Active plans with recent heartbeats or assigned agents are left running
//! - Tasks with 3+ spawn failures are marked as `failed`
//! - Plans stale for 24h+ with no progress are marked `stale`

use convergio_db::pool::ConnPool;
use rusqlite::params;

const EXECUTOR_INTERVAL_SECS: u64 = 30;
const DAEMON_BASE: &str = "http://localhost:8420";
const MAX_SPAWN_FAILURES: i64 = 3;
const STALE_HOURS: i64 = 24;
/// Rate limiter: max spawns per executor tick across all plans.
const MAX_SPAWNS_PER_TICK: usize = 3;
/// Cooldown: skip tick if a recent spawn failed (backoff seconds).
const SPAWN_FAILURE_COOLDOWN_SECS: u64 = 60;
/// Spawn the plan executor background loop.
/// On first tick, pauses any in_progress plans left over from a previous run.
pub fn spawn_plan_executor_loop(pool: ConnPool) {
    tokio::spawn(async move {
        tracing::info!("plan_executor: loop started (interval={EXECUTOR_INTERVAL_SECS}s)");
        ensure_spawn_failures_column(&pool);
        pause_plans_on_boot(&pool);
        mark_stale_plans(&pool);
        let mut last_failure: Option<std::time::Instant> = None;
        loop {
            // Backoff: if last tick had a spawn failure, wait extra
            if let Some(ts) = last_failure {
                if ts.elapsed().as_secs() < SPAWN_FAILURE_COOLDOWN_SECS {
                    tracing::debug!("plan_executor: cooldown active, skipping tick");
                    tokio::time::sleep(std::time::Duration::from_secs(EXECUTOR_INTERVAL_SECS))
                        .await;
                    continue;
                }
                last_failure = None;
            }
            match executor_tick(&pool).await {
                Ok(had_failure) => {
                    if had_failure {
                        last_failure = Some(std::time::Instant::now());
                    }
                }
                Err(e) => {
                    tracing::warn!("plan_executor: tick error: {e}");
                    last_failure = Some(std::time::Instant::now());
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(EXECUTOR_INTERVAL_SECS)).await;
        }
    });
}

/// Ensure spawn_failures column exists (idempotent).
fn ensure_spawn_failures_column(pool: &ConnPool) {
    let Ok(conn) = pool.get() else { return };
    let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN spawn_failures INTEGER DEFAULT 0;");
}

/// On boot: pause only STALE in_progress plans (no task activity in 15+ minutes).
/// Plans with recent task updates, active agents, or manual assignees are left
/// running — prevents "start plan → daemon restart → plan paused" (#666, #867,
/// #872, #984).
fn pause_plans_on_boot(pool: &ConnPool) {
    let Ok(conn) = pool.get() else { return };
    // Only pause plans where NEITHER the plan NOR any task was updated recently,
    // and no tasks have active agents or manual assignment.
    let updated = conn
        .execute(
            "UPDATE plans SET status = 'paused' \
             WHERE status = 'in_progress' \
               AND updated_at < datetime('now', '-15 minutes') \
               AND id NOT IN ( \
                 SELECT DISTINCT plan_id FROM tasks \
                 WHERE updated_at > datetime('now', '-15 minutes') \
                   OR started_at > datetime('now', '-15 minutes') \
                   OR completed_at > datetime('now', '-15 minutes') \
                   OR (executor_agent IS NOT NULL \
                       AND status IN ('pending', 'in_progress')) \
               ) \
               AND id NOT IN ( \
                 SELECT DISTINCT plan_id FROM tasks \
                 WHERE executor_agent = 'manual' \
               )",
            [],
        )
        .unwrap_or(0);
    if updated > 0 {
        tracing::warn!(
            "plan_executor: BOOT SAFETY — paused {updated} stale plans (no activity >15min). \
             Use `cvg plan resume <id>` to continue."
        );
    }
}

/// Mark plans that have been in_progress for 24h+ without task completion as stale.
fn mark_stale_plans(pool: &ConnPool) {
    let Ok(conn) = pool.get() else { return };
    let stale = conn
        .execute(
            "UPDATE plans SET status = 'stale' \
             WHERE status = 'in_progress' \
               AND updated_at < datetime('now', ?1)",
            params![format!("-{STALE_HOURS} hours")],
        )
        .unwrap_or(0);
    if stale > 0 {
        tracing::warn!("plan_executor: marked {stale} stale plans (>{STALE_HOURS}h no progress)");
    }
}

/// One tick of the executor loop. Returns true if any spawn failed.
async fn executor_tick(pool: &ConnPool) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // Circuit breaker: halt plans with too many failed tasks
    check_circuit_breaker(pool);

    let tasks = find_pending_tasks(pool)?;
    if tasks.is_empty() {
        return Ok(false);
    }
    // Rate limit: cap spawns per tick to prevent runaway
    let batch_size = tasks.len().min(MAX_SPAWNS_PER_TICK);
    tracing::info!(
        "plan_executor: {} pending tasks, spawning up to {batch_size}",
        tasks.len()
    );
    let mut had_failure = false;
    for task in tasks.into_iter().take(batch_size) {
        if let Err(e) = executor_routing::spawn_real_agent(pool, &task).await {
            tracing::warn!(
                "plan_executor: failed to spawn agent for task {}: {e}",
                task.db_id
            );
            // Record error in task notes (#870)
            if let Ok(conn) = pool.get() {
                let _ = conn.execute(
                    "UPDATE tasks SET notes = COALESCE(notes, '') || char(10) || '[executor] Spawn error: ' || ?1, \
                     spawn_failures = COALESCE(spawn_failures, 0) + 1, \
                     updated_at = datetime('now') WHERE id = ?2",
                    params![e.to_string(), task.db_id],
                );
            }
            had_failure = true;
        }
    }
    // Advance completed waves and check plan completion (#869 #875)
    crate::wave_advance::advance_completed_waves(pool);
    Ok(had_failure)
}

/// Circuit breaker: if ALL auto-spawnable tasks in the current wave of a plan
/// have failed, mark the plan as failed. Excludes manual tasks (#984).
fn check_circuit_breaker(pool: &ConnPool) {
    let Ok(conn) = pool.get() else { return };
    // Find in_progress plans where the active wave has all NON-MANUAL tasks failed
    let mut stmt = match conn.prepare(
        "SELECT p.id, p.name \
         FROM plans p \
         WHERE p.status = 'in_progress' \
           AND EXISTS ( \
             SELECT 1 FROM waves w \
             WHERE w.plan_id = p.id AND w.status = 'in_progress' \
               AND NOT EXISTS ( \
                 SELECT 1 FROM tasks t \
                 WHERE t.wave_id = w.id \
                   AND t.status NOT IN ('failed', 'cancelled') \
                   AND COALESCE(t.executor_agent, '') != 'manual' \
               ) \
               AND EXISTS ( \
                 SELECT 1 FROM tasks t2 \
                 WHERE t2.wave_id = w.id AND t2.status = 'failed' \
                   AND COALESCE(t2.executor_agent, '') != 'manual' \
               ) \
           )",
    ) {
        Ok(s) => s,
        Err(_) => return,
    };
    let plans: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    for (plan_id, plan_name) in plans {
        tracing::error!(
            plan_id,
            plan_name = plan_name.as_str(),
            "CIRCUIT BREAKER: all tasks in active wave failed — halting plan"
        );
        let _ = conn.execute(
            "UPDATE plans SET status = 'failed', \
             updated_at = datetime('now') WHERE id = ?1",
            params![plan_id],
        );
    }
}

/// A pending task ready for execution.
pub(crate) struct PendingTask {
    pub(crate) db_id: i64,
    pub(crate) task_id: String,
    pub(crate) plan_id: i64,
    pub(crate) wave_db_id: i64,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) notes: String,
    pub(crate) required_capabilities: Option<String>,
    pub(crate) executor_agent: Option<String>,
}

/// Find tasks that are pending in waves that are in_progress.
fn find_pending_tasks(pool: &ConnPool) -> Result<Vec<PendingTask>, rusqlite::Error> {
    let conn = pool.get().map_err(|e| {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
    })?;
    crate::schema::ensure_required_capabilities_column(&conn)?;

    // Skip tasks with executor_agent = 'manual' — those wait for human/orchestrator (#984)
    let mut stmt = conn.prepare(
        "SELECT t.id, t.task_id, t.plan_id, t.wave_id, t.title, \
                COALESCE(t.description, ''), COALESCE(t.notes, ''), \
                t.required_capabilities, t.executor_agent \
         FROM tasks t \
         JOIN waves w ON w.id = t.wave_id \
         JOIN plans p ON p.id = t.plan_id \
         WHERE t.status = 'pending' \
           AND w.status = 'in_progress' \
           AND p.status = 'in_progress' \
           AND p.project_id != '_doctor_test_proj' \
           AND COALESCE(t.executor_agent, '') != 'manual' \
           AND w.id = (SELECT MIN(w2.id) FROM waves w2 \
                       WHERE w2.plan_id = t.plan_id AND w2.status = 'in_progress') \
         ORDER BY t.id ASC \
         LIMIT 5",
    )?;

    let tasks = stmt
        .query_map([], |r| {
            Ok(PendingTask {
                db_id: r.get(0)?,
                task_id: r.get(1)?,
                plan_id: r.get(2)?,
                wave_db_id: r.get(3)?,
                title: r.get(4)?,
                description: r.get(5)?,
                notes: r.get(6)?,
                required_capabilities: r.get(7)?,
                executor_agent: r.get(8)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tasks)
}

#[path = "task_instructions.rs"]
mod task_instructions;

#[path = "executor_routing.rs"]
mod executor_routing;

#[cfg(test)]
#[path = "plan_executor_tests.rs"]
mod tests;
