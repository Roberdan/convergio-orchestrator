//! Plan sequencer — auto-start dependent plans when predecessor completes.
//!
//! Plan Zero T4-01: polls for unblocked plans and starts them automatically.
//! Uses existing plan_hierarchy::dependencies_met() for dependency checks.

use convergio_db::pool::ConnPool;
use rusqlite::params;

const SEQUENCER_INTERVAL_SECS: u64 = 30;

/// Spawn the plan sequencer background loop.
pub fn spawn_plan_sequencer(pool: ConnPool) {
    tokio::spawn(async move {
        tracing::info!("plan_sequencer: loop started (interval={SEQUENCER_INTERVAL_SECS}s)");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(SEQUENCER_INTERVAL_SECS)).await;
            if let Err(e) = sequencer_tick(&pool) {
                tracing::warn!("plan_sequencer: tick error: {e}");
            }
        }
    });
}

/// One tick of the sequencer loop.
fn sequencer_tick(pool: &ConnPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let conn = pool.get()?;

    // Find plans that are 'todo' or 'approved' with depends_on set
    let mut stmt = conn.prepare(
        "SELECT id, name, depends_on FROM plans \
         WHERE status IN ('todo', 'approved') \
           AND depends_on IS NOT NULL AND depends_on != ''",
    )?;
    let candidates: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();

    for (plan_id, plan_name, _deps) in &candidates {
        if crate::plan_hierarchy::dependencies_met(&conn, *plan_id)? {
            tracing::info!("plan_sequencer: plan {plan_id} ({plan_name}) unblocked — starting");
            start_plan(&conn, *plan_id)?;
        }
    }

    // Also find plans that are 'in_progress' but all tasks are done
    check_plan_completion(&conn)?;

    Ok(())
}

/// Start a plan: set status to in_progress, start first wave.
fn start_plan(conn: &rusqlite::Connection, plan_id: i64) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE plans SET status = 'in_progress', updated_at = datetime('now') \
         WHERE id = ?1 AND status IN ('todo', 'approved')",
        params![plan_id],
    )?;
    // Start first wave (with file overlap check)
    let first_wave: Option<i64> = conn
        .query_row(
            "SELECT id FROM waves WHERE plan_id = ?1 AND status = 'pending' \
             ORDER BY id ASC LIMIT 1",
            params![plan_id],
            |r| r.get(0),
        )
        .ok();
    if let Some(wave_id) = first_wave {
        if let Some(conflicts) = check_wave_file_conflicts(conn, plan_id, wave_id) {
            tracing::warn!("plan_sequencer: delaying wave {wave_id} — file overlap: {conflicts:?}");
            return Ok(()); // sequencer retries next tick (~30s)
        }
        conn.execute(
            "UPDATE waves SET status = 'in_progress', started_at = datetime('now') \
             WHERE id = ?1",
            params![wave_id],
        )?;
        tracing::info!("plan_sequencer: started wave {wave_id} for plan {plan_id}");
    }
    Ok(())
}

/// Check if a wave's tasks claim files that overlap with currently in-progress tasks.
fn check_wave_file_conflicts(
    conn: &rusqlite::Connection,
    plan_id: i64,
    wave_id: i64,
) -> Option<Vec<String>> {
    // Get claimed_files from this wave's tasks
    let wave_files: Vec<String> = conn
        .prepare(
            "SELECT claimed_files FROM tasks WHERE plan_id = ?1 AND wave_id = ?2 \
             AND claimed_files != '[]'",
        )
        .ok()?
        .query_map(params![plan_id, wave_id], |r| r.get::<_, String>(0))
        .ok()?
        .filter_map(|r| r.ok())
        .flat_map(|s| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default())
        .collect();
    if wave_files.is_empty() {
        return None;
    }
    // Check against in-progress tasks from OTHER plans/waves
    let active_files: Vec<String> = conn
        .prepare(
            "SELECT claimed_files FROM tasks WHERE status = 'in_progress' \
             AND NOT (plan_id = ?1 AND wave_id = ?2) AND claimed_files != '[]'",
        )
        .ok()?
        .query_map(params![plan_id, wave_id], |r| r.get::<_, String>(0))
        .ok()?
        .filter_map(|r| r.ok())
        .flat_map(|s| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default())
        .collect();
    let overlap: Vec<String> = wave_files
        .into_iter()
        .filter(|f| active_files.contains(f))
        .collect();
    if overlap.is_empty() {
        None
    } else {
        Some(overlap)
    }
}

/// Check if any in-progress plans have all tasks done/submitted.
fn check_plan_completion(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.name, \
         (SELECT COUNT(*) FROM tasks WHERE plan_id = p.id) as total, \
         (SELECT COUNT(*) FROM tasks WHERE plan_id = p.id \
          AND status IN ('done', 'submitted', 'cancelled', 'skipped')) as terminal \
         FROM plans p \
         WHERE p.status = 'in_progress'",
    )?;
    let plans: Vec<(i64, String, i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .filter_map(|r| r.ok())
        .collect();

    for (plan_id, plan_name, total, terminal) in &plans {
        if *total > 0 && *terminal == *total {
            tracing::info!(
                "plan_sequencer: plan {plan_id} ({plan_name}) all {total} tasks terminal"
            );
            // Update tasks_done count
            conn.execute(
                "UPDATE plans SET tasks_done = ?1, updated_at = datetime('now') \
                 WHERE id = ?2",
                params![terminal, plan_id],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "plan_sequencer_tests.rs"]
mod tests;
