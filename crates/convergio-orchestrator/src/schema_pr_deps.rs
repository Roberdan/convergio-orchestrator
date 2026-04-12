//! Migration v21 — pr_dependencies table for PR dependency chains.

use convergio_types::extension::Migration;

pub fn pr_deps_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 21,
        description: "PR dependency chain",
        up: "\
            CREATE TABLE IF NOT EXISTS pr_dependencies (\
                id              INTEGER PRIMARY KEY AUTOINCREMENT,\
                pr_url          TEXT NOT NULL,\
                depends_on_url  TEXT NOT NULL,\
                satisfied       INTEGER NOT NULL DEFAULT 0,\
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),\
                UNIQUE(pr_url, depends_on_url)\
            );\
            CREATE INDEX IF NOT EXISTS idx_prd_pr \
                ON pr_dependencies(pr_url);\
            CREATE INDEX IF NOT EXISTS idx_prd_dep \
                ON pr_dependencies(depends_on_url);",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_deps_migration_applies() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        let mut all = crate::schema::migrations();
        all.extend(crate::schema_merge::merge_guardian_migrations());
        all.extend(crate::schema_wave_branch::wave_branch_migrations());
        all.extend(crate::schema_file_locks::file_locks_migrations());
        all.sort_by_key(|m| m.version);
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
        let applied =
            convergio_db::migration::apply_migrations(&conn, "orchestrator", &pr_deps_migrations())
                .unwrap();
        assert!(applied > 0);
    }
}
