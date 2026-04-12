//! Heartbeat route for task liveness.
//!
//! POST /api/plan-db/task/heartbeat — update last_heartbeat for in_progress task.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

pub fn heartbeat_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/task/heartbeat", post(handle_heartbeat))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct HeartbeatRequest {
    task_id: i64,
}

async fn handle_heartbeat(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<HeartbeatRequest>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match conn.execute(
        "UPDATE tasks SET last_heartbeat = datetime('now') \
         WHERE id = ?1 AND status = 'in_progress'",
        rusqlite::params![body.task_id],
    ) {
        Ok(0) => Json(json!({
            "error": "task not found or not in_progress",
            "task_id": body.task_id
        })),
        Ok(_) => Json(json!({"task_id": body.task_id, "heartbeat": "ok"})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn heartbeat_updates_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (\
                 id INTEGER PRIMARY KEY, status TEXT, last_heartbeat TEXT\
             );\
             INSERT INTO tasks (id, status) VALUES (1, 'in_progress');",
        )
        .unwrap();
        let updated = conn
            .execute(
                "UPDATE tasks SET last_heartbeat = datetime('now') \
                 WHERE id = 1 AND status = 'in_progress'",
                [],
            )
            .unwrap();
        assert_eq!(updated, 1);
        let hb: Option<String> = conn
            .query_row("SELECT last_heartbeat FROM tasks WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(hb.is_some());
    }

    #[test]
    fn heartbeat_rejects_non_in_progress() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (\
                 id INTEGER PRIMARY KEY, status TEXT, last_heartbeat TEXT\
             );\
             INSERT INTO tasks (id, status) VALUES (1, 'pending');",
        )
        .unwrap();
        let updated = conn
            .execute(
                "UPDATE tasks SET last_heartbeat = datetime('now') \
                 WHERE id = 1 AND status = 'in_progress'",
                [],
            )
            .unwrap();
        assert_eq!(updated, 0);
    }
}
