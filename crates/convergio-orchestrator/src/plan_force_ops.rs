//! Force operations for stuck plans — admin escape hatches.
//!
//! POST /api/plan-db/force-resume/:plan_id — resume + unstick paused tasks
//! POST /api/plan-db/force-complete/:plan_id — mark all non-terminal tasks done

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::post;
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

#[derive(Debug, Deserialize, Default)]
struct ForceResumeBody {
    target_status: Option<String>,
}

pub fn force_ops_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route(
            "/api/plan-db/force-resume/:plan_id",
            post(handle_force_resume),
        )
        .route(
            "/api/plan-db/force-complete/:plan_id",
            post(handle_force_complete),
        )
        .with_state(state)
}

/// Force-resume: set plan to a target status (default: in_progress) regardless of current status,
/// clear all task locks, reset stuck in_progress tasks back to pending.
///
/// Accepts an optional JSON body: `{ "target_status": "approved" | "in_progress" | "todo" }`
/// Defaults to "in_progress" if not provided.
async fn handle_force_resume(
    State(s): State<Arc<PlanState>>,
    Path(id): Path<i64>,
    body: Option<Json<ForceResumeBody>>,
) -> Json<serde_json::Value> {
    let Ok(conn) = s.pool.get() else {
        return Json(json!({"error": "db pool"}));
    };
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM plans WHERE id = ?1)",
            params![id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !exists {
        return Json(json!({"error": format!("plan {id} not found")}));
    }

    // Determine target status — only accept valid non-terminal plan states
    let raw_target = body
        .and_then(|b| b.0.target_status)
        .unwrap_or_else(|| "in_progress".to_string());
    let target_status = match raw_target.as_str() {
        "in_progress" | "approved" | "todo" => raw_target.as_str().to_string(),
        other => {
            return Json(json!({
                "error": format!("invalid target_status '{other}'"),
                "hint": "Allowed values: 'in_progress', 'approved', 'todo'",
            }));
        }
    };

    // Force plan to target status
    let _ = conn.execute(
        "UPDATE plans SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
        params![id, target_status],
    );

    // Clear all task locks for this plan
    let locks_cleared = conn
        .execute(
            "UPDATE tasks SET locked_by = '' WHERE plan_id = ?1 AND locked_by != ''",
            params![id],
        )
        .unwrap_or(0);

    // Reset stuck in_progress tasks (no heartbeat) back to pending
    let tasks_reset = conn
        .execute(
            "UPDATE tasks SET status = 'pending', locked_by = '' \
             WHERE plan_id = ?1 AND status = 'in_progress' \
               AND (last_heartbeat IS NULL \
                    OR last_heartbeat < datetime('now', '-10 minutes'))",
            params![id],
        )
        .unwrap_or(0);

    // Resume pending waves
    let _ = conn.execute(
        "UPDATE waves SET status = 'in_progress' WHERE plan_id = ?1 AND status = 'pending' \
         AND id = (SELECT MIN(id) FROM waves WHERE plan_id = ?1 AND status = 'pending')",
        params![id],
    );

    Json(json!({
        "plan_id": id,
        "status": target_status,
        "locks_cleared": locks_cleared,
        "tasks_reset_to_pending": tasks_reset,
        "action": "force_resume"
    }))
}

/// Force-complete: mark ALL non-terminal tasks as done, complete the plan.
/// Use when code is merged on main but daemon DB is stuck.
async fn handle_force_complete(
    State(s): State<Arc<PlanState>>,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    let Ok(conn) = s.pool.get() else {
        return Json(json!({"error": "db pool"}));
    };
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM plans WHERE id = ?1)",
            params![id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !exists {
        return Json(json!({"error": format!("plan {id} not found")}));
    }

    // Mark all non-terminal tasks as done
    let tasks_completed = conn
        .execute(
            "UPDATE tasks SET status = 'done', completed_at = datetime('now'), \
             locked_by = '' \
             WHERE plan_id = ?1 AND status NOT IN ('done', 'cancelled', 'skipped')",
            params![id],
        )
        .unwrap_or(0);

    // Mark all waves as done
    let waves_completed = conn
        .execute(
            "UPDATE waves SET status = 'done', completed_at = datetime('now') \
             WHERE plan_id = ?1 AND status != 'done'",
            params![id],
        )
        .unwrap_or(0);

    // Mark plan as done
    let _ = conn.execute(
        "UPDATE plans SET status = 'done', completed_at = datetime('now'), \
         updated_at = datetime('now') WHERE id = ?1",
        params![id],
    );

    Json(json!({
        "plan_id": id,
        "status": "done",
        "tasks_completed": tasks_completed,
        "waves_completed": waves_completed,
        "action": "force_complete"
    }))
}
