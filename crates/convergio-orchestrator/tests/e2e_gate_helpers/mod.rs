//! Shared helpers for e2e gate tests.
#![allow(unused_imports, dead_code)]

#[path = "../helpers/mod.rs"]
mod helpers;

use helpers::{app, json_body, post_json};
use tower::ServiceExt;

pub use axum::body::Body;
pub use axum::http::{Request, StatusCode};
pub use helpers::{json_body as extract_json, post_json as make_post};
pub use rusqlite::params;

/// Full router including validation and review routes.
pub fn full_app(
    state: &std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
) -> axum::Router {
    use convergio_orchestrator::plan_review::review_routes;
    use convergio_orchestrator::plan_validate::validate_routes;

    app(state)
        .merge(validate_routes(state.clone()))
        .merge(review_routes(state.clone()))
}

pub async fn create_plan(
    state: &std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
) -> i64 {
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

pub async fn create_wave(
    state: &std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
    plan_id: i64,
    wave_id: &str,
    name: &str,
) -> i64 {
    let body = format!(r#"{{"plan_id":{plan_id},"wave_id":"{wave_id}","name":"{name}"}}"#,);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/wave/create", &body))
        .await
        .unwrap();
    json_body(resp).await["id"].as_i64().unwrap()
}

pub async fn create_task(
    state: &std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
    plan_id: i64,
    wave_db_id: i64,
    title: &str,
) -> i64 {
    let body = format!(r#"{{"plan_id":{plan_id},"wave_id":{wave_db_id},"title":"{title}"}}"#,);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/task/create", &body))
        .await
        .unwrap();
    let j = json_body(resp).await;
    j["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("create_task: expected id in response, got: {j}"))
}

pub async fn record_evidence(
    state: &std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
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

pub async fn set_task_notes(
    state: &std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
    task_id: i64,
    notes: &str,
) -> serde_json::Value {
    let body = format!(r#"{{"task_id":{task_id},"notes":"{notes}"}}"#,);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/task/update", &body))
        .await
        .unwrap();
    json_body(resp).await
}

pub async fn set_task_status(
    state: &std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
    task_id: i64,
    status: &str,
    agent_id: &str,
) -> serde_json::Value {
    let body = format!(r#"{{"task_id":{task_id},"status":"{status}","agent_id":"{agent_id}"}}"#,);
    let resp = full_app(state)
        .oneshot(post_json("/api/plan-db/task/update", &body))
        .await
        .unwrap();
    json_body(resp).await
}

/// Set plan status to 'in_progress' directly (bypasses review gate for testing).
pub fn activate_plan(pool: &convergio_db::pool::ConnPool, plan_id: i64) {
    let conn = pool.get().unwrap();
    conn.execute(
        "UPDATE plans SET status = 'in_progress' WHERE id = ?1",
        params![plan_id],
    )
    .unwrap();
}

pub fn create_test_pool() -> (convergio_db::pool::ConnPool, tempfile::TempDir) {
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

/// Full setup: apply all migrations including evidence + gate tables.
pub fn full_setup() -> (
    convergio_db::pool::ConnPool,
    std::sync::Arc<convergio_orchestrator::plan_routes::PlanState>,
    tempfile::TempDir,
) {
    let (pool, tmp) = create_test_pool();
    let conn = pool.get().unwrap();
    convergio_db::migration::ensure_registry(&conn).unwrap();
    let base = convergio_orchestrator::schema::migrations();
    convergio_db::migration::apply_migrations(&conn, "orchestrator", &base).unwrap();
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
    let state = std::sync::Arc::new(convergio_orchestrator::plan_routes::PlanState {
        pool: pool.clone(),
        event_sink: None,
        notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    });
    (pool, state, tmp)
}
