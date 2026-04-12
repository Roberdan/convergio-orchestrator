//! MergeGuardian — POST /api/merge/request
//!
//! Accepts a merge request with PR metadata and files changed.
//! Checks open PRs for file overlap and returns allow/block.

use axum::extract::State;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use convergio_db::pool::ConnPool;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn merge_guardian_routes(pool: ConnPool) -> Router {
    Router::new()
        .route("/api/merge/request", post(merge_request))
        .route("/api/merge/queue", get(list_queue))
        .with_state(pool)
}

#[derive(Debug, Deserialize)]
struct MergeRequestBody {
    pr_number: i64,
    branch: String,
    files_changed: Vec<String>,
}

async fn merge_request(
    State(pool): State<ConnPool>,
    Json(body): Json<MergeRequestBody>,
) -> Json<Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    // Find overlapping files from other open merge requests
    let overlaps = find_overlaps(&conn, body.pr_number, &body.files_changed);

    let decision = if overlaps.is_empty() {
        "allow"
    } else {
        "block"
    };

    // Record this merge request in the queue
    let files_json = serde_json::to_string(&body.files_changed).unwrap_or_default();
    let overlaps_json = serde_json::to_string(&overlaps).unwrap_or_default();

    if let Err(e) = conn.execute(
        "INSERT INTO merge_queue \
         (pr_number, branch, files_json, decision, overlaps_json) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(pr_number) DO UPDATE SET \
         branch = excluded.branch, \
         files_json = excluded.files_json, \
         decision = excluded.decision, \
         overlaps_json = excluded.overlaps_json, \
         updated_at = datetime('now')",
        params![
            body.pr_number,
            body.branch,
            files_json,
            decision,
            overlaps_json
        ],
    ) {
        return Json(json!({"error": e.to_string()}));
    }

    Json(json!({
        "pr_number": body.pr_number,
        "decision": decision,
        "overlapping_files": overlaps,
    }))
}

/// Check open merge requests for file overlap.
fn find_overlaps(conn: &rusqlite::Connection, pr_number: i64, files: &[String]) -> Vec<Value> {
    let mut stmt = match conn.prepare(
        "SELECT pr_number, branch, files_json \
         FROM merge_queue \
         WHERE pr_number != ?1 AND status = 'open'",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let rows: Vec<(i64, String, String)> = stmt
        .query_map(params![pr_number], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    let mut overlaps = Vec::new();
    for (other_pr, other_branch, other_files_json) in &rows {
        let other_files: Vec<String> = serde_json::from_str(other_files_json).unwrap_or_default();
        let shared: Vec<&String> = files.iter().filter(|f| other_files.contains(f)).collect();
        if !shared.is_empty() {
            overlaps.push(json!({
                "pr_number": other_pr,
                "branch": other_branch,
                "shared_files": shared,
            }));
        }
    }
    overlaps
}

async fn list_queue(State(pool): State<ConnPool>) -> Json<Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    let mut stmt = match conn.prepare(
        "SELECT pr_number, branch, files_json, decision, \
         overlaps_json, status, created_at, updated_at \
         FROM merge_queue ORDER BY created_at DESC LIMIT 50",
    ) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };

    let rows: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(json!({
                "pr_number": row.get::<_, i64>(0)?,
                "branch": row.get::<_, String>(1)?,
                "files_changed": row.get::<_, String>(2)?,
                "decision": row.get::<_, String>(3)?,
                "overlaps": row.get::<_, Option<String>>(4)?,
                "status": row.get::<_, String>(5)?,
                "created_at": row.get::<_, String>(6)?,
                "updated_at": row.get::<_, String>(7)?,
            }))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    Json(json!({"queue": rows}))
}
