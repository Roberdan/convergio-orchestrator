//! Shared helpers for lifecycle E2E tests.
//!
//! Provides: mock event sink, full DB setup with all migrations,
//! router builder, and CRUD helper functions.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::Request;
use convergio_types::events::{DomainEvent, DomainEventSink};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Mock event sink — captures all emitted DomainEvents
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct MockEventSink {
    pub events: Arc<Mutex<Vec<DomainEvent>>>,
}

impl DomainEventSink for MockEventSink {
    fn emit(&self, event: DomainEvent) {
        self.events.lock().unwrap().push(event);
    }
}

pub type SetupResult = (
    convergio_db::pool::ConnPool,
    Arc<convergio_orchestrator::plan_routes::PlanState>,
    MockEventSink,
    tempfile::TempDir,
);

/// Full setup with event capture: file-backed DB, all migrations, mock sink.
#[allow(dead_code)]
pub fn lifecycle_setup() -> SetupResult {
    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("lifecycle.db");
    let manager = SqliteConnectionManager::file(&db_path).with_init(|conn| {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    });
    let pool = Pool::builder().max_size(4).build(manager).unwrap();

    let conn = pool.get().unwrap();
    convergio_db::migration::ensure_registry(&conn).unwrap();
    for (ns, migs) in [
        ("orchestrator", convergio_orchestrator::schema::migrations()),
        ("evidence", convergio_evidence::schema::migrations()),
    ] {
        convergio_db::migration::apply_migrations(&conn, ns, &migs).unwrap();
    }
    for m in convergio_ipc::schema::migrations() {
        convergio_db::migration::apply_migrations(
            &conn,
            "ipc",
            &[convergio_types::extension::Migration {
                version: m.version,
                description: m.description,
                up: m.up,
            }],
        )
        .unwrap();
    }
    drop(conn);

    let sink = MockEventSink::default();
    let state = Arc::new(convergio_orchestrator::plan_routes::PlanState {
        pool: pool.clone(),
        event_sink: Some(Arc::new(sink.clone())),
        notify: Arc::new(tokio::sync::Notify::new()),
    });
    (pool, state, sink, tmp)
}

/// Full router: plan + plan_ext + task + validate + review routes.
#[allow(dead_code)]
pub fn router(state: &Arc<convergio_orchestrator::plan_routes::PlanState>) -> axum::Router {
    use convergio_orchestrator::plan_review::review_routes;
    use convergio_orchestrator::plan_routes::plan_routes;
    use convergio_orchestrator::plan_routes_ext::plan_routes_ext;
    use convergio_orchestrator::plan_validate::validate_routes;
    use convergio_orchestrator::task_routes::task_routes;

    plan_routes(state.clone())
        .merge(plan_routes_ext(state.clone()))
        .merge(task_routes(state.clone()))
        .merge(validate_routes(state.clone()))
        .merge(review_routes(state.clone()))
}

// ---------------------------------------------------------------------------
// CRUD helpers
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub async fn create_plan(state: &Arc<convergio_orchestrator::plan_routes::PlanState>) -> i64 {
    let resp = router(state)
        .oneshot(post_json(
            "/api/plan-db/create",
            r#"{"project_id":"p","name":"lifecycle-plan",
                "objective":"o","motivation":"m","requester":"bot"}"#,
        ))
        .await
        .unwrap();
    json_body(resp).await["id"].as_i64().unwrap()
}

#[allow(dead_code)]
pub async fn create_wave(
    state: &Arc<convergio_orchestrator::plan_routes::PlanState>,
    plan_id: i64,
    wid: &str,
    name: &str,
) -> i64 {
    let b = format!(r#"{{"plan_id":{plan_id},"wave_id":"{wid}","name":"{name}"}}"#);
    let r = router(state)
        .oneshot(post_json("/api/plan-db/wave/create", &b))
        .await
        .unwrap();
    json_body(r).await["id"].as_i64().unwrap()
}

#[allow(dead_code)]
pub async fn create_task(
    state: &Arc<convergio_orchestrator::plan_routes::PlanState>,
    plan_id: i64,
    wave_id: i64,
    title: &str,
) -> i64 {
    let b = format!(r#"{{"plan_id":{plan_id},"wave_id":{wave_id},"title":"{title}"}}"#);
    let r = router(state)
        .oneshot(post_json("/api/plan-db/task/create", &b))
        .await
        .unwrap();
    json_body(r).await["id"].as_i64().unwrap()
}

#[allow(dead_code)]
pub async fn record_evidence(
    state: &Arc<convergio_orchestrator::plan_routes::PlanState>,
    task_db_id: i64,
    evidence_type: &str,
) {
    let b = format!(
        r#"{{"task_db_id":{task_db_id},"evidence_type":"{evidence_type}",
            "command":"cargo test","output_summary":"ok","exit_code":0}}"#,
    );
    router(state)
        .oneshot(post_json("/api/plan-db/task/evidence", &b))
        .await
        .unwrap();
}

#[allow(dead_code)]
pub async fn set_task_notes(
    state: &Arc<convergio_orchestrator::plan_routes::PlanState>,
    task_id: i64,
    notes: &str,
) {
    let b = format!(r#"{{"task_id":{task_id},"notes":"{notes}"}}"#);
    router(state)
        .oneshot(post_json("/api/plan-db/task/update", &b))
        .await
        .unwrap();
}

#[allow(dead_code)]
pub async fn update_status(
    state: &Arc<convergio_orchestrator::plan_routes::PlanState>,
    task_id: i64,
    status: &str,
) -> serde_json::Value {
    let b = format!(r#"{{"task_id":{task_id},"status":"{status}","agent_id":"e2e-bot"}}"#);
    let r = router(state)
        .oneshot(post_json("/api/plan-db/task/update", &b))
        .await
        .unwrap();
    json_body(r).await
}

/// Set plan status to 'in_progress' directly (bypasses review gate for testing).
#[allow(dead_code)]
pub fn activate_plan(pool: &convergio_db::pool::ConnPool, plan_id: i64) {
    let conn = pool.get().unwrap();
    conn.execute(
        "UPDATE plans SET status = 'in_progress' WHERE id = ?1",
        rusqlite::params![plan_id],
    )
    .unwrap();
}

/// Submit a task through gates: evidence + test_pass + PR notes + status.
#[allow(dead_code)]
pub async fn submit_task_through_gates(
    state: &Arc<convergio_orchestrator::plan_routes::PlanState>,
    task_id: i64,
) -> serde_json::Value {
    record_evidence(state, task_id, "test_result").await;
    record_evidence(state, task_id, "test_pass").await;
    set_task_notes(state, task_id, "https://github.com/x/y/pull/1").await;
    update_status(state, task_id, "submitted").await
}

// ---------------------------------------------------------------------------
// Inline HTTP helpers (avoids cross-module resolution issues)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub fn post_json(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[allow(dead_code)]
pub async fn json_body(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
