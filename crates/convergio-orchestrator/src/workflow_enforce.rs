//! Workflow enforcement handlers: plan creation and execution guards.
//!
//! create_plan requires a valid solve_session_id (from cvg_solve).
//! execute_plan requires a plan in "approved" status.
//! howto queries the knowledge base.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::workflow_routes::WorkflowState;

// ── Create Plan (solve_session_id optional — falls back to manual) ───────────

#[derive(Debug, Deserialize)]
pub struct CreatePlanRequest {
    #[serde(default)]
    solve_session_id: Option<String>,
    plan_name: String,
    #[serde(default)]
    project_id: Option<String>,
}

pub async fn handle_create_plan(
    State(state): State<Arc<WorkflowState>>,
    Json(body): Json<CreatePlanRequest>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    let (project_id, source) = if let Some(ref ssid) = body.solve_session_id {
        // Solve-driven path: validate session
        let session = conn.query_row(
            "SELECT project_id, scale, status FROM solve_sessions WHERE id = ?1",
            params![ssid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        );

        let (proj, scale, status) = match session {
            Ok(s) => s,
            Err(_) => {
                return Json(json!({
                    "error": "invalid solve_session_id",
                    "hint": "You must call cvg_solve first to get a solve_session_id",
                    "required_step": "cvg_solve",
                }));
            }
        };

        if status != "active" {
            return Json(json!({
                "error": format!("solve session status is '{status}', expected 'active'"),
                "hint": "This solve session has already been used or expired",
            }));
        }

        if scale == "light" {
            return Json(json!({
                "error": "solve session scale is 'light' — no plan needed",
                "hint": "For light-scale problems, implement directly without a plan",
            }));
        }

        let pid = body.project_id.as_deref().unwrap_or(&proj).to_string();
        (pid, "solve")
    } else {
        // Manual path: project_id required
        let pid = match body.project_id {
            Some(ref p) if !p.is_empty() => p.clone(),
            _ => {
                return Json(json!({
                    "error": "project_id is required when solve_session_id is not provided",
                    "hint": "Either call cvg_solve first, or provide project_id for manual plan creation",
                }));
            }
        };
        tracing::warn!(plan_name = %body.plan_name, "creating plan without solve session (manual)");
        (pid, "manual")
    };

    let plan_result = conn.execute(
        "INSERT INTO plans (project_id, name, status, source) VALUES (?1, ?2, 'todo', ?3)",
        params![project_id, body.plan_name, source],
    );

    match plan_result {
        Ok(_) => {
            let plan_id = conn.last_insert_rowid();
            if let Some(ref ssid) = body.solve_session_id {
                let _ = conn.execute(
                    "UPDATE solve_sessions SET plan_id = ?1, status = 'consumed' \
                     WHERE id = ?2",
                    params![plan_id, ssid],
                );
            }
            let _ = conn.execute(
                "INSERT OR IGNORE INTO plan_metadata (plan_id, objective) \
                 VALUES (?1, ?2)",
                params![plan_id, body.plan_name],
            );

            Json(json!({
                "plan_id": plan_id,
                "project_id": project_id,
                "status": "todo",
                "source": source,
                "solve_session_id": body.solve_session_id,
                "next_step": "Import spec, add waves/tasks, then approve before cvg_execute_plan",
            }))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

// ── Execute Plan (requires approved status) ──────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ExecutePlanRequest {
    plan_id: i64,
}

pub async fn handle_execute_plan(
    State(state): State<Arc<WorkflowState>>,
    Json(body): Json<ExecutePlanRequest>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // ENFORCEMENT: validate plan exists and is approved
    let plan_status = conn.query_row(
        "SELECT status, tasks_total FROM plans WHERE id = ?1",
        params![body.plan_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    );

    let (status, tasks_total) = match plan_status {
        Ok(s) => s,
        Err(_) => {
            return Json(json!({
                "error": "plan not found",
                "hint": "Call cvg_create_plan first to create a plan",
                "required_step": "cvg_create_plan",
            }));
        }
    };

    if status != "approved" && status != "in_progress" {
        return Json(json!({
            "error": format!("plan status is '{status}', must be 'approved' or 'in_progress'"),
            "hint": "Approve via POST /api/plan-db/approve/:plan_id after review, or use force-resume to set it to in_progress.",
            "current_status": status,
            "required_status": "approved or in_progress",
        }));
    }

    if tasks_total == 0 {
        return Json(json!({
            "error": "plan has no tasks — import a spec first",
            "hint": "Use POST /api/plan-db/import to add waves and tasks",
        }));
    }

    let _ = conn.execute(
        "UPDATE plans SET status = 'in_progress', \
         started_at = datetime('now'), updated_at = datetime('now') \
         WHERE id = ?1",
        params![body.plan_id],
    );

    // UPDATE ... ORDER BY ... LIMIT requires SQLITE_ENABLE_UPDATE_DELETE_LIMIT
    // which rusqlite does not enable by default. Use subquery instead (#729).
    let _ = conn.execute(
        "UPDATE waves SET status = 'in_progress', started_at = datetime('now') \
         WHERE id = (SELECT id FROM waves WHERE plan_id = ?1 AND status = 'pending' \
         ORDER BY id ASC LIMIT 1)",
        params![body.plan_id],
    );

    Json(json!({
        "plan_id": body.plan_id,
        "status": "in_progress",
        "tasks_total": tasks_total,
        "next_step": "Plan execution started. Monitor via cvg_get_plan.",
    }))
}

// ── How-To (KB search) ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct HowToQuery {
    question: String,
    #[serde(default = "default_limit")]
    limit: Option<i64>,
}

fn default_limit() -> Option<i64> {
    Some(5)
}

pub async fn handle_howto(
    State(state): State<Arc<WorkflowState>>,
    Query(q): Query<HowToQuery>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    let limit = q.limit.unwrap_or(5);
    let pattern = format!("%{}%", q.question);

    let mut stmt = match conn.prepare(
        "SELECT id, domain, title, content FROM knowledge_base \
         WHERE title LIKE ?1 OR content LIKE ?1 OR domain LIKE ?1 \
         ORDER BY hit_count DESC LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(_) => {
            return Json(json!({
                "results": [],
                "count": 0,
                "hint": "Knowledge base is empty. Onboard a project first.",
            }));
        }
    };

    let rows: Vec<serde_json::Value> = stmt
        .query_map(params![pattern, limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "domain": r.get::<_, Option<String>>(1)?,
                "title": r.get::<_, Option<String>>(2)?,
                "content": r.get::<_, Option<String>>(3)?,
            }))
        })
        .map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();

    let count = rows.len();
    Json(json!({"results": rows, "count": count, "question": q.question}))
}
