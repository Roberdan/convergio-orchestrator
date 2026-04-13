//! Extended plan-db routes: waves, checkpoints, evidence, execution-tree.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

pub fn plan_routes_ext(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/wave/create", post(handle_wave_create))
        .route("/api/plan-db/wave/update", post(handle_wave_update))
        .route(
            "/api/plan-db/wave/complete",
            post(crate::wave_complete_flow::handle_wave_complete),
        )
        .route("/api/plan-db/checkpoint/save", post(handle_checkpoint_save))
        .route(
            "/api/plan-db/checkpoint/restore",
            get(handle_checkpoint_restore),
        )
        .route(
            "/api/plan-db/execution-tree/:plan_id",
            get(handle_execution_tree),
        )
        .with_state(state.clone())
        .merge(crate::workspace_context_routes::workspace_context_routes(
            state.clone(),
        ))
        .merge(crate::evidence_routes::evidence_routes(state))
}

#[derive(Debug, Deserialize)]
pub struct WaveCreate {
    pub plan_id: i64,
    pub wave_id: String,
    #[serde(default)]
    pub name: String,
}

async fn handle_wave_create(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<WaveCreate>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match conn.execute(
        "INSERT INTO waves (wave_id, plan_id, name) VALUES (?1, ?2, ?3)",
        params![body.wave_id, body.plan_id, body.name],
    ) {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            Json(json!({"id": id, "wave_id": body.wave_id}))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[derive(Debug, Deserialize)]
pub struct WaveUpdate {
    pub wave_id: i64,
    pub status: String,
}

async fn handle_wave_update(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<WaveUpdate>,
) -> Json<serde_json::Value> {
    // Validate wave status is a known value
    const VALID_WAVE_STATUSES: &[&str] = &[
        "pending",
        "in_progress",
        "done",
        "cancelled",
        "failed",
        "paused",
    ];
    if !VALID_WAVE_STATUSES.contains(&body.status.as_str()) {
        return Json(json!({
            "error": format!("invalid wave status '{}'", body.status),
            "allowed": VALID_WAVE_STATUSES,
        }));
    }
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let mut sql = "UPDATE waves SET status = ?1".to_string();
    if body.status == "in_progress" {
        sql.push_str(", started_at = datetime('now')");
    }
    sql.push_str(" WHERE id = ?2");
    match conn.execute(&sql, params![body.status, body.wave_id]) {
        Ok(0) => Json(json!({"error": "wave not found"})),
        Ok(_) => Json(json!({"wave_id": body.wave_id, "status": body.status})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[derive(Debug, Deserialize)]
pub struct CheckpointSave {
    pub plan_id: i64,
}

/// Build checkpoint file path. plan_id is validated by callers to be > 0.
/// Since plan_id is i64 (integer only), no path traversal is possible.
fn checkpoint_path(plan_id: i64) -> std::path::PathBuf {
    let base = convergio_types::platform_paths::convergio_data_dir().join("checkpoints");
    base.join(format!("plan-{plan_id}.json"))
}

async fn handle_checkpoint_save(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<CheckpointSave>,
) -> Json<serde_json::Value> {
    if body.plan_id <= 0 {
        return Json(json!({"error": "invalid plan_id"}));
    }
    // Path is safe by construction: checkpoint_path() builds from integer plan_id only,
    // no user-supplied strings. No validate_path_components needed — it rejects
    // absolute paths and checkpoint_path() always returns one.
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let plan = match conn.query_row(
        "SELECT id, name, status, project_id FROM plans WHERE id = ?1",
        params![body.plan_id],
        |r| {
            Ok(
                json!({"id": r.get::<_,i64>(0)?, "name": r.get::<_,String>(1)?,
                       "status": r.get::<_,String>(2)?, "project_id": r.get::<_,String>(3)?}),
            )
        },
    ) {
        Ok(p) => p,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let checkpoint = json!({"plan_id": body.plan_id, "plan": plan,
                            "saved_at": chrono::Utc::now().to_rfc3339()});
    let path = checkpoint_path(body.plan_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(
        &path,
        serde_json::to_string_pretty(&checkpoint).unwrap_or_default(),
    ) {
        Ok(()) => Json(json!({"plan_id": body.plan_id, "saved": true,
                              "path": path.to_string_lossy()})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[derive(Debug, Deserialize)]
pub struct CheckpointQuery {
    pub plan_id: i64,
}

async fn handle_checkpoint_restore(
    State(_state): State<Arc<PlanState>>,
    Query(q): Query<CheckpointQuery>,
) -> Json<serde_json::Value> {
    if q.plan_id <= 0 {
        return Json(json!({"error": "invalid plan_id"}));
    }
    let path = checkpoint_path(q.plan_id);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let data = serde_json::from_str::<serde_json::Value>(&contents)
                .unwrap_or(json!({"raw": contents}));
            Json(json!({"plan_id": q.plan_id, "data": data}))
        }
        Err(e) => Json(json!({"error": format!("checkpoint not found: {e}")})),
    }
}

#[derive(Debug, Deserialize)]
struct TreeQuery {
    /// If true, return minimal fields (no description/notes) to save tokens.
    #[serde(default)]
    compact: bool,
}

#[tracing::instrument(skip_all, fields(%plan_id))]
async fn handle_execution_tree(
    State(state): State<Arc<PlanState>>,
    Path(plan_id): Path<i64>,
    Query(query): Query<TreeQuery>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    // Compute tasks_total from actual rows (body value can be stale)
    let tasks_total: i64 = conn
        .query_row(
            "SELECT count(*) FROM tasks WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let plan = match conn.query_row(
        "SELECT id, name, status, tasks_done FROM plans WHERE id = ?1",
        params![plan_id],
        |r| {
            Ok(
                json!({"id": r.get::<_,i64>(0)?, "name": r.get::<_,String>(1)?,
                       "status": r.get::<_,String>(2)?, "tasks_done": r.get::<_,i64>(3)?,
                       "tasks_total": tasks_total}),
            )
        },
    ) {
        Ok(p) => p,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let wave_rows = fetch_wave_ids(&conn, plan_id);
    let mut waves = Vec::new();
    for (wave_pk, wave_id, name, status) in &wave_rows {
        let tasks = fetch_tasks_for_wave(&conn, plan_id, *wave_pk, query.compact);
        waves.push(json!({
            "id": wave_pk, "wave_id": wave_id,
            "name": name, "status": status, "tasks": tasks
        }));
    }
    Json(json!({"plan": plan, "waves": waves}))
}

fn fetch_wave_ids(conn: &rusqlite::Connection, plan_id: i64) -> Vec<(i64, String, String, String)> {
    let mut stmt = match conn.prepare(
        "SELECT id, wave_id, name, status FROM waves \
         WHERE plan_id = ?1 ORDER BY id",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![plan_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

fn fetch_tasks_for_wave(
    conn: &rusqlite::Connection,
    plan_id: i64,
    wave_pk: i64,
    compact: bool,
) -> Vec<serde_json::Value> {
    if compact {
        // Compact mode: minimal fields to save tokens for agents
        let mut stmt = match conn.prepare(
            "SELECT id, task_id, title, status \
             FROM tasks WHERE plan_id = ?1 AND wave_id = ?2",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        return stmt
            .query_map(params![plan_id, wave_pk], |r| {
                Ok(json!({
                    "id": r.get::<_,i64>(0)?,
                    "task_id": r.get::<_,Option<String>>(1)?,
                    "title": r.get::<_,String>(2)?,
                    "status": r.get::<_,String>(3)?
                }))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
    }
    let mut stmt = match conn.prepare(
        "SELECT id, task_id, title, status, executor_agent, claimed_files \
         FROM tasks WHERE plan_id = ?1 AND wave_id = ?2",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![plan_id, wave_pk], |r| {
        Ok(json!({
            "id": r.get::<_,i64>(0)?,
            "task_id": r.get::<_,Option<String>>(1)?,
            "title": r.get::<_,String>(2)?,
            "status": r.get::<_,String>(3)?,
            "executor_agent": r.get::<_,Option<String>>(4)?,
            "claimed_files": r.get::<_,Option<String>>(5)?
        }))
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}
