//! E2E: lifecycle gate enforcement and multi-wave sequencing.
//!
//! - Multi-wave sequencing (wave-2 blocked until wave-1 terminal)
//! - ValidatorGate enforcement (submitted → done requires verdict)
//! - Plan cleanup (all entities reach terminal state)

mod lifecycle_helpers;

use axum::body::Body;
use axum::http::Request;
use lifecycle_helpers::*;
use rusqlite::params;
use tower::ServiceExt;

// ===========================================================================
// Multi-wave lifecycle with sequencing
// ===========================================================================

#[tokio::test]
async fn test_multi_wave_lifecycle_sequencing() {
    let (pool, state, _sink, _tmp) = lifecycle_setup();
    let plan_id = create_plan(&state).await;
    let w1 = create_wave(&state, plan_id, "W-1", "foundation").await;
    let w2 = create_wave(&state, plan_id, "W-2", "features").await;
    let t1 = create_task(&state, plan_id, w1, "Core setup").await;
    let t2 = create_task(&state, plan_id, w2, "Feature X").await;
    activate_plan(&pool, plan_id);

    // W2 task blocked while W1 pending
    let r = update_status(&state, t2, "in_progress").await;
    assert!(
        r["error"].as_str().unwrap().contains("WaveSequenceGate"),
        "W2 should be blocked: {r}"
    );

    // Complete W1 task (bypass gates for setup)
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    // W2 task now unblocked
    let r = update_status(&state, t2, "in_progress").await;
    assert_eq!(r["updated"], true, "W2 should start after W1 done: {r}");
}

// ===========================================================================
// ValidatorGate: submitted → done requires Thor verdict
// ===========================================================================

#[tokio::test]
async fn test_validator_gate_blocks_done_without_verdict() {
    let (pool, state, _sink, _tmp) = lifecycle_setup();
    let plan_id = create_plan(&state).await;
    let w = create_wave(&state, plan_id, "W-1", "w").await;
    let tid = create_task(&state, plan_id, w, "Validate me").await;
    activate_plan(&pool, plan_id);

    // Force to submitted
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![tid],
        )
        .unwrap();
    }

    // submitted → done without verdict → ValidatorGate blocks
    let r = update_status(&state, tid, "done").await;
    assert!(
        r["error"].as_str().unwrap().contains("ValidatorGate"),
        "should require verdict: {r}"
    );

    // Record a passing verdict
    {
        let conn = pool.get().unwrap();
        let qid = convergio_orchestrator::validator::enqueue_validation(
            &conn,
            Some(tid),
            Some(w),
            Some(plan_id),
        )
        .unwrap();
        convergio_orchestrator::validator::record_verdict(
            &conn,
            qid,
            "pass",
            Some("lgtm"),
            Some("thor"),
        )
        .unwrap();
    }

    // Now done succeeds
    let r = update_status(&state, tid, "done").await;
    assert_eq!(r["updated"], true, "done should pass with verdict: {r}");
}

// ===========================================================================
// Cleanup: all entities reach terminal state after plan complete
// ===========================================================================

#[tokio::test]
async fn test_plan_cleanup_all_terminal() {
    let (pool, state, sink, _tmp) = lifecycle_setup();
    let plan_id = create_plan(&state).await;
    let w = create_wave(&state, plan_id, "W-1", "only-wave").await;
    let t = create_task(&state, plan_id, w, "Only task").await;

    // Fast-forward task to done (bypass gates for speed)
    {
        let conn = pool.get().unwrap();
        conn.execute("UPDATE tasks SET status = 'done' WHERE id = ?1", params![t])
            .unwrap();
        // Set plan to in_progress so the FSM allows todo → in_progress → done
        conn.execute(
            "UPDATE plans SET status = 'in_progress' WHERE id = ?1",
            params![plan_id],
        )
        .unwrap();
    }

    // Complete the plan
    let resp = router(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/plan-db/complete/{plan_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["status"], "done");

    // Verify no non-terminal tasks remain
    {
        let conn = pool.get().unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 \
                 AND status NOT IN ('done','submitted','cancelled','skipped')",
                params![plan_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0, "no non-terminal tasks should remain");
    }

    // Verify plan is done in DB
    {
        let conn = pool.get().unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM plans WHERE id = ?1",
                params![plan_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "done");
    }

    // Verify emitted events are from orchestrator actor
    let events = sink.events.lock().unwrap();
    for ev in events.iter() {
        assert_eq!(ev.actor.name, "orchestrator");
    }
}

// ===========================================================================
// Agent identity enforcement
// ===========================================================================

#[tokio::test]
async fn test_status_change_requires_agent_id() {
    let (_pool, state, _sink, _tmp) = lifecycle_setup();
    let plan_id = create_plan(&state).await;
    let w = create_wave(&state, plan_id, "W-1", "w").await;
    let tid = create_task(&state, plan_id, w, "Agent task").await;

    let b = format!(r#"{{"task_id":{tid},"status":"in_progress"}}"#);
    let resp = router(&state)
        .oneshot(post_json("/api/plan-db/task/update", &b))
        .await
        .unwrap();
    let r = json_body(resp).await;
    assert!(
        r["error"].as_str().unwrap().contains("agent_id required"),
        "status change without agent_id should fail: {r}"
    );
}
