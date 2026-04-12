//! Auto-Thor: automatic wave validation when all tasks reach terminal state.

use std::sync::Arc;

use crate::plan_routes::PlanState;

/// When all tasks in a wave are terminal (submitted/done/cancelled/skipped),
/// automatically trigger Thor validation for the wave.
pub async fn try_auto_thor(state: &Arc<PlanState>, task_id: i64) {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    // Find the wave_id and plan_id for this task
    let row: Option<(i64, i64)> = conn
        .query_row(
            "SELECT wave_id, plan_id FROM tasks WHERE id = ?1",
            rusqlite::params![task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((wave_id, plan_id)) = row else {
        return;
    };
    // Check if any non-terminal tasks remain in this wave
    let pending: i64 = conn
        .query_row(
            "SELECT count(*) FROM tasks WHERE wave_id = ?1 AND plan_id = ?2 \
             AND status NOT IN ('submitted','done','cancelled','skipped')",
            rusqlite::params![wave_id, plan_id],
            |r| r.get(0),
        )
        .unwrap_or(1);
    if pending > 0 {
        return;
    }
    drop(conn);
    // All tasks in wave are terminal — trigger Thor validation
    tracing::info!(
        wave_id,
        plan_id,
        "Auto-Thor triggered for wave {wave_id} in plan {plan_id}"
    );
    if let Err(e) = crate::handlers::on_wave_validated(
        &state.pool,
        &state.notify,
        &state.event_sink,
        wave_id,
        plan_id,
    ) {
        tracing::error!(wave_id, plan_id, error = %e, "Auto-Thor validation failed");
    }
}
