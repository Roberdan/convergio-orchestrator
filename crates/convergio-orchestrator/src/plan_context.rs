//! Plan context, execution-context, drift-check, validate-task routes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use rusqlite::params;
use serde_json::json;

use convergio_db::pool::ConnPool;

pub fn context_routes(pool: ConnPool) -> Router {
    let state = Arc::new(CtxState { pool });
    Router::new()
        .route("/api/plan-db/context/:plan_id", get(handle_context))
        .route(
            "/api/plan-db/execution-context/:plan_id",
            get(handle_exec_context),
        )
        .route("/api/plan-db/drift-check/:plan_id", get(handle_drift_check))
        .route(
            "/api/plan-db/validate-task/:task_id/:plan_id",
            get(handle_validate_task),
        )
        .with_state(state)
}

struct CtxState {
    pool: ConnPool,
}

/// Full plan context for execution agents.
async fn handle_context(
    State(state): State<Arc<CtxState>>,
    Path(plan_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let plan = conn
        .query_row(
            "SELECT id, name, status, project_id FROM plans WHERE id = ?1",
            params![plan_id],
            |r| {
                Ok(json!({
                    "id": r.get::<_,i64>(0)?,
                    "name": r.get::<_,String>(1)?,
                    "status": r.get::<_,String>(2)?,
                    "project_id": r.get::<_,String>(3)?,
                }))
            },
        )
        .unwrap_or(json!(null));
    let meta = conn
        .query_row(
            "SELECT objective, motivation, worktree_path \
             FROM plan_metadata WHERE plan_id = ?1",
            params![plan_id],
            |r| {
                Ok(json!({
                    "objective": r.get::<_,Option<String>>(0)?,
                    "motivation": r.get::<_,Option<String>>(1)?,
                    "worktree_path": r.get::<_,Option<String>>(2)?,
                }))
            },
        )
        .unwrap_or(json!(null));
    let task_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let done_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id=?1 AND status='done'",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Json(json!({
        "plan": plan, "metadata": meta,
        "tasks_total": task_count, "tasks_done": done_count,
    }))
}

/// Next pending task + prompt for executor agents.
async fn handle_exec_context(
    State(state): State<Arc<CtxState>>,
    Path(plan_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let task = conn.query_row(
        "SELECT t.id, t.task_id, t.title, t.executor_agent, t.metadata, \
         w.wave_id FROM tasks t JOIN waves w ON w.id = t.wave_id \
         WHERE t.plan_id = ?1 AND t.status = 'pending' \
         ORDER BY w.id, t.id LIMIT 1",
        params![plan_id],
        |r| {
            Ok(json!({
                "id": r.get::<_,i64>(0)?,
                "task_id": r.get::<_,Option<String>>(1)?,
                "title": r.get::<_,String>(2)?,
                "executor_agent": r.get::<_,Option<String>>(3)?,
                "metadata": r.get::<_,Option<String>>(4)?,
                "wave_id": r.get::<_,String>(5)?,
            }))
        },
    );
    match task {
        Ok(t) => Json(json!({"plan_id": plan_id, "next_task": t})),
        Err(_) => Json(json!({"plan_id": plan_id, "next_task": null})),
    }
}

/// Drift check — are tasks stale vs latest code?
async fn handle_drift_check(
    State(state): State<Arc<CtxState>>,
    Path(plan_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let created: Option<String> = conn
        .query_row(
            "SELECT created_at FROM plans WHERE id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .ok();
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id=?1 AND status='pending'",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Json(json!({
        "plan_id": plan_id,
        "created_at": created,
        "pending_tasks": pending,
        "total_tasks": total,
        "completion_pct": if total > 0 { ((total - pending) * 100) / total } else { 0 },
    }))
}

/// Validate a single task (Thor gate check).
async fn handle_validate_task(
    State(state): State<Arc<CtxState>>,
    Path((task_id, plan_id)): Path<(i64, i64)>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let task = conn.query_row(
        "SELECT id, status, metadata FROM tasks \
         WHERE id = ?1 AND plan_id = ?2",
        params![task_id, plan_id],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        },
    );
    match task {
        Ok((id, status, meta)) => {
            let has_evidence: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM task_evidence \
                     WHERE task_id = ?1)",
                    params![id],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            Json(json!({
                "task_id": id, "plan_id": plan_id,
                "status": status, "has_evidence": has_evidence,
                "metadata": meta,
                "valid": status == "submitted" && has_evidence,
            }))
        }
        Err(_) => Json(json!({"error": "task not found"})),
    }
}
