//! Atomic task completion endpoint — any tool/model makes ONE call.
//!
//! POST /api/plan-db/task/complete-flow
//! Internally: set notes → record evidence → record test_pass → submit.
//! All gates checked. Returns clear error if any gate fails.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

#[derive(Debug, Deserialize)]
pub struct CompleteFlowRequest {
    pub task_db_id: i64,
    pub agent_id: String,
    /// PR URL or commit hash — required by PrCommitGate.
    pub pr_url: String,
    /// Shell command used for testing (e.g. "cargo test -p convergio-cli").
    #[serde(default)]
    pub test_command: String,
    /// Summary of test output.
    #[serde(default)]
    pub test_output: String,
    /// Exit code of test command (0 = pass).
    #[serde(default)]
    pub test_exit_code: i32,
    /// Optional free-form notes.
    #[serde(default)]
    pub notes: Option<String>,
}

/// Atomic task completion: notes → evidence → test_pass → submit.
pub async fn handle_complete_flow(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<CompleteFlowRequest>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // Step 1: Set notes with PR URL (PrCommitGate reads DB before status change)
    let notes = if let Some(ref extra) = body.notes {
        format!("{} — {extra}", body.pr_url)
    } else {
        body.pr_url.clone()
    };

    // Wave PR guard: block duplicate PRs for the same wave (#703)
    // Skip for cross-repo waves where tasks have 'direct_to_main' in notes (#986)
    if body.pr_url.contains("/pull/") {
        let is_cross_repo = notes.contains("direct_to_main");
        if !is_cross_repo {
            let wave_pr: Option<(i64, String)> = conn
                .query_row(
                    "SELECT t2.wave_id, t2.notes FROM tasks t1 \
                     JOIN tasks t2 ON t2.wave_id = t1.wave_id \
                     WHERE t1.id = ?1 AND t2.id != ?1 \
                     AND t2.notes LIKE 'https://github.com/%/pull/%' LIMIT 1",
                    params![body.task_db_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            if let Some((wave_id, existing)) = wave_pr {
                let existing_url = existing.split_whitespace().next().unwrap_or(&existing);
                if !body.pr_url.starts_with(existing_url) {
                    return Json(json!({
                        "error": format!("Wave {wave_id} already has PR: {existing_url}. One PR per wave (Learning #25)."),
                        "step": "wave_pr_guard"
                    }));
                }
            }
        }
    }

    if let Err(e) = conn.execute(
        "UPDATE tasks SET notes = ?1 WHERE id = ?2",
        params![notes, body.task_db_id],
    ) {
        return Json(json!({"error": format!("set notes: {e}"), "step": "notes"}));
    }

    // Step 2: Record evidence — test_result
    if let Err(e) = conn.execute(
        "INSERT INTO task_evidence \
         (task_db_id, evidence_type, command, output_summary, exit_code) \
         VALUES (?1, 'test_result', ?2, ?3, ?4)",
        params![
            body.task_db_id,
            body.test_command,
            body.test_output,
            body.test_exit_code
        ],
    ) {
        return Json(
            json!({"error": format!("evidence test_result: {e}"), "step": "evidence_test_result"}),
        );
    }

    // Step 3: Record evidence — test_pass
    if let Err(e) = conn.execute(
        "INSERT INTO task_evidence \
         (task_db_id, evidence_type, command, output_summary, exit_code) \
         VALUES (?1, 'test_pass', ?2, ?3, ?4)",
        params![
            body.task_db_id,
            body.test_command,
            body.test_output,
            body.test_exit_code
        ],
    ) {
        return Json(
            json!({"error": format!("evidence test_pass: {e}"), "step": "evidence_test_pass"}),
        );
    }

    // Step 4: Check all gates before status transition
    if let Err(gate_err) =
        crate::gates::check_task_transition(&state.pool, body.task_db_id, "submitted")
    {
        return Json(json!({
            "error": format!("gate blocked: {gate_err}"),
            "gate": gate_err.gate,
            "expected": gate_err.expected,
            "step": "gate_check"
        }));
    }

    // Step 5: Update status to submitted
    if let Err(e) = conn.execute(
        "UPDATE tasks SET status = 'submitted', \
         completed_at = datetime('now') WHERE id = ?1",
        params![body.task_db_id],
    ) {
        return Json(json!({"error": format!("submit: {e}"), "step": "submit"}));
    }

    // Audit trail
    crate::audit::log_status_change(
        &conn,
        body.task_db_id,
        "in_progress",
        "submitted",
        &body.agent_id,
        Some(&notes),
    );

    // Emit lifecycle event
    drop(conn);
    crate::task_lifecycle::emit_task_lifecycle(&state, body.task_db_id);

    Json(json!({
        "task_db_id": body.task_db_id,
        "status": "submitted",
        "evidence_recorded": true,
        "notes_set": true,
        "gates_passed": true
    }))
}
