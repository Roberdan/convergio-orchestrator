//! Migration v19 — wave branch management columns.
//!
//! Adds branch_name and commit_strategy to waves table
//! for one-branch-per-wave enforcement (Plan Zero T1-01/T1-02).

use convergio_types::extension::Migration;

pub fn wave_branch_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 19,
        description: "wave branch management: branch_name + commit_strategy",
        up: "\
            ALTER TABLE waves ADD COLUMN branch_name TEXT DEFAULT '';\
            ALTER TABLE waves ADD COLUMN commit_strategy TEXT DEFAULT 'via_pr';",
    }]
}

// Migration v22: add updated_at to waves + tasks for mesh sync
pub fn sync_fix_migrations() -> Vec<convergio_types::extension::Migration> {
    vec![convergio_types::extension::Migration {
        version: 22,
        description: "add updated_at to waves and tasks for mesh sync",
        up: "\
            ALTER TABLE waves ADD COLUMN updated_at TEXT DEFAULT '';\
            ALTER TABLE tasks ADD COLUMN updated_at TEXT DEFAULT '';\
            UPDATE waves SET updated_at = COALESCE(started_at, datetime('now'));\
            UPDATE tasks SET updated_at = COALESCE(completed_at, started_at, datetime('now'));\
            CREATE TABLE IF NOT EXISTS knowledge_base (\
                id INTEGER PRIMARY KEY, domain TEXT, title TEXT, \
                content TEXT, created_at TEXT, hit_count INTEGER DEFAULT 0, \
                updated_at TEXT DEFAULT '');\
            UPDATE knowledge_base SET updated_at = COALESCE(created_at, datetime('now')) \
                WHERE updated_at = '' OR updated_at IS NULL;",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_branch_migration_applies_cleanly() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        // Apply all prior migrations (consolidated v1 + merge guardian)
        let mut all = crate::schema::migrations();
        all.extend(crate::schema_merge::merge_guardian_migrations());
        all.sort_by_key(|m| m.version);
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
        // Apply wave branch migration
        let applied = convergio_db::migration::apply_migrations(
            &conn,
            "orchestrator",
            &wave_branch_migrations(),
        )
        .unwrap();
        assert!(applied > 0);
        // Verify columns exist
        conn.execute(
            "UPDATE waves SET branch_name = 'test', commit_strategy = 'via_pr' WHERE 1=0",
            [],
        )
        .unwrap();
    }
}
