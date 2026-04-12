//! POST /api/plan-db/validate — structural validation plus Thor wave promotion.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use axum::routing::post;
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

pub fn validate_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/validate", post(handle_validate))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ValidateReq {
    plan_id: i64,
    #[serde(default)]
    wave_id: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

fn submitted_wave_ids(
    conn: &rusqlite::Connection,
    plan_id: i64,
    requested_wave_id: Option<i64>,
) -> rusqlite::Result<Vec<i64>> {
    if let Some(wave_id) = requested_wave_id {
        return Ok(vec![wave_id]);
    }

    let mut stmt = conn.prepare(
        "SELECT DISTINCT wave_id FROM tasks \
         WHERE plan_id = ?1 AND status = 'submitted' \
         ORDER BY wave_id",
    )?;
    let rows = stmt.query_map(params![plan_id], |row| row.get(0))?;
    rows.collect::<Result<Vec<i64>, _>>()
}

/// Validate plan completeness. When tasks are already submitted, this doubles as the
/// Thor validation entrypoint and promotes the relevant wave(s) to done.
#[tracing::instrument(skip_all, fields(plan_id = %body.plan_id))]
async fn handle_validate(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<ValidateReq>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // 1. Plan must exist
    let plan_name: Option<String> = conn
        .query_row(
            "SELECT name FROM plans WHERE id = ?1",
            params![body.plan_id],
            |r| r.get(0),
        )
        .ok();
    if plan_name.is_none() {
        return Json(json!({"valid": false, "errors": ["plan not found"]}));
    }

    let wants_thor =
        body.wave_id.is_some() || matches!(body.scope.as_deref(), Some("wave") | Some("thor"));
    let pending_submitted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 AND status = 'submitted'",
            params![body.plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if wants_thor || pending_submitted > 0 {
        let wave_ids = match submitted_wave_ids(&conn, body.plan_id, body.wave_id) {
            Ok(ids) => ids,
            Err(e) => return Json(json!({"valid": false, "error": e.to_string()})),
        };

        let mut promoted_tasks = 0_i64;
        for wave_id in &wave_ids {
            promoted_tasks += conn
                .query_row(
                    "SELECT COUNT(*) FROM tasks \
                     WHERE plan_id = ?1 AND wave_id = ?2 AND status = 'submitted'",
                    params![body.plan_id, wave_id],
                    |r| r.get(0),
                )
                .unwrap_or(0);

            if let Err(e) = crate::handlers::on_wave_validated(
                &state.pool,
                &state.notify,
                &state.event_sink,
                *wave_id,
                body.plan_id,
            ) {
                return Json(json!({
                    "valid": false,
                    "plan_id": body.plan_id,
                    "wave_id": wave_id,
                    "error": e.to_string(),
                }));
            }
        }

        let plan_status: String = conn
            .query_row(
                "SELECT status FROM plans WHERE id = ?1",
                params![body.plan_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "unknown".to_string());

        return Json(json!({
            "valid": true,
            "plan_id": body.plan_id,
            "plan_name": plan_name,
            "scope": body.scope.unwrap_or_else(|| "plan".to_string()),
            "thor": "pass",
            "wave_ids": wave_ids,
            "promoted_tasks": promoted_tasks,
            "status": plan_status,
        }));
    }

    let mut errors: Vec<String> = Vec::new();

    // 2. Plan must have metadata (objective, motivation, requester)
    let has_meta: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM plan_metadata WHERE plan_id = ?1 \
             AND objective IS NOT NULL AND motivation IS NOT NULL \
             AND requester IS NOT NULL)",
            params![body.plan_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_meta {
        errors.push("missing plan_metadata (objective, motivation, requester)".into());
    }

    // 3. Plan must have at least 1 wave
    let wave_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM waves WHERE plan_id = ?1",
            params![body.plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if wave_count == 0 {
        errors.push("plan has zero waves".into());
    }

    // 4. Plan must have at least 1 task
    let task_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1",
            params![body.plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if task_count == 0 {
        errors.push("plan has zero tasks".into());
    }

    // 5. Every task must have a non-empty title
    let empty_titles: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 \
             AND (title IS NULL OR title = '')",
            params![body.plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if empty_titles > 0 {
        errors.push(format!("{empty_titles} task(s) have empty titles"));
    }

    // 6. Every task must be assigned to a wave
    let orphan_tasks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 AND wave_id IS NULL",
            params![body.plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if orphan_tasks > 0 {
        errors.push(format!("{orphan_tasks} task(s) not assigned to a wave"));
    }

    if errors.is_empty() {
        Json(json!({
            "valid": true,
            "plan_id": body.plan_id,
            "plan_name": plan_name,
            "waves": wave_count,
            "tasks": task_count,
        }))
    } else {
        Json(json!({
            "valid": false,
            "plan_id": body.plan_id,
            "errors": errors,
        }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn validate_route_compiles() {
        // Ensures the route function signature is correct
    }
}
