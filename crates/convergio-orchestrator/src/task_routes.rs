//! Task create/update/delete routes.
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{patch, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::plan_routes::PlanState;

pub fn task_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/task/create", post(handle_task_create))
        .route("/api/plan-db/task/update", post(handle_task_update))
        .route(
            "/api/plan-db/task/complete-flow",
            post(handle_complete_flow),
        )
        .route("/api/plan-db/task/delete/:id", post(handle_task_delete))
        .route(
            "/api/plan-db/task/:id",
            patch(crate::task_patch::handle_patch_task_content),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct TaskUpdate {
    pub task_id: i64,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub output_data: Option<String>,
    #[serde(default)]
    pub executor_agent: Option<String>,
    #[serde(default)]
    pub tokens: Option<i64>,
    /// Required for status changes — identifies who is making the update.
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TaskCreate {
    pub plan_id: i64,
    pub wave_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub executor_agent: Option<String>,
    /// JSON array of required capabilities for skill-based dispatch.
    #[serde(default)]
    pub required_capabilities: Option<String>,
    /// JSON array of file paths this task will modify (for conflict detection).
    #[serde(default)]
    pub claimed_files: Option<String>,
}

#[tracing::instrument(skip_all, fields(plan_id = %body.plan_id))]
async fn handle_task_create(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<TaskCreate>,
) -> Response {
    // Input validation: bound string lengths
    if body.title.len() > 500
        || body.description.as_deref().unwrap_or("").len() > 10000
        || body.task_id.as_deref().unwrap_or("").len() > 200
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "field exceeds maximum length"})),
        )
            .into_response();
    }
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})).into_response(),
    };
    if let Err(e) = crate::schema::ensure_required_capabilities_column(&conn) {
        return Json(json!({"error": format!("schema drift: {e}")})).into_response();
    }
    // ImportGate: only add tasks to plans in draft/todo/approved
    if let Err(gate_err) = crate::gates::import_gate(&conn, body.plan_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("gate blocked: {gate_err}"), "gate": gate_err.gate, "expected": gate_err.expected})),
        )
            .into_response();
    }
    let task_id_str = body.task_id.unwrap_or_default();
    match conn.execute(
        "INSERT INTO tasks (task_id, plan_id, wave_id, title, description, executor_agent, required_capabilities, claimed_files) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            task_id_str,
            body.plan_id,
            body.wave_id,
            body.title,
            body.description.as_deref().unwrap_or(""),
            body.executor_agent.as_deref().unwrap_or(""),
            body.required_capabilities,
            body.claimed_files.as_deref().unwrap_or("[]"),
        ],
    ) {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            let _ = conn.execute(
                "UPDATE plans SET tasks_total = tasks_total + 1 WHERE id = ?1",
                rusqlite::params![body.plan_id],
            );
            Json(json!({"id": id, "task_id": task_id_str})).into_response()
        }
        Err(e) => Json(json!({"error": e.to_string()})).into_response(),
    }
}

