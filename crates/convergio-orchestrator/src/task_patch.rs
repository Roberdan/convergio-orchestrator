//! PATCH /api/plan-db/task/:id — update task content fields.
//! Only allowed on plans in todo/draft status.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::json;

use crate::plan_routes::PlanState;

#[derive(Debug, Deserialize)]
pub struct TaskContentUpdate {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub executor_agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort_level: Option<i64>,
    #[serde(default)]
    pub test_criteria: Option<String>,
    #[serde(default)]
    pub verify: Option<Vec<String>>,
}

#[tracing::instrument(skip_all, fields(task_id = %id))]
pub async fn handle_patch_task_content(
    State(state): State<Arc<PlanState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(body): Json<TaskContentUpdate>,
) -> Response {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})).into_response(),
    };

    // Look up the task's plan and check plan status
    let plan_info = conn.query_row(
        "SELECT t.plan_id, p.status FROM tasks t \
         JOIN plans p ON t.plan_id = p.id WHERE t.id = ?1",
        rusqlite::params![id],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    );
    let (plan_id, plan_status) = match plan_info {
        Ok(info) => info,
        Err(_) => return Json(json!({"error": "task not found"})).into_response(),
    };

    if !matches!(plan_status.as_str(), "todo" | "draft") {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": format!("cannot edit task content: plan status is '{plan_status}'"),
                "hint": "Task content can only be edited on plans in todo/draft status",
                "plan_id": plan_id,
            })),
        )
            .into_response();
    }

    // Build column updates
    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref t) = body.title {
        params.push(Box::new(t.clone()));
        sets.push(format!("title = ?{}", params.len()));
    }
    if let Some(ref d) = body.description {
        params.push(Box::new(d.clone()));
        sets.push(format!("description = ?{}", params.len()));
    }
    if let Some(ref a) = body.executor_agent {
        params.push(Box::new(a.clone()));
        sets.push(format!("executor_agent = ?{}", params.len()));
    }

    // Update metadata JSON for model/effort/verify/test_criteria
    let needs_meta = body.model.is_some()
        || body.effort_level.is_some()
        || body.verify.is_some()
        || body.test_criteria.is_some();

    if needs_meta {
        let existing_meta: String = conn
            .query_row(
                "SELECT COALESCE(metadata, '{}') FROM tasks WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "{}".to_string());

        let mut meta: serde_json::Value = serde_json::from_str(&existing_meta).unwrap_or(json!({}));

        if let Some(ref m) = body.model {
            meta["model"] = json!(m);
        }
        if let Some(e) = body.effort_level {
            meta["effort"] = json!(e);
        }
        if let Some(ref v) = body.verify {
            meta["verify"] = json!(v);
        }
        if let Some(ref tc) = body.test_criteria {
            meta["test_criteria"] = json!(tc);
        }

        params.push(Box::new(meta.to_string()));
        sets.push(format!("metadata = ?{}", params.len()));
    }

    if sets.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "no fields to update"})),
        )
            .into_response();
    }

    params.push(Box::new(id));
    let sql = format!(
        "UPDATE tasks SET {} WHERE id = ?{}",
        sets.join(", "),
        params.len()
    );
    let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|v| v.as_ref()).collect();
    match conn.execute(&sql, refs.as_slice()) {
        Ok(0) => Json(json!({"error": "task not found"})).into_response(),
        Ok(_) => Json(json!({"task_id": id, "updated": true, "plan_id": plan_id})).into_response(),
        Err(e) => Json(json!({"error": e.to_string()})).into_response(),
    }
}
