// Migration v25: plan_reviews table, UNIQUE indexes for import safety,
// plans.source column for solve vs manual tracking.

use convergio_types::extension::Migration;

pub fn plan_reviews_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 25,
        description: "plan_reviews table, UNIQUE indexes, plans.source",
        up: "\
            CREATE TABLE IF NOT EXISTS plan_reviews (\
              id INTEGER PRIMARY KEY AUTOINCREMENT,\
              plan_id INTEGER NOT NULL,\
              reviewer_agent TEXT NOT NULL,\
              verdict TEXT NOT NULL,\
              suggestions TEXT,\
              created_at TEXT NOT NULL DEFAULT (datetime('now')),\
              FOREIGN KEY (plan_id) REFERENCES plans(id)\
            );\
            CREATE INDEX IF NOT EXISTS idx_plan_reviews_plan ON plan_reviews(plan_id);\
            CREATE UNIQUE INDEX IF NOT EXISTS idx_waves_plan_waveid \
              ON waves(plan_id, wave_id);\
            CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_plan_taskid \
              ON tasks(plan_id, task_id) WHERE task_id IS NOT NULL AND task_id != '';\
            ALTER TABLE plans ADD COLUMN source TEXT DEFAULT 'solve';\
        ",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_reviews_migration_applies() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        // Apply base schema first
        let mut all = crate::schema::migrations();
        all.sort_by_key(|m| m.version);
        convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
        // Apply this migration
        let applied = convergio_db::migration::apply_migrations(
            &conn,
            "orchestrator",
            &plan_reviews_migrations(),
        )
        .unwrap();
        assert!(applied > 0);
        // Verify table exists — insert with a real plan_id
        conn.execute(
            "INSERT INTO plans (project_id, name, status) VALUES ('test', 'test-plan', 'todo')",
            [],
        )
        .unwrap();
        let plan_id: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO plan_reviews (plan_id, reviewer_agent, verdict) VALUES (?1, 'test', 'proceed')",
            rusqlite::params![plan_id],
        )
        .unwrap();
        // Verify source column
        conn.execute("UPDATE plans SET source = 'manual' WHERE 1=0", [])
            .unwrap();
    }
}
