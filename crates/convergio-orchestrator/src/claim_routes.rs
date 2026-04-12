//! Runtime file claim API — agents can claim files discovered during execution.
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::post;
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::plan_routes::PlanState;

pub fn claim_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/task/:id/claim-file", post(handle_claim_file))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ClaimRequest {
    file_path: String,
}

/// POST /api/plan-db/task/:id/claim-file — claim an additional file at runtime.
async fn handle_claim_file(
    State(state): State<Arc<PlanState>>,
    Path(task_id): Path<i64>,
    Json(req): Json<ClaimRequest>,
) -> Json<Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let current: String = conn
        .query_row(
            "SELECT claimed_files FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "[]".into());
    let mut files: Vec<String> = serde_json::from_str(&current).unwrap_or_default();
    if files.contains(&req.file_path) {
        return Json(json!({"ok": true, "claimed_files": files, "added": false}));
    }
    files.push(req.file_path);
    let updated = serde_json::to_string(&files).unwrap_or_else(|_| "[]".into());
    match conn.execute(
        "UPDATE tasks SET claimed_files = ?1 WHERE id = ?2",
        params![updated, task_id],
    ) {
        Ok(0) => Json(json!({"error": "task not found"})),
        Ok(_) => {
            tracing::info!(task_id, claimed_files = %updated, "runtime file claim");
            // Emit FilesClaimed event if sink available
            if let Some(ref sink) = state.event_sink {
                use convergio_types::events::{make_event, EventContext, EventKind};
                sink.emit(make_event(
                    "claim-api",
                    EventKind::FilesClaimed {
                        task_id,
                        agent: String::new(),
                        file_paths: files.clone(),
                    },
                    EventContext {
                        org_id: None,
                        plan_id: None,
                        task_id: Some(task_id),
                    },
                ));
            }
            Json(json!({"ok": true, "claimed_files": files, "added": true}))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_routes_builds() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let state = Arc::new(PlanState {
            pool,
            event_sink: None,
            notify: Arc::new(tokio::sync::Notify::new()),
        });
        let _router = claim_routes(state);
    }
}
