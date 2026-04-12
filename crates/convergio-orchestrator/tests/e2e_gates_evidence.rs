//! E2E tests for EvidenceGate, TestGate, and PrCommitGate.

mod gates_common;
mod helpers;

use gates_common::{
    activate_plan, create_plan, create_task, create_wave, full_setup, record_evidence,
    set_task_notes, set_task_status,
};

// ===========================================================================
// EvidenceGate
// ===========================================================================

#[tokio::test]
async fn test_submit_without_evidence_fails() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "No-evidence task").await;
    activate_plan(&pool, plan_id);

    // Set notes with PR URL (passes PrCommitGate) but skip evidence
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/200",
    )
    .await;

    // Try to submit — should fail at EvidenceGate
    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("EvidenceGate"),
        "expected EvidenceGate error, got: {resp}"
    );
}

#[tokio::test]
async fn test_submit_with_evidence_passes_evidence_gate() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Evidence task").await;
    activate_plan(&pool, plan_id);

    // Record evidence + test_pass, set notes with PR URL
    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/201",
    )
    .await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "expected success, got error: {resp}"
    );
    assert_eq!(resp["updated"], true);
}

// ===========================================================================
// TestGate
// ===========================================================================

#[tokio::test]
async fn test_submit_without_test_pass_fails() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Test-gate task").await;
    activate_plan(&pool, plan_id);

    // Record test_result evidence but NOT test_pass
    record_evidence(&state, task_id, "test_result").await;
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/202",
    )
    .await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("TestGate"),
        "expected TestGate error, got: {resp}"
    );
}

#[tokio::test]
async fn test_submit_with_test_pass_passes() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Test-pass task").await;
    activate_plan(&pool, plan_id);

    // Record both test_result AND test_pass
    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/203",
    )
    .await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "expected success, got error: {resp}"
    );
}

// ===========================================================================
// PrCommitGate
// ===========================================================================

#[tokio::test]
async fn test_submit_without_pr_in_notes_fails() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "No-PR task").await;
    activate_plan(&pool, plan_id);

    // Record evidence (passes EvidenceGate + TestGate)
    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    // Set notes WITHOUT PR URL or commit hash
    set_task_notes(
        &state,
        task_id,
        "just some random notes without a reference",
    )
    .await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("PrCommitGate"),
        "expected PrCommitGate error, got: {resp}"
    );
}

#[tokio::test]
async fn test_submit_with_pr_url_passes() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "PR-URL task").await;
    activate_plan(&pool, plan_id);

    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/204",
    )
    .await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "expected success with PR URL, got: {resp}"
    );
}

#[tokio::test]
async fn test_submit_with_commit_hash_passes() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Commit-hash task").await;
    activate_plan(&pool, plan_id);

    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    set_task_notes(&state, task_id, "Fixed in a1b2c3d4e5f6789").await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "expected success with commit hash, got: {resp}"
    );
}
