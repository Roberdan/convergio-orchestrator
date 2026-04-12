//! Migration v24 — task locking for concurrent agent safety.
//!
//! Adds locked_by column to tasks table. When a task is in_progress,
//! locked_by records which agent claimed it. Other agents are rejected.

use convergio_types::extension::Migration;

pub fn task_lock_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 24,
        description: "task locking: locked_by column for concurrent agent safety",
        up: "ALTER TABLE tasks ADD COLUMN locked_by TEXT DEFAULT '';",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_lock_migration_applies() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        let mut all = crate::schema::migrations();
        all.extend(crate::schema_merge::merge_guardian_migrations());
        all.extend(crate::schema_wave_branch::wave_branch_migrations());
        all.extend(crate::schema_wave_branch::sync_fix_migrations());
        all.extend(crate::schema_wave_deps::wave_deps_migrations());
        all.sort_by_key(|m| m.version);
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
        let applied = convergio_db::migration::apply_migrations(
            &conn,
            "orchestrator",
            &task_lock_migrations(),
        )
        .unwrap();
        assert!(applied > 0);
        conn.execute("UPDATE tasks SET locked_by = 'test-agent' WHERE 1=0", [])
            .unwrap();
    }
}
