//! Migration v20 — advisory_locks table for file lock system.

use convergio_types::extension::Migration;

pub fn file_locks_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 20,
        description: "advisory file locks",
        up: "\
            CREATE TABLE IF NOT EXISTS advisory_locks (\
                id          INTEGER PRIMARY KEY AUTOINCREMENT,\
                file_path   TEXT NOT NULL UNIQUE,\
                agent_id    TEXT NOT NULL,\
                acquired_at TEXT NOT NULL DEFAULT (datetime('now')),\
                ttl_secs    INTEGER NOT NULL DEFAULT 3600\
            );\
            CREATE INDEX IF NOT EXISTS idx_al_agent \
                ON advisory_locks(agent_id);\
            CREATE INDEX IF NOT EXISTS idx_al_acquired \
                ON advisory_locks(acquired_at);",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_locks_migration_applies() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        let mut all = crate::schema::migrations();
        all.extend(crate::schema_merge::merge_guardian_migrations());
        all.extend(crate::schema_wave_branch::wave_branch_migrations());
        all.sort_by_key(|m| m.version);
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
        let applied = convergio_db::migration::apply_migrations(
            &conn,
            "orchestrator",
            &file_locks_migrations(),
        )
        .unwrap();
        assert!(applied > 0);
        // Verify table exists
        conn.execute(
            "INSERT INTO advisory_locks (file_path, agent_id, ttl_secs) \
             VALUES ('test.rs', 'a1', 60)",
            [],
        )
        .unwrap();
    }
}
