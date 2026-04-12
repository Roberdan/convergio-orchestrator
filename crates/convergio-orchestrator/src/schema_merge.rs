//! Migration v18 — MergeGuardian queue table.

use convergio_types::extension::Migration;

pub fn merge_guardian_migrations() -> Vec<Migration> {
    vec![Migration {
        version: 18,
        description: "merge guardian queue",
        up: "CREATE TABLE IF NOT EXISTS merge_queue (\
                id INTEGER PRIMARY KEY AUTOINCREMENT,\
                pr_number INTEGER NOT NULL UNIQUE,\
                branch TEXT NOT NULL,\
                files_json TEXT NOT NULL DEFAULT '[]',\
                decision TEXT NOT NULL DEFAULT 'pending',\
                overlaps_json TEXT,\
                status TEXT NOT NULL DEFAULT 'open',\
                created_at TEXT NOT NULL DEFAULT (datetime('now')),\
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))\
            );\
            CREATE INDEX IF NOT EXISTS idx_mq_status \
                ON merge_queue(status);\
            CREATE INDEX IF NOT EXISTS idx_mq_pr \
                ON merge_queue(pr_number);",
    }]
}
