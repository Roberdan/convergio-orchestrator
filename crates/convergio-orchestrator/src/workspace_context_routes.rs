//! Routes for workspace context: who is working on what files.
//! Used by agent spawn enrichment to prevent file conflicts.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use serde_json::json;

use crate::plan_routes::PlanState;

pub fn workspace_context_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route(
            "/api/plan-db/tasks/in-progress",
            get(handle_tasks_in_progress),
        )
        .with_state(state)
}

/// GET /api/plan-db/tasks/in-progress — active tasks with claimed_files.
async fn handle_tasks_in_progress(State(state): State<Arc<PlanState>>) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, title, executor_agent, claimed_files \
         FROM tasks WHERE status = 'in_progress'",
    ) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let tasks: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_,i64>(0)?,
                "title": r.get::<_,String>(1)?,
                "executor_agent": r.get::<_,Option<String>>(2)?,
                "claimed_files": r.get::<_,Option<String>>(3)?
            }))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    Json(json!({"tasks": tasks}))
}
