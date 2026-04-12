//! E2E tests for WaveSequenceGate and gate chain ordering.

mod e2e_gate_helpers;
use e2e_gate_helpers::*;

// ===========================================================================
// WaveSequenceGate
// ===========================================================================

#[tokio::test]
async fn test_wave2_task_blocked_when_wave1_incomplete() {
    let (_pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let _t1 = create_task(&state, plan_id, w1, "W1 task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&_pool, plan_id);

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("WaveSequenceGate"),
        "expected WaveSequenceGate error, got: {resp}"
    );
}

#[tokio::test]
async fn test_wave2_task_allowed_when_wave1_done() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "expected W2 task to start after W1 done, got: {resp}"
    );
}

#[tokio::test]
async fn test_wave2_task_allowed_when_wave1_submitted() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "submitted is terminal for wave gate, got: {resp}"
    );
}

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

    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/250",
    )
    .await;

    let resp = set_task_status(&state, task_id, "submitted", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "full chain should pass, got: {resp}"
    );
    assert_eq!(resp["updated"], true);

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
