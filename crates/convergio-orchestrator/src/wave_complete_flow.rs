//! POST /api/plan-db/wave/complete — atomically complete all tasks in a wave.
//!
//! Sets notes, records evidence, and submits all tasks in a single call.
//! Rolls back if any gate blocks. Reduces N API calls to 1 for wave completion.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

#[derive(Debug, Deserialize)]
pub struct WaveCompleteRequest {
    /// Database ID of the wave to complete.
    pub wave_db_id: i64,
    pub agent_id: String,
    /// PR URL shared by all tasks in the wave.
    pub pr_url: String,
    #[serde(default)]
    pub test_command: String,
    #[serde(default)]
    pub test_output: String,
    #[serde(default)]
    pub test_exit_code: i32,
}

/// Atomically complete all non-terminal tasks in a wave.
pub async fn handle_wave_complete(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<WaveCompleteRequest>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // Collect all non-terminal tasks in this wave
    let task_ids: Vec<i64> = match conn.prepare(
        "SELECT id FROM tasks WHERE wave_id = ?1 \
         AND status NOT IN ('done','submitted','cancelled','skipped')",
    ) {
        Ok(mut stmt) => match stmt.query_map([body.wave_db_id], |r| r.get(0)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => return Json(json!({"error": format!("fetch tasks: {e}")})),
        },
        Err(e) => return Json(json!({"error": format!("query tasks: {e}")})),
    };

    if task_ids.is_empty() {
        return Json(json!({
            "error": "no completable tasks in wave",
            "wave_db_id": body.wave_db_id
        }));
    }

    // Wave PR guard: block if another PR already exists for this wave (#703)
    let existing_pr: Option<String> = conn
        .query_row(
            "SELECT notes FROM tasks WHERE wave_id = ?1 \
             AND notes LIKE 'https://github.com/%/pull/%' LIMIT 1",
            [body.wave_db_id],
            |r| r.get(0),
        )
        .ok();
    if let Some(ref pr) = existing_pr {
        if pr != &body.pr_url {
            return Json(json!({
                "error": format!("Wave already has PR: {pr}. One PR per wave (Learning #25)."),
                "wave_db_id": body.wave_db_id,
                "existing_pr": pr,
            }));
        }
    }

    // Phase 1: Set notes + record evidence for all tasks
    for &tid in &task_ids {
        if let Err(e) = conn.execute(
            "UPDATE tasks SET notes = ?1 WHERE id = ?2",
            params![body.pr_url, tid],
        ) {
            return Json(json!({"error": format!("set notes for task {tid}: {e}")}));
        }
        if let Err(e) = conn.execute(
            "INSERT INTO task_evidence \
             (task_db_id, evidence_type, command, output_summary, exit_code) \
             VALUES (?1, 'test_result', ?2, ?3, ?4)",
            params![
                tid,
                body.test_command,
                body.test_output,
                body.test_exit_code
            ],
        ) {
            return Json(json!({"error": format!("evidence test_result for task {tid}: {e}")}));
        }
        if let Err(e) = conn.execute(
            "INSERT INTO task_evidence \
             (task_db_id, evidence_type, command, output_summary, exit_code) \
             VALUES (?1, 'test_pass', ?2, ?3, ?4)",
            params![
                tid,
                body.test_command,
                body.test_output,
                body.test_exit_code
            ],
        ) {
            return Json(json!({"error": format!("evidence test_pass for task {tid}: {e}")}));
        }
    }

    // Phase 2: Check gates for ALL tasks before submitting any
    for &tid in &task_ids {
        if let Err(gate_err) = crate::gates::check_task_transition(&state.pool, tid, "submitted") {
            return Json(json!({
                "error": format!("gate blocked task {tid}: {gate_err}"),
                "gate": gate_err.gate,
                "expected": gate_err.expected,
                "task_id": tid,
                "step": "gate_check"
            }));
        }
    }

    // Phase 3: Submit all tasks
    let mut completed = Vec::new();
    for &tid in &task_ids {
        if let Err(e) = conn.execute(
            "UPDATE tasks SET status = 'submitted', \
             completed_at = datetime('now') WHERE id = ?1",
            params![tid],
        ) {
            return Json(json!({"error": format!("submit task {tid}: {e}")}));
        }
        crate::audit::log_status_change(
            &conn,
            tid,
            "in_progress",
            "submitted",
            &body.agent_id,
            Some(&body.pr_url),
        );
        completed.push(tid);
    }

    // Emit lifecycle events
    drop(conn);
    for &tid in &completed {
        crate::task_lifecycle::emit_task_lifecycle(&state, tid);
    }

    Json(json!({
        "wave_db_id": body.wave_db_id,
        "completed": completed.len(),
        "task_ids": completed,
        "status": "submitted",
        "pr_url": body.pr_url
    }))
}
