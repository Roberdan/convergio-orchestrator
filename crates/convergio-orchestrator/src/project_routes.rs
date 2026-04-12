//! Dashboard project CRUD — GET/POST /api/dashboard/projects, project tree.

use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use convergio_db::pool::ConnPool;

pub fn project_routes(pool: ConnPool) -> Router {
    Router::new()
        .route("/api/dashboard/projects", get(list_projects))
        .route("/api/dashboard/projects", post(create_project))
        .route("/api/project/:project_id/tree", get(project_tree))
        .with_state(pool)
}

async fn list_projects(State(pool): State<ConnPool>) -> Json<serde_json::Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, name, description, output_path, created_at, updated_at \
         FROM projects ORDER BY name",
    ) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "description": row.get::<_, Option<String>>(2)?,
                "output_path": row.get::<_, Option<String>>(3)?,
                "created_at": row.get::<_, String>(4)?,
                "updated_at": row.get::<_, String>(5)?,
            }))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    Json(json!(rows))
}

#[derive(Debug, Deserialize)]
struct CreateProjectBody {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    input_path: Option<String>,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn create_project(
    State(pool): State<ConnPool>,
    Json(body): Json<CreateProjectBody>,
) -> Json<serde_json::Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let id = slug(&body.name);
    let output = body
        .output_path
        .as_deref()
        .or(body.path.as_deref())
        .unwrap_or("");
    match conn.execute(
        "INSERT INTO projects (id, name, description, output_path) VALUES (?1, ?2, ?3, ?4)",
        params![id, body.name, body.description, output],
    ) {
        Ok(_) => Json(json!({"id": id, "name": body.name, "status": "created"})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

async fn project_tree(
    State(pool): State<ConnPool>,
    Path(project_id): Path<String>,
) -> Json<serde_json::Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match crate::plan_hierarchy::project_plan_tree(&conn, &project_id) {
        Ok(tree) => Json(serde_json::to_value(tree).unwrap_or(json!({"error": "serialize"}))),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}
