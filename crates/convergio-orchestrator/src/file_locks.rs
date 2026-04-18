//! Advisory file lock system for multi-agent coordination.
//!
//! - `POST /api/locks/acquire` — acquire an advisory lock
//! - `GET  /api/locks/active`  — list current locks
//! - `POST /api/locks/release` — release an advisory lock
//! - Auto-expire on TTL via background loop

use std::time::Duration;

use axum::extract::State;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use convergio_db::pool::ConnPool;

/// Build the file lock routes.
pub fn file_lock_routes(pool: ConnPool) -> Router {
    Router::new()
        .route("/api/locks/acquire", post(acquire_lock))
        .route("/api/locks/active", get(list_locks))
        .route("/api/locks/release", post(release_lock))
        .with_state(pool)
}

#[derive(Debug, Deserialize)]
struct AcquireBody {
    file_path: String,
    agent_id: String,
    ttl_secs: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ReleaseBody {
    file_path: String,
    agent_id: String,
}

async fn acquire_lock(State(pool): State<ConnPool>, Json(body): Json<AcquireBody>) -> Json<Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("lock acquire pool error: {e}");
            return Json(json!({"error": "internal server error"}));
        }
    };
    let ttl = body.ttl_secs.unwrap_or(3600);

    // Expire stale locks first
    expire_locks_sync(&conn);

    // Check if already locked by another agent
    let existing: Option<String> = conn
        .query_row(
            "SELECT agent_id FROM advisory_locks WHERE file_path = ?1",
            params![body.file_path],
            |r| r.get(0),
        )
        .ok();

    if let Some(holder) = existing {
        if holder != body.agent_id {
            return Json(json!({
                "ok": false,
                "error": "LOCK_HELD",
                "holder": holder,
            }));
        }
        // Refresh TTL for same agent
        let _ = conn.execute(
            "UPDATE advisory_locks SET ttl_secs = ?1, \
             acquired_at = datetime('now') \
             WHERE file_path = ?2 AND agent_id = ?3",
            params![ttl, body.file_path, body.agent_id],
        );
        return Json(json!({"ok": true, "refreshed": true}));
    }

    match conn.execute(
        "INSERT INTO advisory_locks (file_path, agent_id, ttl_secs) \
         VALUES (?1, ?2, ?3)",
        params![body.file_path, body.agent_id, ttl],
    ) {
        Ok(_) => Json(json!({"ok": true, "acquired": true})),
        Err(e) => {
            tracing::warn!("lock acquire failed: {e}");
            Json(json!({"error": "failed to acquire lock"}))
        }
    }
}

async fn list_locks(State(pool): State<ConnPool>) -> Json<Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("lock list pool error: {e}");
            return Json(json!({"error": "internal server error"}));
        }
    };

    expire_locks_sync(&conn);

    let mut stmt = match conn.prepare(
        "SELECT id, file_path, agent_id, acquired_at, ttl_secs \
         FROM advisory_locks ORDER BY acquired_at DESC",
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("lock list prepare error: {e}");
            return Json(json!({"error": "internal server error"}));
        }
    };

    let rows: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "file_path": row.get::<_, String>(1)?,
                "agent_id": row.get::<_, String>(2)?,
                "acquired_at": row.get::<_, String>(3)?,
                "ttl_secs": row.get::<_, i64>(4)?,
            }))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    Json(json!({"ok": true, "locks": rows}))
}

async fn release_lock(State(pool): State<ConnPool>, Json(body): Json<ReleaseBody>) -> Json<Value> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("lock release pool error: {e}");
            return Json(json!({"error": "internal server error"}));
        }
    };

    match conn.execute(
        "DELETE FROM advisory_locks \
         WHERE file_path = ?1 AND agent_id = ?2",
        params![body.file_path, body.agent_id],
    ) {
        Ok(n) if n > 0 => Json(json!({"ok": true, "released": true})),
        Ok(_) => Json(json!({"ok": false, "error": "NOT_FOUND"})),
        Err(e) => {
            tracing::warn!("lock release failed: {e}");
            Json(json!({"error": "failed to release lock"}))
        }
    }
}

/// Remove locks whose TTL has expired (synchronous helper).
fn expire_locks_sync(conn: &rusqlite::Connection) {
    let _ = conn.execute(
        "DELETE FROM advisory_locks \
         WHERE datetime(acquired_at, '+' || ttl_secs || ' seconds') \
               < datetime('now')",
        [],
    );
}

/// Spawn a background loop that expires locks every 60 seconds.
pub fn spawn_lock_expiry_loop(pool: ConnPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Ok(conn) = pool.get() {
                expire_locks_sync(&conn);
            }
        }
    })
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
        for m in crate::schema_merge::merge_guardian_migrations() {
            conn.execute_batch(m.up).unwrap();
        }
        for m in schema_file_locks_migrations() {
            conn.execute_batch(m.up).unwrap();
        }
        pool
    }

    fn schema_file_locks_migrations() -> Vec<convergio_types::extension::Migration> {
        crate::schema_file_locks::file_locks_migrations()
    }

    #[test]
    fn file_lock_routes_build() {
        let pool = setup_pool();
        let _router = file_lock_routes(pool);
    }

    #[test]
    fn expire_locks_removes_expired() {
        let pool = setup_pool();
        let conn = pool.get().unwrap();
        // Insert a lock that expired 10 seconds ago
        conn.execute(
            "INSERT INTO advisory_locks (file_path, agent_id, ttl_secs, acquired_at) \
             VALUES ('src/main.rs', 'agent-1', 1, datetime('now', '-10 seconds'))",
            [],
        )
        .unwrap();
        expire_locks_sync(&conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM advisory_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn expire_locks_keeps_valid() {
        let pool = setup_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO advisory_locks (file_path, agent_id, ttl_secs) \
             VALUES ('src/lib.rs', 'agent-2', 3600)",
            [],
        )
        .unwrap();
        expire_locks_sync(&conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM advisory_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
