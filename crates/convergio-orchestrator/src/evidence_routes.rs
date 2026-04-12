//! Evidence recording routes for task lifecycle.
//!
//! - POST /api/plan-db/task/evidence       — record task evidence
//! - GET  /api/plan-db/task/evidence/:id   — get task evidence

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

pub fn evidence_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/task/evidence", post(handle_evidence_create))
        .route(
            "/api/plan-db/task/evidence/:task_id",
            get(handle_evidence_get),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct EvidenceCreate {
    pub task_db_id: i64,
    pub evidence_type: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub output_summary: String,
    #[serde(default)]
    pub exit_code: i32,
}

#[tracing::instrument(skip_all, fields(task_db_id = %body.task_db_id))]
async fn handle_evidence_create(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<EvidenceCreate>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    };
    // Pre-check: task must exist and be in_progress
    let task_status: Option<String> = conn
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            params![body.task_db_id],
            |r| r.get(0),
        )
        .ok();
    let Some(status) = task_status else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error": "task not found", "task_db_id": body.task_db_id})),
        );
    };
    if status == "done" || status == "cancelled" {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "cannot record evidence on a terminal task",
                "task_db_id": body.task_db_id,
                "status": status
            })),
        );
    }
    // Duplicate check (allow multiple test_result, reject other duplicates)
    if body.evidence_type != "test_result" {
        let dup: i64 = conn
            .query_row(
                "SELECT count(*) FROM task_evidence \
                 WHERE task_db_id = ?1 AND evidence_type = ?2",
                params![body.task_db_id, body.evidence_type],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if dup > 0 {
            return (
                axum::http::StatusCode::CONFLICT,
                Json(json!({
                    "error": "duplicate evidence",
                    "evidence_type": body.evidence_type,
                    "task_db_id": body.task_db_id
                })),
            );
        }
    }
    match conn.execute(
        "INSERT INTO task_evidence \
         (task_db_id, evidence_type, command, output_summary, exit_code) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            body.task_db_id,
            body.evidence_type,
            body.command,
            body.output_summary,
            body.exit_code
        ],
    ) {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            (
                axum::http::StatusCode::OK,
                Json(json!({"id": id, "task_db_id": body.task_db_id})),
            )
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

#[tracing::instrument(skip_all, fields(%task_id))]
async fn handle_evidence_get(
    State(state): State<Arc<PlanState>>,
    Path(task_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, evidence_type, command, output_summary, exit_code \
         FROM task_evidence WHERE task_db_id = ?1 ORDER BY id",
    ) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let rows: Vec<serde_json::Value> = stmt
        .query_map(params![task_id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "type": r.get::<_, String>(1)?,
                "command": r.get::<_, String>(2)?,
                "summary": r.get::<_, String>(3)?,
                "exit_code": r.get::<_, i32>(4)?
            }))
        })
        .unwrap_or_else(|_| panic!("evidence query failed"))
        .filter_map(|r| r.ok())
        .collect();
    Json(json!({"task_db_id": task_id, "evidence": rows}))
}
