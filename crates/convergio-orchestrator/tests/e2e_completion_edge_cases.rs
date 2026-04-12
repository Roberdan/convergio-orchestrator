//! E2E tests for validate-completion and wave edge cases.

mod e2e_gate_helpers;
use e2e_gate_helpers::*;

use tower::ServiceExt;

// ===========================================================================
// Validate-completion (post-execution Thor review)
// ===========================================================================

#[tokio::test]
async fn test_validate_completion_finds_missing_evidence() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "No-evidence done task").await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
    }

    let resp = full_app(&state)
        .oneshot(make_post(
            "/api/plan-db/validate-completion",
            &format!(r#"{{"plan_id":{plan_id}}}"#),
        ))
        .await
        .unwrap();
    let body = extract_json(resp).await;
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
        .oneshot(make_post(
            "/api/plan-db/validate-completion",
            &format!(r#"{{"plan_id":{plan_id}}}"#),
        ))
        .await
        .unwrap();
    let body = extract_json(resp).await;
    assert_eq!(
        body["verdict"], "pass",
        "should pass with evidence, got: {body}"
    );
}

// ===========================================================================
// Edge cases
// ===========================================================================

#[tokio::test]
async fn test_task_with_no_wave_bypasses_wave_gate() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    activate_plan(&pool, plan_id);

    let task_id: i64 = {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO tasks (task_id, plan_id, wave_id, title, status) \
             VALUES ('T-orphan', ?1, NULL, 'Orphan task', 'pending')",
            params![plan_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    };

    let resp = set_task_status(&state, task_id, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "task without wave should bypass wave gate, got: {resp}"
    );
}

#[tokio::test]
async fn test_multiple_tasks_in_wave1_all_must_complete() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1a = create_task(&state, plan_id, w1, "W1 task A").await;
    let t1b = create_task(&state, plan_id, w1, "W1 task B").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;

    // Activate AFTER creating all entities (ImportGate blocks adds on active plans)
    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![t1a],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("WaveSequenceGate"),
        "W2 should be blocked with one W1 task still pending, got: {resp}"
    );

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![t1b],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "W2 should be allowed after both W1 tasks done, got: {resp}"
    );
}

#[tokio::test]
async fn test_cancelled_wave1_tasks_dont_block_wave2() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 cancelled task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;

    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'cancelled' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "cancelled is terminal — should not block W2, got: {resp}"
    );
}

#[tokio::test]
async fn test_skipped_wave1_tasks_dont_block_wave2() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 skipped task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;

    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'skipped' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "skipped is terminal — should not block W2, got: {resp}"
    );
}
