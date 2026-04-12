//! Shared helpers for E2E gate tests.
//!
//! Provides `full_setup()` with file-backed SQLite (multi-connection safe),
//! plus convenience functions to create plans, waves, tasks and record evidence.
#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use convergio_orchestrator::plan_routes::PlanState;
use tower::ServiceExt;

// Re-export helpers from the sibling `helpers` module declared by each test crate root.
pub use crate::helpers::{app, json_body, post_json};

// ---------------------------------------------------------------------------
// Router with validation + review routes
// ---------------------------------------------------------------------------

/// Full router including validation and review routes.
/// Note: evidence_routes are already included via plan_routes_ext.
pub fn full_app(state: &Arc<PlanState>) -> Router {
    use convergio_orchestrator::plan_review::review_routes;
    use convergio_orchestrator::plan_validate::validate_routes;

    app(state)
        .merge(validate_routes(state.clone()))
        .merge(review_routes(state.clone()))
}

// ---------------------------------------------------------------------------
// Entity creation helpers
// ---------------------------------------------------------------------------

/// Create a plan via the API, returning its DB id.
pub async fn create_plan(state: &Arc<PlanState>) -> i64 {
    let resp = full_app(state)
        .oneshot(post_json(
            "/api/plan-db/create",
            r#"{"project_id":"proj-1","name":"gate-test-plan",
                "objective":"test gates","motivation":"CI","requester":"test"}"#,
        ))
        .await
        .unwrap();
    json_body(resp).await["id"].as_i64().unwrap()
}

/// Create a wave under `plan_id`, returning the wave DB id.
pub async fn create_wave(state: &Arc<PlanState>, plan_id: i64, wave_id: &str, name: &str) -> i64 {
    let body = format!(r#"{{"plan_id":{plan_id},"wave_id":"{wave_id}","name":"{name}"}}"#);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/wave/create", &body))
        .await
        .unwrap();
    json_body(resp).await["id"].as_i64().unwrap()
}

/// Create a task under `plan_id` / `wave_db_id`, returning its DB id.
pub async fn create_task(
    state: &Arc<PlanState>,
    plan_id: i64,
    wave_db_id: i64,
    title: &str,
) -> i64 {
    let body = format!(r#"{{"plan_id":{plan_id},"wave_id":{wave_db_id},"title":"{title}"}}"#);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/task/create", &body))
        .await
        .unwrap();
    json_body(resp).await["id"].as_i64().unwrap()
}

/// Record evidence for a task.
pub async fn record_evidence(
    state: &Arc<PlanState>,
    task_db_id: i64,
    evidence_type: &str,
) -> serde_json::Value {
    let body = format!(
        r#"{{"task_db_id":{task_db_id},"evidence_type":"{evidence_type}",
            "command":"cargo test","output_summary":"all passed","exit_code":0}}"#,
    );
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/task/evidence", &body))
        .await
        .unwrap();
    json_body(resp).await
}

/// Update task notes.
pub async fn set_task_notes(
    state: &Arc<PlanState>,
    task_id: i64,
    notes: &str,
) -> serde_json::Value {
    let body = format!(r#"{{"task_id":{task_id},"notes":"{notes}"}}"#);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/task/update", &body))
        .await
        .unwrap();
    json_body(resp).await
}

/// Update task status (requires agent_id).
pub async fn set_task_status(
    state: &Arc<PlanState>,
    task_id: i64,
    status: &str,
    agent_id: &str,
) -> serde_json::Value {
    let body = format!(r#"{{"task_id":{task_id},"status":"{status}","agent_id":"{agent_id}"}}"#);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/task/update", &body))
        .await
        .unwrap();
    json_body(resp).await
}

// ---------------------------------------------------------------------------
// Database setup
// ---------------------------------------------------------------------------

/// Create a temp-file pool with enough connections for concurrent access.
/// Each test gets its own unique database file to avoid cross-test interference.
fn create_test_pool() -> (convergio_db::pool::ConnPool, tempfile::TempDir) {
    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let manager = SqliteConnectionManager::file(&db_path).with_init(|conn| {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    });
    let pool = Pool::builder().max_size(4).build(manager).unwrap();
    (pool, tmp)
}

/// Set plan status to 'in_progress' directly (bypasses review gate for testing).
pub fn activate_plan(pool: &convergio_db::pool::ConnPool, plan_id: i64) {
    let conn = pool.get().unwrap();
    conn.execute(
        "UPDATE plans SET status = 'in_progress' WHERE id = ?1",
        rusqlite::params![plan_id],
    )
    .unwrap();
}

/// Full setup: apply all migrations including evidence + gate tables.
/// Uses a temp-file pool to avoid pool exhaustion when handler and gate check
/// both acquire connections concurrently (in-memory pool has max_size=1).
/// Returns TempDir -- caller must hold it to keep the DB alive.
pub fn full_setup() -> (
    convergio_db::pool::ConnPool,
    Arc<PlanState>,
    tempfile::TempDir,
) {
    let (pool, tmp) = create_test_pool();
    let conn = pool.get().unwrap();
    convergio_db::migration::ensure_registry(&conn).unwrap();
    let migs = convergio_orchestrator::schema::migrations();
    convergio_db::migration::apply_migrations(&conn, "orchestrator", &migs).unwrap();
    let ipc_raw = convergio_ipc::schema::migrations();
    for m in ipc_raw {
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
    let evidence_migs = convergio_evidence::schema::migrations();
    convergio_db::migration::apply_migrations(&conn, "evidence", &evidence_migs).unwrap();
    drop(conn);
    let state = Arc::new(PlanState {
        pool: pool.clone(),
        event_sink: None,
        notify: Arc::new(tokio::sync::Notify::new()),
    });
    (pool, state, tmp)
}
