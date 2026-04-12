//! Plan readiness, approval, worktree, and review reset routes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

pub fn readiness_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/readiness/:plan_id", get(handle_readiness))
        .route("/api/plan-db/approve/:plan_id", post(handle_approve))
        .route(
            "/api/plan-db/set-worktree/:plan_id",
            post(handle_set_worktree),
        )
        .route("/api/plan-db/review/reset", post(handle_review_reset))
        .with_state(state)
}

async fn handle_readiness(
    State(state): State<Arc<PlanState>>,
    Path(plan_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let mut checks: Vec<serde_json::Value> = Vec::new();
    let mut errors = 0u32;

    // 1. Plan exists
    let plan_status: Option<String> = conn
        .query_row(
            "SELECT status FROM plans WHERE id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .ok();
    let exists = plan_status.is_some();
    if !exists {
        return Json(json!({"error": "plan not found"}));
    }
    checks.push(json!({"check": "plan_exists", "pass": true}));

    // 2. Has waves
    let wave_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM waves WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let has_waves = wave_count > 0;
    if !has_waves {
        errors += 1;
    }
    checks.push(json!({
        "check": "has_waves", "pass": has_waves, "count": wave_count
    }));

    // 3. Has tasks
    let task_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let has_tasks = task_count > 0;
    if !has_tasks {
        errors += 1;
    }
    checks.push(json!({
        "check": "has_tasks", "pass": has_tasks, "count": task_count
    }));

    // 4. Has metadata
    let has_meta: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM plan_metadata WHERE plan_id = ?1)",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_meta {
        errors += 1;
    }
    checks.push(json!({"check": "has_metadata", "pass": has_meta}));

    // 5. Status is importable
    let status = plan_status.unwrap_or_default();
    let valid = matches!(status.as_str(), "todo" | "draft" | "approved");
    if !valid {
        errors += 1;
    }
    checks.push(json!({
        "check": "status_valid", "pass": valid, "status": status
    }));

    Json(json!({
        "plan_id": plan_id,
        "ready": errors == 0,
        "errors": errors,
        "checks": checks,
    }))
}

async fn handle_approve(
    State(state): State<Arc<PlanState>>,
    Path(plan_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match conn.execute(
        "UPDATE plans SET status = 'approved' WHERE id = ?1 \
         AND status IN ('todo', 'draft')",
        params![plan_id],
    ) {
        Ok(n) if n > 0 => Json(json!({"plan_id": plan_id, "status": "approved"})),
        Ok(_) => Json(json!({"error": "plan not found or not in draft/todo"})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[derive(Debug, Deserialize)]
struct WorktreeBody {
    worktree_path: String,
}

async fn handle_set_worktree(
    State(state): State<Arc<PlanState>>,
    Path(plan_id): Path<i64>,
    Json(body): Json<WorktreeBody>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match conn.execute(
        "UPDATE plan_metadata SET worktree_path = ?1 WHERE plan_id = ?2",
        params![body.worktree_path, plan_id],
    ) {
        Ok(0) => {
            // No metadata row yet — insert
            let _ = conn.execute(
                "INSERT OR IGNORE INTO plan_metadata (plan_id, worktree_path) \
                 VALUES (?1, ?2)",
                params![plan_id, body.worktree_path],
            );
            Json(json!({"plan_id": plan_id, "worktree_path": body.worktree_path}))
        }
        Ok(_) => Json(json!({"plan_id": plan_id, "worktree_path": body.worktree_path})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[derive(Debug, Deserialize)]
struct ReviewResetBody {
    #[serde(default)]
    plan_id: Option<i64>,
}

async fn handle_review_reset(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<ReviewResetBody>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let deleted = if let Some(pid) = body.plan_id {
        conn.execute("DELETE FROM plan_reviews WHERE plan_id = ?1", params![pid])
            .unwrap_or(0)
    } else {
        conn.execute("DELETE FROM plan_reviews", []).unwrap_or(0)
    };
    Json(json!({"reset": true, "deleted": deleted}))
}
