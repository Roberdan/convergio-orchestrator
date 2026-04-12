//! Wave lifecycle: auto-complete waves, activate next, finalize plans.

use convergio_db::pool::ConnPool;

/// Advance waves: mark done when all tasks finished, activate next wave.
/// Also mark plans as done only when ALL waves are done (#875).
pub(crate) fn advance_completed_waves(pool: &ConnPool) {
    let Ok(conn) = pool.get() else { return };
    let waves_done = conn
        .execute(
            "UPDATE waves SET status = 'done', updated_at = datetime('now') \
             WHERE status = 'in_progress' \
             AND NOT EXISTS ( \
               SELECT 1 FROM tasks WHERE wave_id = waves.id \
               AND status IN ('pending', 'in_progress', 'submitted') \
             )",
            [],
        )
        .unwrap_or(0);
    if waves_done > 0 {
        tracing::info!("plan_executor: {waves_done} wave(s) completed, advancing");
    }
    // Activate next pending wave (one per plan, only if no wave is in_progress)
    let _ = conn.execute(
        "UPDATE waves SET status = 'in_progress', updated_at = datetime('now') \
         WHERE status = 'pending' \
         AND plan_id IN (SELECT id FROM plans WHERE status = 'in_progress') \
         AND id IN ( \
           SELECT MIN(w2.id) FROM waves w2 \
           WHERE w2.status = 'pending' GROUP BY w2.plan_id \
         ) \
         AND NOT EXISTS ( \
           SELECT 1 FROM waves w3 \
           WHERE w3.plan_id = waves.plan_id AND w3.status = 'in_progress' \
         )",
        [],
    );
    // Mark plans as done when ALL waves are done (#875)
    let plans_done = conn
        .execute(
            "UPDATE plans SET status = 'done', updated_at = datetime('now') \
             WHERE status = 'in_progress' \
             AND NOT EXISTS ( \
               SELECT 1 FROM waves WHERE plan_id = plans.id \
               AND status NOT IN ('done', 'cancelled') \
             ) \
             AND EXISTS ( \
               SELECT 1 FROM waves WHERE plan_id = plans.id AND status = 'done' \
             )",
            [],
        )
        .unwrap_or(0);
    if plans_done > 0 {
        tracing::info!("plan_executor: {plans_done} plan(s) completed (all waves done)");
    }
}
