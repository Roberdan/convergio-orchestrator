//! E2E tests for the complete gate chain, agent identity, and validate-completion.

mod gates_common;
mod helpers;

use gates_common::{
    activate_plan, create_plan, create_task, create_wave, full_app, full_setup, json_body,
    post_json, record_evidence, set_task_notes, set_task_status,
};
use rusqlite::params;
use tower::ServiceExt;

// ===========================================================================
// Complete gate chain (all gates in sequence)
// ===========================================================================

#[tokio::test]
async fn test_full_gate_chain_passes() {
    let (_pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Full chain task").await;
    activate_plan(&_pool, plan_id);

    // 1. Record evidence (EvidenceGate)
    record_evidence(&state, task_id, "test_result").await;
    // 2. Record test_pass (TestGate)
    record_evidence(&state, task_id, "test_pass").await;
    // 3. Set notes with PR URL (PrCommitGate)
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/250",
    )
    .await;

    // 4. Submit — all gates should pass
    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "full chain should pass, got: {resp}"
    );
    assert_eq!(resp["updated"], true);

    // Verify task is actually submitted in DB
    let conn = _pool.get().unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "submitted");
}

#[tokio::test]
async fn test_gate_order_evidence_before_test() {
    let (_pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Order test task").await;
    activate_plan(&_pool, plan_id);

    // No evidence at all — EvidenceGate should fire first (before TestGate)
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/251",
    )
    .await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    let err = resp["error"].as_str().unwrap();
    assert!(
        err.contains("EvidenceGate"),
        "EvidenceGate should fire first, got: {err}"
    );
}

// ===========================================================================
// Agent identity enforcement
// ===========================================================================

#[tokio::test]
async fn test_status_change_requires_agent_id() {
    let (_pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Agent-ID task").await;

    // Try status change without agent_id
    let body = format!(r#"{{"task_id":{task_id},"status":"in_progress"}}"#);
    let resp = full_app(&state)
        .oneshot(post_json("/api/plan-db/task/update", &body))
        .await
        .unwrap();
    let result = json_body(resp).await;
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("agent_id required"),
        "status change without agent_id should be rejected, got: {result}"
    );
}

// ===========================================================================
// Validate-completion (post-execution Thor review)
// ===========================================================================

#[tokio::test]
async fn test_validate_completion_finds_missing_evidence() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "No-evidence done task").await;

    // Mark task as done directly (bypassing gates for this scenario)
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
    }

    let resp = full_app(&state)
        .oneshot(post_json(
            "/api/plan-db/validate-completion",
            &format!(r#"{{"plan_id":{plan_id}}}"#),
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["verdict"], "fail");
    let findings = body["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.as_str().unwrap().contains("no evidence")),
        "should flag tasks without evidence, got: {findings:?}"
    );
}

#[tokio::test]
async fn test_validate_completion_passes_with_evidence() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Evidence done task").await;

    // Record evidence and mark done
    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
    }

    let resp = full_app(&state)
        .oneshot(post_json(
            "/api/plan-db/validate-completion",
            &format!(r#"{{"plan_id":{plan_id}}}"#),
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(
        body["verdict"], "pass",
        "should pass with evidence, got: {body}"
    );
}
