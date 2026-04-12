//! PR dependency chain — tracks merge ordering constraints.
//!
//! - `POST /api/merge/dependencies` — declare dependencies between PRs
//! - `GET  /api/merge/queue`        — pending merges with dependency status

use axum::extract::State;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use convergio_db::pool::ConnPool;

/// Build the PR dependency routes.
pub fn pr_dependency_routes(pool: ConnPool) -> Router {
    Router::new()
        .route("/api/merge/dependencies", post(add_dependency))
        .route("/api/merge/dependency-queue", get(dependency_queue))
        .with_state(pool)
}

#[derive(Debug, Deserialize)]
struct DependencyBody {
    pr_url: String,
    depends_on: Vec<String>,
}

async fn add_dependency(
    State(pool): State<ConnPool>,
    Json(body): Json<DependencyBody>,
) -> Json<Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    let mut added = 0usize;
    for dep_url in &body.depends_on {
        match conn.execute(
            "INSERT OR IGNORE INTO pr_dependencies \
             (pr_url, depends_on_url) VALUES (?1, ?2)",
            params![body.pr_url, dep_url],
        ) {
            Ok(n) => added += n,
            Err(e) => {
                return Json(json!({"error": e.to_string()}));
            }
        }
    }

    Json(json!({
        "ok": true,
        "pr_url": body.pr_url,
        "dependencies_added": added,
    }))
}

async fn dependency_queue(State(pool): State<ConnPool>) -> Json<Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // Get all PRs that have dependencies
    let mut stmt = match conn.prepare("SELECT DISTINCT pr_url FROM pr_dependencies ORDER BY pr_url")
    {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    let pr_urls: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    let mut queue = Vec::new();
    for pr_url in &pr_urls {
        let deps = list_deps_for(&conn, pr_url);
        let all_satisfied = deps.iter().all(|d| d.satisfied);
        queue.push(json!({
            "pr_url": pr_url,
            "dependencies": deps.iter().map(|d| json!({
                "depends_on_url": d.depends_on_url,
                "satisfied": d.satisfied,
            })).collect::<Vec<_>>(),
            "ready": all_satisfied,
        }));
    }

    Json(json!({"ok": true, "queue": queue}))
}

struct DepInfo {
    depends_on_url: String,
    satisfied: bool,
}

fn list_deps_for(conn: &rusqlite::Connection, pr_url: &str) -> Vec<DepInfo> {
    let mut stmt = match conn.prepare(
        "SELECT depends_on_url, satisfied FROM pr_dependencies \
         WHERE pr_url = ?1",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![pr_url], |row| {
        Ok(DepInfo {
            depends_on_url: row.get(0)?,
            satisfied: row.get::<_, i64>(1)? != 0,
        })
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Mark a dependency as satisfied (called when a PR is merged).
pub fn mark_satisfied(conn: &rusqlite::Connection, merged_pr_url: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE pr_dependencies SET satisfied = 1 \
         WHERE depends_on_url = ?1",
        params![merged_pr_url],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_pool() -> ConnPool {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        for m in crate::schema::migrations() {
            conn.execute_batch(m.up).unwrap();
        }
        for m in crate::schema_pr_deps::pr_deps_migrations() {
            conn.execute_batch(m.up).unwrap();
        }
        pool
    }

    #[test]
    fn pr_dependency_routes_build() {
        let pool = setup_pool();
        let _router = pr_dependency_routes(pool);
    }

    #[test]
    fn mark_satisfied_updates_rows() {
        let pool = setup_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO pr_dependencies (pr_url, depends_on_url) \
             VALUES ('pr-2', 'pr-1')",
            [],
        )
        .unwrap();
        let updated = mark_satisfied(&conn, "pr-1").unwrap();
        assert_eq!(updated, 1);
        let satisfied: bool = conn
            .query_row(
                "SELECT satisfied FROM pr_dependencies WHERE pr_url = 'pr-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(satisfied);
    }

    #[test]
    fn list_deps_empty_returns_empty() {
        let pool = setup_pool();
        let conn = pool.get().unwrap();
        let deps = list_deps_for(&conn, "nonexistent");
        assert!(deps.is_empty());
    }
}
