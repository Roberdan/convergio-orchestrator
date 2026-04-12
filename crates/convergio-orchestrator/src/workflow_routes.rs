//! Workflow routes: solve → plan → execute with dependency enforcement.
//!
//! These routes enforce the solve→plan→execute ordering by requiring
//! data dependencies: create_plan needs a valid solve_session_id,
//! execute_plan needs a plan in "approved" status.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use convergio_db::pool::ConnPool;

use crate::workflow_enforce;

// ── State ────────────────────────────────────────────────────────────────────

pub struct WorkflowState {
    pub pool: ConnPool,
}

// ── Routes ───────────────────────────────────────────────────────────────────

pub fn workflow_routes(pool: ConnPool) -> Router {
    let state = Arc::new(WorkflowState { pool });
    Router::new()
        .route("/api/workflow/solve", post(handle_solve))
        .route("/api/workflow/solve/:session_id", get(handle_get_solve))
        .route(
            "/api/workflow/plan",
            post(workflow_enforce::handle_create_plan),
        )
        .route(
            "/api/workflow/execute",
            post(workflow_enforce::handle_execute_plan),
        )
        .route("/api/workflow/howto", get(workflow_enforce::handle_howto))
        .with_state(state)
}

// ── Solve ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SolveRequest {
    project_id: String,
    problem_description: String,
    #[serde(default)]
    input_documents: Vec<String>,
}

async fn handle_solve(
    State(state): State<Arc<WorkflowState>>,
    Json(body): Json<SolveRequest>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // Verify project exists
    let project_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM projects WHERE id = ?1",
            params![body.project_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !project_exists {
        return Json(
            json!({"error": "project not found", "hint": "register via POST /api/projects"}),
        );
    }

    // Perform scale triage based on problem description length
    let scale = triage_scale(&body.problem_description);

    let session_id = Uuid::new_v4().to_string();
    let docs_json = serde_json::to_string(&body.input_documents).unwrap_or_default();

    let result = conn.execute(
        "INSERT INTO solve_sessions \
         (id, project_id, problem_description, scale, requirements_json, status) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'active')",
        params![
            session_id,
            body.project_id,
            body.problem_description,
            scale,
            docs_json,
        ],
    );

    match result {
        Ok(_) => {
            let next = if scale == "light" {
                "Direct implementation (scale=light, no plan needed)"
            } else {
                "Call cvg_create_plan with this solve_session_id"
            };
            Json(json!({
                "solve_session_id": session_id,
                "project_id": body.project_id,
                "scale": scale,
                "status": "active",
                "next_step": next,
            }))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn handle_get_solve(
    State(state): State<Arc<WorkflowState>>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    match conn.query_row(
        "SELECT id, project_id, problem_description, scale, \
         requirements_json, plan_id, status, created_at \
         FROM solve_sessions WHERE id = ?1",
        params![session_id],
        |r| {
            Ok(json!({
                "solve_session_id": r.get::<_, String>(0)?,
                "project_id": r.get::<_, String>(1)?,
                "problem_description": r.get::<_, String>(2)?,
                "scale": r.get::<_, String>(3)?,
                "requirements_json": r.get::<_, Option<String>>(4)?,
                "plan_id": r.get::<_, Option<i64>>(5)?,
                "status": r.get::<_, String>(6)?,
                "created_at": r.get::<_, String>(7)?,
            }))
        },
    ) {
        Ok(session) => Json(session),
        Err(_) => Json(json!({"error": "solve session not found"})),
    }
}

// ── Scale triage ─────────────────────────────────────────────────────────────

fn triage_scale(description: &str) -> &'static str {
    let word_count = description.split_whitespace().count();
    if word_count < 30 {
        "light"
    } else if word_count < 150 {
        "standard"
    } else {
        "full"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_triage_light() {
        assert_eq!(triage_scale("Fix the typo in README"), "light");
    }

    #[test]
    fn scale_triage_standard() {
        let desc = "We need to implement a new authentication system \
                     that supports OAuth2 and SAML. The system should \
                     integrate with our existing user database and \
                     provide single sign-on capabilities across all \
                     microservices in the platform. This involves \
                     modifying the API gateway, user service, and \
                     adding a new auth service.";
        assert_eq!(triage_scale(desc), "standard");
    }

    #[test]
    fn scale_triage_full() {
        let desc = (0..200)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(triage_scale(&desc), "full");
    }
}
