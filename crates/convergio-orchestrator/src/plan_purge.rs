//! Purge cancelled/filtered plans and all related rows.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

#[derive(Debug, Deserialize)]
pub struct PurgeBody {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub name_prefix: Option<String>,
}

/// POST /api/plan-db/purge — permanently delete plans matching filters.
///
/// Defaults to `status = "cancelled"`. Optionally filter by `name_prefix`.
/// Cascade-deletes related rows in tasks, waves, plan_metadata, etc.
#[tracing::instrument(skip_all)]
pub async fn handle_purge(
    State(s): State<Arc<PlanState>>,
    Json(body): Json<PurgeBody>,
) -> Json<serde_json::Value> {
    let conn = match s.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let status = body.status.as_deref().unwrap_or("cancelled");

    let ids: Vec<i64> = if let Some(ref prefix) = body.name_prefix {
        let like = format!("{prefix}%");
        let mut stmt = match conn.prepare("SELECT id FROM plans WHERE status = ?1 AND name LIKE ?2")
        {
            Ok(s) => s,
            Err(e) => return Json(json!({"error": e.to_string()})),
        };
        stmt.query_map(params![status, like], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    } else {
        let mut stmt = match conn.prepare("SELECT id FROM plans WHERE status = ?1") {
            Ok(s) => s,
            Err(e) => return Json(json!({"error": e.to_string()})),
        };
        stmt.query_map(params![status], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };

    if ids.is_empty() {
        return Json(json!({"purged": 0, "status": status}));
    }

    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let refs: Vec<Box<dyn rusqlite::types::ToSql>> = ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let ps: Vec<&dyn rusqlite::types::ToSql> = refs.iter().map(|b| b.as_ref()).collect();

    let mut details = serde_json::Map::new();

    // Delete task_evidence via task IDs (before deleting tasks)
    let te_sql = format!(
        "DELETE FROM task_evidence WHERE task_db_id IN \
         (SELECT id FROM tasks WHERE plan_id IN ({placeholders}))"
    );
    exec_best_effort(&conn, &te_sql, &ps, "task_evidence", &mut details);

    // Cascade delete tables that have plan_id column
    let tables = [
        "tasks",
        "waves",
        "plan_metadata",
        "plan_evaluations",
        "validation_queue",
        "review_outcomes",
        "scheduling_decisions",
        "approval_requests",
        "compensation_actions",
    ];
    for table in &tables {
        let sql = format!("DELETE FROM {table} WHERE plan_id IN ({placeholders})");
        exec_best_effort(&conn, &sql, &ps, table, &mut details);
    }

    // Delete plans themselves
    let sql = format!("DELETE FROM plans WHERE id IN ({placeholders})");
    let purged = conn.execute(&sql, ps.as_slice()).unwrap_or(0);

    Json(json!({
        "purged": purged,
        "status": status,
        "details": details,
    }))
}

fn exec_best_effort(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn rusqlite::types::ToSql],
    label: &str,
    details: &mut serde_json::Map<String, serde_json::Value>,
) {
    match conn.execute(sql, params) {
        Ok(n) => {
            details.insert(label.to_string(), json!(n));
        }
        Err(_) => {
            details.insert(label.to_string(), json!("skip"));
        }
    }
}
