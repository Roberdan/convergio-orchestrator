//! Migration v23 — wave dependency support (depends_on_wave).
//!
//! Adds depends_on_wave column to waves table for explicit wave ordering.
//! When set, WaveSequenceGate checks this instead of sequential DB id order.

use convergio_types::extension::Migration;

pub fn wave_deps_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 23,
        description: "wave dependency: depends_on_wave column",
        up: "ALTER TABLE waves ADD COLUMN depends_on_wave TEXT DEFAULT '';",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_deps_migration_applies_cleanly() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        let mut all = crate::schema::migrations();
        all.extend(crate::schema_merge::merge_guardian_migrations());
        all.extend(crate::schema_wave_branch::wave_branch_migrations());
        all.extend(crate::schema_wave_branch::sync_fix_migrations());
        all.sort_by_key(|m| m.version);
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
        let applied = convergio_db::migration::apply_migrations(
            &conn,
            "orchestrator",
            &wave_deps_migrations(),
        )
        .unwrap();
        assert!(applied > 0);
        conn.execute("UPDATE waves SET depends_on_wave = 'W0' WHERE 1=0", [])
            .unwrap();
    }
}
