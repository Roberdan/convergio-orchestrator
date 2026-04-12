//! Migration for workflow solve sessions table.

use convergio_types::extension::Migration;

pub fn workflow_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 23,
        description: "workflow solve sessions for MCP enforcement",
        up: "CREATE TABLE IF NOT EXISTS solve_sessions (\
             id TEXT PRIMARY KEY, \
             project_id TEXT NOT NULL, \
             problem_description TEXT NOT NULL, \
             scale TEXT NOT NULL DEFAULT 'standard', \
             requirements_json TEXT, \
             acceptance_invariants_json TEXT, \
             plan_id INTEGER, \
             status TEXT NOT NULL DEFAULT 'active', \
             created_at TEXT NOT NULL DEFAULT (datetime('now'))\
         );\
         CREATE INDEX IF NOT EXISTS idx_solve_sessions_project \
             ON solve_sessions(project_id);\
         CREATE INDEX IF NOT EXISTS idx_solve_sessions_status \
             ON solve_sessions(status);",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_migration_version() {
        let m = workflow_migrations();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].version, 23);
    }

    #[test]
    fn workflow_migration_applies_cleanly() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        // Apply base schema first
        let base = crate::schema::migrations();
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &base).unwrap();
        // Apply workflow migration
        let applied = convergio_db::migration::apply_migrations(
            &conn,
            "orchestrator",
            &workflow_migrations(),
        )
        .unwrap();
        assert_eq!(applied, 1);
    }
}