#[tracing::instrument(skip_all, fields(task_id = %body.task_id))]
async fn handle_task_update(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<TaskUpdate>,
) -> Response {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})).into_response(),
    };
    // Agent identity enforcement: status changes require agent_id
    if body.status.is_some() && body.agent_id.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "agent_id required for status changes"})),
        )
            .into_response();
    }
    // TaskLockGate: prevent two agents from claiming the same task
    if body.status.as_deref() == Some("in_progress") {
        if let Err(gate_err) =
            crate::concurrency_gates::task_lock_gate(&conn, body.task_id, body.agent_id.as_deref())
        {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": format!("gate blocked: {gate_err}"), "gate": gate_err.gate, "expected": gate_err.expected})),
            )
                .into_response();
        }
    }
    // WavePrDedupGate: ensure one PR per wave
    if body.status.as_deref() == Some("submitted") {
        if let Err(gate_err) = crate::concurrency_gates::wave_pr_dedup_gate(&conn, body.task_id) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": format!("gate blocked: {gate_err}"), "gate": gate_err.gate, "expected": gate_err.expected})),
            )
                .into_response();
        }
    }
    // Lifecycle gate check: validate transition before applying
    if let Some(ref new_status) = body.status {
        if let Err(gate_err) =
            crate::gates::check_task_transition(&state.pool, body.task_id, new_status)
        {
            tracing::warn!(
                task_id = body.task_id,
                gate = gate_err.gate,
                "gate blocked transition"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("gate blocked: {gate_err}"), "gate": gate_err.gate, "expected": gate_err.expected})),
            )
                .into_response();
        }
    }
    // Fetch old status for audit trail
    let old_status: Option<String> = conn
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            rusqlite::params![body.task_id],
            |r| r.get(0),
        )
        .ok();

    let mut sets: Vec<String> = Vec::new();
    let mut p: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(ref s) = body.status {
        p.push(Box::new(s.clone()));
        sets.push(format!("status = ?{}", p.len()));
        if s == "in_progress" {
            sets.push("started_at = datetime('now')".into());
        } else if s == "done" || s == "submitted" {
            sets.push("completed_at = datetime('now')".into());
        }
    }
    if let Some(ref n) = body.notes {
        p.push(Box::new(n.clone()));
        sets.push(format!("notes = ?{}", p.len()));
    }
    if let Some(ref o) = body.output_data {
        p.push(Box::new(o.clone()));
        sets.push(format!("output_data = ?{}", p.len()));
    }
    if let Some(ref a) = body.executor_agent {
        p.push(Box::new(a.clone()));
        sets.push(format!("executor_agent = ?{}", p.len()));
    }
    if let Some(t) = body.tokens {
        p.push(Box::new(t));
        sets.push(format!("tokens = ?{}", p.len()));
    }
    if sets.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "no fields to update"})),
        )
            .into_response();
    }
    p.push(Box::new(body.task_id));
    let sql = format!(
        "UPDATE tasks SET {} WHERE id = ?{}",
        sets.join(", "),
        p.len()
    );
    let refs: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|v| v.as_ref()).collect();
    match conn.execute(&sql, refs.as_slice()) {
        Ok(0) => Json(json!({"error": "task not found"})).into_response(),
        Ok(_) => {
            // Update task lock: set locked_by on in_progress, clear on terminal
            if let Some(ref new_status) = body.status {
                let agent = body.agent_id.as_deref().unwrap_or("");
                crate::concurrency_gates::update_task_lock(&conn, body.task_id, new_status, agent);
            }
            if let (Some(ref new_status), Some(ref old)) = (&body.status, &old_status) {
                let agent = body.agent_id.as_deref().unwrap_or("");
                crate::audit::log_status_change(
                    &conn,
                    body.task_id,
                    old,
                    new_status,
                    agent,
                    body.notes.as_deref(),
                );
            }
            if let Some(ref new_status) = body.status {
                if new_status == "done" || new_status == "submitted" {
                    let trigger_auto_thor = new_status == "submitted";
                    let task_id = body.task_id;
                    let state_clone = state.clone();
                    drop(conn);
                    emit_task_lifecycle(&state, body.task_id);
                    // Auto-Thor: if all wave tasks are terminal, auto-validate
                    if trigger_auto_thor {
                        tokio::spawn(async move {
                            crate::auto_thor::try_auto_thor(&state_clone, task_id).await;
                        });
                    }
                }
            }
            Json(json!({"task_id": body.task_id, "updated": true})).into_response()
        }
        Err(e) => Json(json!({"error": e.to_string()})).into_response(),
    }
}

use crate::task_lifecycle::emit_task_lifecycle;

// Complete-flow handler lives in sibling module
pub use crate::task_complete_flow::handle_complete_flow;

/// DELETE a task from a plan (only if plan is in draft/todo state).
#[tracing::instrument(skip_all, fields(task_id = %id))]
async fn handle_task_delete(
    State(state): State<Arc<PlanState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    // Check plan status — only allow delete on non-started plans
    let plan_id: Result<i64, _> = conn.query_row(
        "SELECT plan_id FROM tasks WHERE id = ?1",
        rusqlite::params![id],
        |r| r.get(0),
    );
    let plan_id = match plan_id {
        Ok(p) => p,
        Err(_) => return Json(json!({"error": "task not found"})),
    };
    if let Err(e) = crate::gates::import_gate(&conn, plan_id) {
        return Json(json!({"error": format!("cannot delete: {e}")}));
    }
    match conn.execute("DELETE FROM tasks WHERE id = ?1", rusqlite::params![id]) {
        Ok(1) => {
            let _ = conn.execute(
                "UPDATE plans SET tasks_total = tasks_total - 1 WHERE id = ?1",
                rusqlite::params![plan_id],
            );
            Json(json!({"deleted": true, "task_id": id}))
        }
        _ => Json(json!({"error": "task not found or already deleted"})),
    }
}
#[cfg(test)]
#[path = "task_routes_tests.rs"]
mod tests;
