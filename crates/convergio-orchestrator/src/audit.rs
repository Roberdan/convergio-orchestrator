//! Task status audit trail — logs every status transition.
//!
//! - `log_status_change()` — insert into task_status_log
//! - GET /api/plan-db/task/:id/history — view audit trail

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use rusqlite::{params, Connection};
use serde_json::json;

use crate::plan_routes::PlanState;

/// Record a task status transition in the audit log.
pub fn log_status_change(
    conn: &Connection,
    task_id: i64,
    old_status: &str,
    new_status: &str,
    agent: &str,
    notes: Option<&str>,
) {
    if let Err(e) = conn.execute(
        "INSERT INTO task_status_log (task_id, old_status, new_status, agent, notes) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, old_status, new_status, agent, notes],
    ) {
        tracing::warn!(task_id, "audit log insert failed: {e}");
    }
}

pub fn audit_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route(
            "/api/plan-db/task/:task_id/history",
            get(handle_task_history),
        )
        .with_state(state)
}

async fn handle_task_history(
    State(state): State<Arc<PlanState>>,
    Path(task_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, old_status, new_status, agent, notes, created_at \
         FROM task_status_log WHERE task_id = ?1 ORDER BY id",
    ) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let rows: Vec<serde_json::Value> = stmt
        .query_map(params![task_id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "old_status": r.get::<_, String>(1)?,
                "new_status": r.get::<_, String>(2)?,
                "agent": r.get::<_, String>(3)?,
                "notes": r.get::<_, Option<String>>(4)?,
                "created_at": r.get::<_, String>(5)?
            }))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    Json(json!({"task_id": task_id, "history": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE task_status_log (\
                 id INTEGER PRIMARY KEY AUTOINCREMENT,\
                 task_id INTEGER NOT NULL,\
                 old_status TEXT NOT NULL,\
                 new_status TEXT NOT NULL,\
                 agent TEXT NOT NULL DEFAULT '',\
                 notes TEXT,\
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))\
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn log_inserts_record() {
        let conn = setup_db();
        log_status_change(&conn, 1, "pending", "in_progress", "agent-a", None);
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM task_status_log WHERE task_id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn log_preserves_fields() {
        let conn = setup_db();
        log_status_change(
            &conn,
            42,
            "in_progress",
            "submitted",
            "bot-x",
            Some("PR #99"),
        );
        let (old, new, agent, notes): (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT old_status, new_status, agent, notes \
                 FROM task_status_log WHERE task_id = 42",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(old, "in_progress");
        assert_eq!(new, "submitted");
        assert_eq!(agent, "bot-x");
        assert_eq!(notes.as_deref(), Some("PR #99"));
    }
}
