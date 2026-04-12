//! E2E tests for ValidatorGate, Thor promotion, counters, and agent identity.

mod e2e_gate_helpers;
use e2e_gate_helpers::*;

use axum::body::Body;
use axum::http::Request;
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

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
    }

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

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
    }

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

    let initial_done: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row(
            "SELECT tasks_done FROM plans WHERE id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap()
    };

    record_evidence(&state, task_id, "test_result").await;
    record_evidence(&state, task_id, "test_pass").await;
    set_task_notes(
        &state,
        task_id,
        "https://github.com/Roberdan/convergio/pull/260",
    )
    .await;
    set_task_status(&state, task_id, "submitted", "agent-test").await;

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

#[tokio::test]
async fn test_plan_validate_endpoint_promotes_submitted_tasks() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let task_id = create_task(&state, plan_id, wave_id, "Thor endpoint task").await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![task_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE waves SET status = 'in_progress' WHERE id = ?1",
            params![wave_id],
        )
        .unwrap();
    }

    let resp = full_app(&state)
        .oneshot(make_post(
            "/api/plan-db/validate",
            &format!(r#"{{"plan_id":{plan_id}}}"#),
        ))
        .await
        .unwrap();
    let body = extract_json(resp).await;
    assert_eq!(body["thor"], "pass");

    let conn = pool.get().unwrap();
    let task_status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    let wave_status: String = conn
        .query_row(
            "SELECT status FROM waves WHERE id = ?1",
            params![wave_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(task_status, "done");
    assert_eq!(wave_status, "done");
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
    let body = extract_json(resp).await;
    assert_eq!(body["gate"], "ThorPreReview");
}

#[tokio::test]
async fn test_plan_start_allowed_after_thor_review() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let _task_id = create_task(&state, plan_id, wave_id, "Task 1").await;

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
    let body = extract_json(resp).await;
    assert_eq!(body["status"], "in_progress");
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

    let body = format!(r#"{{"task_id":{task_id},"status":"in_progress"}}"#);
    let resp = full_app(&state)
        .oneshot(make_post("/api/plan-db/task/update", &body))
        .await
        .unwrap();
    let result = extract_json(resp).await;
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("agent_id required"),
        "status change without agent_id should be rejected, got: {result}"
    );
}
