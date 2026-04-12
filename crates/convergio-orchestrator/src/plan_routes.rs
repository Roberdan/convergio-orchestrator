//! Plan-db CRUD: list, create, json, start, complete, cancel.
use axum::extract::{Path, Query, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use convergio_db::pool::ConnPool;
use convergio_types::events::DomainEventSink;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub struct PlanState {
    pub pool: ConnPool,
    pub event_sink: Option<Arc<dyn DomainEventSink>>,
    pub notify: Arc<tokio::sync::Notify>,
}

pub fn plan_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/list", get(handle_list))
        .route("/api/plan-db/create", post(handle_create))
        .route("/api/plan-db/json/:plan_id", get(handle_get))
        .route("/api/plan-db/start/:plan_id", post(handle_start))
        .route("/api/plan-db/complete/:plan_id", post(handle_complete))
        .route("/api/plan-db/cancel/:plan_id", post(handle_cancel))
        .route("/api/plan-db/resume/:plan_id", post(handle_resume))
        .route("/api/plan-db/purge", post(crate::plan_purge::handle_purge))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub project_id: Option<String>,
    pub limit: Option<i64>,
}

#[tracing::instrument(skip_all)]
async fn handle_list(
    State(state): State<Arc<PlanState>>,
    Query(q): Query<ListQuery>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let mut sql = "SELECT id, project_id, name, status, tasks_done, tasks_total, \
                   created_at, updated_at FROM plans"
        .to_string();
    let mut conds: Vec<String> = Vec::new();
    let mut p: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(ref s) = q.status {
        if s != "all" {
            p.push(Box::new(s.clone()));
            conds.push(format!("status = ?{}", p.len()));
        }
    } else {
        // Default: only active plans (not done/cancelled/failed)
        conds.push("status IN ('todo','in_progress','active','paused')".to_string());
    }
    if let Some(ref pid) = q.project_id {
        p.push(Box::new(pid.clone()));
        conds.push(format!("project_id = ?{}", p.len()));
    }
    if !conds.is_empty() {
        sql.push_str(&format!(" WHERE {}", conds.join(" AND ")));
    }
    let max_rows = q.limit.unwrap_or(100).clamp(1, 1000);
    sql.push_str(&format!(" ORDER BY id DESC LIMIT {max_rows}"));
    let refs: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|v| v.as_ref()).collect();
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let plans: Vec<serde_json::Value> = match stmt.query_map(refs.as_slice(), |row| {
        Ok(json!({
            "id": row.get::<_, i64>(0)?,
            "project_id": row.get::<_, String>(1)?,
            "name": row.get::<_, String>(2)?,
            "status": row.get::<_, String>(3)?,
            "tasks_done": row.get::<_, i64>(4)?,
            "tasks_total": row.get::<_, i64>(5)?,
            "created_at": row.get::<_, String>(6)?,
            "updated_at": row.get::<_, String>(7)?,
        }))
    }) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    Json(json!(plans))
}

#[derive(Debug, Deserialize)]
pub struct CreatePlan {
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub depends_on: Option<String>,
    #[serde(default)]
    pub execution_mode: Option<String>,
    #[serde(default)]
    pub tasks_total: i64,
    pub objective: String,
    pub motivation: String,
    pub requester: String,
    /// Agent that created this plan — used for planner/executor separation (#703).
    #[serde(default)]
    pub planner_agent_id: Option<String>,
}

