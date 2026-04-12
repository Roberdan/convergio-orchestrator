//! HTTP routes for wave branch management.
//!
//! GET  /api/plan-db/wave/:wave_id/branch  — get/assign branch for wave
//! POST /api/plan-db/wave/:wave_id/strategy — assign commit strategy

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;

use crate::plan_routes::PlanState;
use crate::wave_branch;

pub fn wave_branch_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/wave/:wave_id/branch", get(handle_get_branch))
        .route(
            "/api/plan-db/wave/:wave_id/strategy",
            post(handle_assign_strategy),
        )
        .with_state(state)
}

/// GET /api/plan-db/wave/:wave_id/branch
/// Returns the wave's branch name (assigns one if not yet set).
async fn handle_get_branch(
    State(state): State<Arc<PlanState>>,
    Path(wave_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match wave_branch::assign_wave_branch(&conn, wave_id) {
        Ok(branch) => {
            let strategy = wave_branch::get_commit_strategy(&conn, wave_id)
                .unwrap_or(wave_branch::CommitStrategy::ViaPr);
            Json(json!({
                "wave_id": wave_id,
                "branch": branch,
                "commit_strategy": strategy.as_str(),
            }))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

/// POST /api/plan-db/wave/:wave_id/strategy
/// Determines and stores the commit strategy based on wave tasks.
async fn handle_assign_strategy(
    State(state): State<Arc<PlanState>>,
    Path(wave_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match wave_branch::assign_commit_strategy(&conn, wave_id) {
        Ok(strategy) => Json(json!({
            "wave_id": wave_id,
            "commit_strategy": strategy.as_str(),
        })),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}
