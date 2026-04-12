//! E2E tests for Thor validation, pre-review, and validate-completion.

mod gates_common;
mod helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gates_common::{
    activate_plan, create_plan, create_task, create_wave, full_app, full_setup, json_body,
    record_evidence, set_task_notes, set_task_status,
};
use rusqlite::params;
use tower::ServiceExt;

// ===========================================================================
// Thor validation (ValidatorGate + promotion)
// ===========================================================================

#[tokio::test]
async fn test_done_without_validator_verdict_fails() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Validator task").await;
    activate_plan(&pool, plan_id);

    // Move to submitted first (bypass by setting status directly in DB)
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
    }

    // Try to promote to done via API — should fail at ValidatorGate
    let resp = set_task_status(&state, task_id, "done", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("ValidatorGate"),
        "expected ValidatorGate error, got: {resp}"
    );
}

#[tokio::test]
async fn test_done_with_passing_verdict_succeeds() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Validator pass task").await;
    activate_plan(&pool, plan_id);

    // Move to submitted
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
    }

    // Create a validation queue entry and record a passing verdict
    {
        let conn = pool.get().unwrap();
        let queue_id = convergio_orchestrator::validator::enqueue_validation(
            &conn,
            Some(task_id),
            Some(wave_id),
            Some(plan_id),
        )
        .unwrap();
        convergio_orchestrator::validator::record_verdict(
            &conn,
            queue_id,
            "pass",
            Some("looks good"),
            Some("thor"),
        )
        .unwrap();
    }

    // Now promote to done — should pass ValidatorGate
    let resp = set_task_status(&state, task_id, "done", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "expected success with passing verdict, got: {resp}"
    );
}

#[tokio::test]
async fn test_tasks_done_counter_increments_on_submit() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Counter task").await;
    activate_plan(&pool, plan_id);

    // Check initial tasks_done
    let initial_done: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row(
            "SELECT tasks_done FROM plans WHERE id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap()
    };

    // Complete the full gate chain and submit
    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/260",
    )
    .await;
    set_task_status(&state, task_id, "submitted", "agent-test").await;

    // tasks_done should have incremented
    let final_done: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row(
            "SELECT tasks_done FROM plans WHERE id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        final_done,
        initial_done + 1,
        "tasks_done should increment on submit"
    );
}

// ===========================================================================
// Thor pre-review required to start plan
// ===========================================================================

#[tokio::test]
async fn test_plan_start_blocked_without_thor_review() {
    let (_pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let _task_id = create_task(&state, plan_id, wave_id, "Task 1").await;

    let resp = full_app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/plan-db/start/{plan_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["gate"], "ThorPreReview");
}

#[tokio::test]
async fn test_plan_start_allowed_after_thor_review() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let _task_id = create_task(&state, plan_id, wave_id, "Task 1").await;

    // Insert passing Thor pre-review
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE plan_metadata SET report_json = ?1 WHERE plan_id = ?2",
            params![
                r#"{"pre_review":true,"verdict":"pass","notes":"ok"}"#,
                plan_id
            ],
        )
        .unwrap();
    }

    let resp = full_app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/plan-db/start/{plan_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["status"], "in_progress");
}