#[tracing::instrument(skip_all, fields(project_id = %body.project_id))]
async fn handle_create(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<CreatePlan>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match conn.execute(
        "INSERT INTO plans (project_id, name, depends_on, execution_mode, tasks_total, planner_agent_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            body.project_id,
            body.name,
            body.depends_on,
            body.execution_mode,
            body.tasks_total,
            body.planner_agent_id.as_deref().unwrap_or(""),
        ],
    ) {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            if let Some(ref sink) = state.event_sink {
                sink.emit(convergio_types::events::make_event(
                    "orchestrator",
                    convergio_types::events::EventKind::PlanCreated {
                        plan_id: id,
                        name: body.name.clone(),
                    },
                    convergio_types::events::EventContext {
                        plan_id: Some(id),
                        ..Default::default()
                    },
                ));
            }
            // Always create plan_metadata (32c: protocol fields required)
            let _ = conn.execute(
                "INSERT OR IGNORE INTO plan_metadata (plan_id, objective, motivation, requester) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, body.objective, body.motivation, body.requester],
            );
            Json(json!({"id": id, "status": "created"}))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

#[tracing::instrument(skip_all, fields(%plan_id))]
async fn handle_get(
    State(state): State<Arc<PlanState>>,
    Path(plan_id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let plan = conn.query_row(
        "SELECT id, project_id, name, status, tasks_done, tasks_total, \
         depends_on, execution_mode, created_at, updated_at FROM plans WHERE id = ?1",
        params![plan_id],
        |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "project_id": row.get::<_, String>(1)?,
                "name": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "tasks_done": row.get::<_, i64>(4)?,
                "tasks_total": row.get::<_, i64>(5)?,
                "depends_on": row.get::<_, Option<String>>(6)?,
                "execution_mode": row.get::<_, Option<String>>(7)?,
                "created_at": row.get::<_, String>(8)?,
                "updated_at": row.get::<_, String>(9)?,
            }))
        },
    );
    match plan {
        Ok(p) => Json(p),
        Err(rusqlite::Error::QueryReturnedNoRows) => Json(json!({"error": "plan not found"})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

fn set_plan_status(pool: &ConnPool, plan_id: i64, status: &str) -> serde_json::Value {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };
    // Read current status and validate transition via FSM
    let current: String = match conn.query_row(
        "SELECT status FROM plans WHERE id = ?1",
        params![plan_id],
        |r| r.get(0),
    ) {
        Ok(s) => s,
        Err(rusqlite::Error::QueryReturnedNoRows) => return json!({"error": "plan not found"}),
        Err(e) => return json!({"error": e.to_string()}),
    };
    if let Err(reason) = crate::plan_state::validate_plan_transition(&current, status) {
        return json!({"error": reason, "current_status": current, "requested_status": status});
    }
    match conn.execute(
        "UPDATE plans SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![status, plan_id],
    ) {
        Ok(0) => json!({"error": "plan not found"}),
        Ok(_) => json!({"id": plan_id, "status": status}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

#[tracing::instrument(skip_all, fields(plan_id = %id))]
async fn handle_start(
    State(s): State<Arc<PlanState>>,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    let conn = match s.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    // StartGate: must have at least 1 task
    if let Err(e) = crate::gates::start_gate(&conn, id) {
        return Json(
            json!({"error": format!("gate blocked: {e}"), "gate": e.gate, "expected": e.expected}),
        );
    }
    // Thor pre-review: check plan_metadata.report_json has a passing pre_review
    let has_review: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM plan_metadata WHERE plan_id = ?1 \
             AND report_json LIKE '%\"verdict\":\"pass\"%' \
             AND report_json LIKE '%pre_review%')",
            params![id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_review {
        return Json(json!({
            "error": "Thor pre-review required. Call POST /api/plan-db/review first.",
            "gate": "ThorPreReview"
        }));
    }
    drop(conn);
    let result = set_plan_status(&s.pool, id, "in_progress");
    // Also start the first pending wave so the plan executor can pick up tasks.
    // Without this, waves stay 'pending' and executor_tick finds nothing.
    if result.get("status").and_then(|v| v.as_str()) == Some("in_progress") {
        if let Ok(conn) = s.pool.get() {
            let first_wave: Option<i64> = conn
                .query_row(
                    "SELECT id FROM waves WHERE plan_id = ?1 AND status = 'pending' \
                     ORDER BY id ASC LIMIT 1",
                    params![id],
                    |r| r.get(0),
                )
                .ok();
            if let Some(wave_id) = first_wave {
                let _ = conn.execute(
                    "UPDATE waves SET status = 'in_progress', started_at = datetime('now') \
                     WHERE id = ?1",
                    params![wave_id],
                );
                tracing::info!("handle_start: started wave {wave_id} for plan {id}");
            }
        }
    }
    Json(result)
}

async fn handle_complete(
    State(s): State<Arc<PlanState>>,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    Json(set_plan_status(&s.pool, id, "done"))
}

async fn handle_cancel(
    State(s): State<Arc<PlanState>>,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    Json(set_plan_status(&s.pool, id, "cancelled"))
}

/// Resume a paused plan (set by boot safety or manual pause).
async fn handle_resume(
    State(s): State<Arc<PlanState>>,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    let Ok(conn) = s.pool.get() else {
        return Json(json!({"error": "db pool"}));
    };
    let status: String = conn
        .query_row("SELECT status FROM plans WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .unwrap_or_default();
    if status != "paused" && status != "stale" {
        return Json(json!({"error": format!("plan {id} is '{status}', not paused/stale")}));
    }
    Json(set_plan_status(&s.pool, id, "in_progress"))
}
