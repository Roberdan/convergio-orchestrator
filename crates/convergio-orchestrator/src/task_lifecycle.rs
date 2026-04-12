//! Task lifecycle event emission — IPC + domain events on status change.

use serde_json::json;

use crate::plan_routes::PlanState;

/// Emit IPC task_done (for reactor) and DomainEvent TaskCompleted (for SSE/UI).
/// Handles its own DB connection to avoid holding the pool during IPC emit.
pub fn emit_task_lifecycle(state: &PlanState, task_id: i64) {
    let plan_id = {
        let conn = match state.pool.get() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(task_id, "pool error in emit_task_lifecycle: {e}");
                return;
            }
        };
        let plan_id: Option<i64> = conn
            .query_row(
                "SELECT plan_id FROM tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .ok();
        let Some(plan_id) = plan_id else {
            tracing::warn!(task_id, "task has no plan_id — skipping lifecycle emit");
            return;
        };
        let _ = conn.execute(
            "UPDATE plans SET tasks_done = \
             (SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 AND status IN ('done','submitted')), \
             updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![plan_id],
        );
        plan_id
    };

    let content = json!({
        "type": "task_done",
        "task_id": task_id.to_string(),
        "plan_id": plan_id
    });
    if let Err(e) = convergio_ipc::messaging::broadcast(
        &state.pool,
        &state.notify,
        "task-updater",
        &content.to_string(),
        "event",
        Some("#orchestration"),
        100,
    ) {
        tracing::warn!(task_id, "failed to emit task_done IPC: {e}");
    }

    if let Some(ref sink) = state.event_sink {
        sink.emit(convergio_types::events::make_event(
            "orchestrator",
            convergio_types::events::EventKind::TaskCompleted { task_id },
            convergio_types::events::EventContext {
                plan_id: Some(plan_id),
                task_id: Some(task_id),
                ..Default::default()
            },
        ));
    }

    tracing::info!(task_id, plan_id, "task lifecycle events emitted");
}
