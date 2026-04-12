//! E2E: full process flow lifecycle
//!
//! plan create → wave create → tasks create → task transitions through
//! pending → in_progress → submitted → plan complete → events verified.

mod lifecycle_helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use lifecycle_helpers::*;
use rusqlite::params;
use tower::ServiceExt;

#[tokio::test]
async fn test_full_process_flow_lifecycle() {
    let (pool, state, sink, _tmp) = lifecycle_setup();

    // ── Step 1: Create plan ──────────────────────────────────────
    let plan_id = create_plan(&state).await;
    assert!(plan_id > 0);

    // Verify initial status is "todo"
    let resp = router(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/plan-db/json/{plan_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["status"], "todo");

    // ── Step 2: Create wave ──────────────────────────────────────
    let wave_id = create_wave(&state, plan_id, "W-1", "wave-alpha").await;
    assert!(wave_id > 0);

    // ── Step 3: Create tasks ─────────────────────────────────────
    let t1 = create_task(&state, plan_id, wave_id, "Implement auth").await;
    let t2 = create_task(&state, plan_id, wave_id, "Write tests").await;

    // ── Step 4: Start plan (Thor pre-review gate) ────────────────
    let resp = router(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/plan-db/start/{plan_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["gate"], "ThorPreReview");

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

    let resp = router(&state)
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
    assert_eq!(json_body(resp).await["status"], "in_progress");

    // ── Step 5: Task lifecycle (pending → in_progress → submitted)
    for &tid in &[t1, t2] {
        let r = update_status(&state, tid, "in_progress").await;
        assert_eq!(r["updated"], true, "in_progress failed: {r}");

        // EvidenceGate blocks without evidence
        set_task_notes(&state, tid, "https://github.com/x/y/pull/1").await;
        let r = update_status(&state, tid, "submitted").await;
        assert!(
            r["error"].as_str().unwrap().contains("EvidenceGate"),
            "expected EvidenceGate: {r}"
        );

        // Submit through all gates
        let r = submit_task_through_gates(&state, tid).await;
        assert_eq!(r["updated"], true, "submitted failed: {r}");
    }

    // ── Step 6: Verify tasks_done counter ────────────────────────
    {
        let conn = pool.get().unwrap();
        let done: i64 = conn
            .query_row(
                "SELECT tasks_done FROM plans WHERE id = ?1",
                params![plan_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(done, 2, "tasks_done should be 2");
    }

    // ── Step 7: IPC events broadcast ─────────────────────────────
    {
        let conn = pool.get().unwrap();
        let ipc_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ipc_messages \
                 WHERE channel = '#orchestration'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ipc_count >= 2, "expected ≥2 IPC messages, got {ipc_count}");
    }

    // ── Step 8: DomainEventSink captured TaskCompleted ───────────
    {
        let events = sink.events.lock().unwrap();
        let tc = events
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    convergio_types::events::EventKind::TaskCompleted { .. }
                )
            })
            .count();
        assert_eq!(tc, 2, "expected 2 TaskCompleted events, got {tc}");
    }

    // ── Step 9: Complete plan ────────────────────────────────────
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
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"], "done");

    // Verify final status
    let resp = router(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/plan-db/json/{plan_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["status"], "done");
}
