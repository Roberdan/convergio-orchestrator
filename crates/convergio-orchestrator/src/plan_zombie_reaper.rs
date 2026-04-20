//! Periodic reaper for stale plans.
//!
//! WHY: before this existed, plans would pile up in `in_progress` or
//! `paused` indefinitely. Operators manually cancelled or force-completed
//! them, which (combined with a lax wave-advance) produced the
//! "plan=done, tasks=5/28" zombies we discovered on 2026-04-20.
//! This module closes the loop: on a slow cron, plans that have not
//! made progress inside their SLA are moved to a terminal state.
//!
//! Rules:
//! - `in_progress` with no update for > 7 days → `failed`
//! - `paused`      with no update for > 14 days → `cancelled`
//! - `done`        with non-terminal tasks      → `failed` (repair state
//!   for zombies created before the integrity guard shipped)

use convergio_db::pool::ConnPool;
use rusqlite::params;

const STALE_IN_PROGRESS_DAYS: i64 = 7;
const STALE_PAUSED_DAYS: i64 = 14;

/// Counts of plans touched by the reaper in one cycle.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ZombieReapOutcome {
    pub failed_inprogress: usize,
    pub cancelled_paused: usize,
    pub repaired_done: usize,
}

impl ZombieReapOutcome {
    pub fn total(&self) -> usize {
        self.failed_inprogress + self.cancelled_paused + self.repaired_done
    }
}

/// Run one reaping pass. Safe to call concurrently; every statement is
/// a single atomic UPDATE on the `plans` table.
pub fn reap_zombie_plans(pool: &ConnPool) -> Result<ZombieReapOutcome, rusqlite::Error> {
    let conn = pool
        .get()
        .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;

    let failed_inprogress = conn.execute(
        "UPDATE plans SET status='failed', updated_at=datetime('now') \
         WHERE status='in_progress' \
         AND updated_at < datetime('now', ?1)",
        params![format!("-{STALE_IN_PROGRESS_DAYS} days")],
    )?;

    let cancelled_paused = conn.execute(
        "UPDATE plans SET status='cancelled', updated_at=datetime('now') \
         WHERE status='paused' \
         AND updated_at < datetime('now', ?1)",
        params![format!("-{STALE_PAUSED_DAYS} days")],
    )?;

    let repaired_done = conn.execute(
        "UPDATE plans SET status='failed', updated_at=datetime('now') \
         WHERE status='done' \
         AND EXISTS ( \
           SELECT 1 FROM tasks \
           WHERE tasks.plan_id = plans.id \
           AND tasks.status NOT IN ('done','submitted','cancelled','skipped') \
         )",
        [],
    )?;

    let out = ZombieReapOutcome {
        failed_inprogress,
        cancelled_paused,
        repaired_done,
    };

    if out.total() > 0 {
        tracing::info!(
            failed = out.failed_inprogress,
            cancelled = out.cancelled_paused,
            repaired = out.repaired_done,
            "plan_zombie_reaper: reaped stale plans"
        );
    }

    Ok(out)
}

/// Spawn the reaper on a 6-hour interval. The first pass runs after the
/// interval elapses (not at startup) to avoid racing other bootstrap work.
pub fn spawn_plan_zombie_reaper(pool: ConnPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            if let Err(e) = reap_zombie_plans(&pool) {
                tracing::warn!("plan_zombie_reaper error: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> ConnPool {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF; \
             CREATE TABLE plans(id INTEGER PRIMARY KEY, status TEXT, updated_at TEXT); \
             CREATE TABLE tasks(id INTEGER PRIMARY KEY, plan_id INTEGER, status TEXT);",
        )
        .unwrap();
        pool
    }

    #[test]
    fn stale_in_progress_is_failed() {
        let pool = setup();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO plans(id, status, updated_at) VALUES \
                 (1,'in_progress',datetime('now','-8 days')), \
                 (2,'in_progress',datetime('now','-1 days'))",
                [],
            )
            .unwrap();
        }
        let outcome = reap_zombie_plans(&pool).unwrap();
        assert_eq!(outcome.failed_inprogress, 1);
        assert_eq!(outcome.cancelled_paused, 0);
        assert_eq!(outcome.repaired_done, 0);
    }

    #[test]
    fn stale_paused_is_cancelled() {
        let pool = setup();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO plans(id, status, updated_at) VALUES \
                 (1,'paused',datetime('now','-15 days')), \
                 (2,'paused',datetime('now','-2 days'))",
                [],
            )
            .unwrap();
        }
        let outcome = reap_zombie_plans(&pool).unwrap();
        assert_eq!(outcome.cancelled_paused, 1);
    }

    #[test]
    fn done_with_open_tasks_is_repaired() {
        let pool = setup();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO plans(id, status, updated_at) VALUES (1,'done',datetime('now'))",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks(plan_id, status) VALUES (1,'done'),(1,'pending')",
                [],
            )
            .unwrap();
        }
        let outcome = reap_zombie_plans(&pool).unwrap();
        assert_eq!(outcome.repaired_done, 1);
    }

    #[test]
    fn clean_plans_are_untouched() {
        let pool = setup();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO plans(id, status, updated_at) VALUES (1,'done',datetime('now'))",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO tasks(plan_id, status) VALUES (1,'done')", [])
                .unwrap();
        }
        let outcome = reap_zombie_plans(&pool).unwrap();
        assert_eq!(outcome.total(), 0);
    }
}
