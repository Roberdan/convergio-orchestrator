//! Knowledge base search and write routes.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use convergio_db::pool::ConnPool;

pub fn kb_routes(pool: ConnPool) -> Router {
    let state = Arc::new(KbState { pool });
    Router::new()
        .route("/api/plan-db/kb-search", get(handle_kb_search))
        .route("/api/plan-db/kb-write", post(handle_kb_write))
        .with_state(state)
}

struct KbState {
    pool: ConnPool,
}

#[derive(Debug, Deserialize)]
struct KbQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    domain: Option<String>,
}
fn default_limit() -> i64 {
    10
}

/// Simple relevance score: title exact > title contains > content contains.
fn relevance_score(query: &str, title: &str, content: &str) -> f64 {
    let q = query.to_lowercase();
    let t = title.to_lowercase();
    let c = content.to_lowercase();
    if t == q {
        1.0
    } else if t.contains(&q) {
        0.85
    } else if c.contains(&q) {
        0.6
    } else {
        0.3
    }
}

async fn handle_kb_search(
    State(state): State<Arc<KbState>>,
    Query(q): Query<KbQuery>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let pattern = format!("%{}%", q.q);

    let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) =
        if let Some(ref dom) = q.domain {
            (
                "SELECT id, domain, title, content, created_at FROM knowledge_base \
             WHERE (title LIKE ?1 OR content LIKE ?1) AND (domain = ?3 OR domain = ?4) \
             ORDER BY hit_count DESC LIMIT ?2",
                vec![
                    Box::new(pattern.clone()),
                    Box::new(q.limit),
                    Box::new(dom.clone()),
                    Box::new(format!("org:{dom}")),
                ],
            )
        } else {
            (
                "SELECT id, domain, title, content, created_at FROM knowledge_base \
             WHERE title LIKE ?1 OR content LIKE ?1 OR domain LIKE ?1 \
             ORDER BY hit_count DESC LIMIT ?2",
                vec![Box::new(pattern.clone()), Box::new(q.limit)],
            )
        };

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| &**p).collect();
    let mut rows: Vec<serde_json::Value> = stmt
        .query_map(params_refs.as_slice(), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map(|r| {
            r.filter_map(|x| x.ok())
                .map(|(id, domain, title, content, created_at)| {
                    let score = relevance_score(
                        &q.q,
                        title.as_deref().unwrap_or(""),
                        content.as_deref().unwrap_or(""),
                    );
                    json!({
                        "id": id,
                        "domain": domain,
                        "title": title,
                        "content": content,
                        "score": score,
                        "created_at": created_at,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Sort by score descending.
    rows.sort_by(|a, b| {
        let sa = a["score"].as_f64().unwrap_or(0.0);
        let sb = b["score"].as_f64().unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });

    let count = rows.len();
    Json(json!({"results": rows, "count": count, "query": q.q}))
}

#[derive(Debug, Deserialize)]
struct KbWriteBody {
    domain: String,
    title: String,
    content: String,
}

async fn handle_kb_write(
    State(state): State<Arc<KbState>>,
    Json(body): Json<KbWriteBody>,
) -> Json<serde_json::Value> {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(e) => return Json(json!({"error": e.to_string()})),
    };
    match conn.execute(
        "INSERT OR REPLACE INTO knowledge_base (domain, title, content, created_at) \
         VALUES (?1, ?2, ?3, datetime('now'))",
        params![body.domain, body.title, body.content],
    ) {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            Json(json!({"id": id, "status": "created", "domain": body.domain, "title": body.title}))
        }
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}
