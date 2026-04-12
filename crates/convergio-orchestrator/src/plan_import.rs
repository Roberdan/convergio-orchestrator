//! POST /api/plan-db/import — parse spec YAML, create waves + tasks.
//! Core types and insertion logic in plan_import_core.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use axum::routing::post;
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use crate::plan_import_core::{import_waves_and_tasks, parse_spec};
use crate::plan_routes::PlanState;

pub fn import_routes(state: Arc<PlanState>) -> Router {
    Router::new()
        .route("/api/plan-db/import", post(handle_import))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct ImportRequest {
    plan_id: i64,
    #[serde(default)]
    spec: Option<String>,
    #[serde(default)]
    source_file: Option<String>,
    #[serde(default = "default_import_mode")]
    import_mode: String,
}

fn default_import_mode() -> String {
    "append".to_string()
}

async fn handle_import(
    State(state): State<Arc<PlanState>>,
    Json(body): Json<ImportRequest>,
) -> Json<serde_json::Value> {
    let mode = body.import_mode.as_str();
    if !matches!(mode, "append" | "replace" | "merge") {
        return Json(json!({
            "error": format!("invalid import_mode '{mode}', must be: append, replace, merge"),
        }));
    }

    let spec_str = match resolve_spec(&body) {
        Ok(s) => s,
        Err(e) => return Json(e),
    };

    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    if let Err(e) = crate::gates::import_gate(&conn, body.plan_id) {
        return Json(json!({"error": format!("{}: {}", e.gate, e.reason)}));
    }

    let spec = match parse_spec(&spec_str) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": format!("YAML parse: {e}")})),
    };

    // Replace mode: check for evidence, then clear
    if mode == "replace" {
        let has_evidence: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM task_evidence e \
                 JOIN tasks t ON e.task_db_id = t.id WHERE t.plan_id = ?1)",
                params![body.plan_id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_evidence {
            return Json(json!({
                "error": "cannot replace: plan has tasks with recorded evidence",
                "hint": "Use 'append' or 'merge' mode, or remove evidence first",
            }));
        }
        let _ = conn.execute(
            "DELETE FROM tasks WHERE plan_id = ?1",
            params![body.plan_id],
        );
        let _ = conn.execute(
            "DELETE FROM waves WHERE plan_id = ?1",
            params![body.plan_id],
        );
    }

    let stats = import_waves_and_tasks(&conn, body.plan_id, &spec, mode);

    let mut result = json!({
        "plan_id": body.plan_id,
        "import_mode": mode,
        "waves_created": stats.waves_created,
        "tasks_created": stats.tasks_created,
    });
    if stats.waves_skipped > 0 || stats.tasks_skipped > 0 {
        result["waves_skipped"] = json!(stats.waves_skipped);
        result["tasks_skipped"] = json!(stats.tasks_skipped);
    }
    if !stats.errors.is_empty() {
        result["errors"] = json!(stats.errors);
    }
    Json(result)
}

fn resolve_spec(body: &ImportRequest) -> Result<String, serde_json::Value> {
    if let Some(ref path) = body.source_file {
        let canonical = std::path::Path::new(path)
            .canonicalize()
            .map_err(|e| json!({"error": format!("invalid source_file path: {e}")}))?;
        let cwd = std::env::current_dir().unwrap_or_default();
        if !canonical.starts_with(&cwd) {
            return Err(json!({"error": "source_file must be under the daemon working directory"}));
        }
        std::fs::read_to_string(&canonical)
            .map_err(|e| json!({"error": format!("read source_file: {e}")}))
    } else if let Some(ref s) = body.spec {
        Ok(s.clone())
    } else {
        Err(json!({"error": "either `spec` or `source_file` must be provided"}))
    }
}
