// DB migrations for orchestrator tables.
//
// All tables consolidated into a single v1 migration.

use convergio_types::extension::Migration;
use rusqlite::Connection;

mod sql {
    pub const SCHEMA: &str = include_str!("schema.sql");
}

pub fn migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            description: "orchestrator tables",
            up: sql::SCHEMA,
        },
        Migration {
            version: 2,
            description: "tasks.required_capabilities for skill-based dispatch",
            up: "ALTER TABLE tasks ADD COLUMN required_capabilities TEXT;",
        },
        Migration {
            version: 3,
            description: "plans.planner_agent_id for planner/executor separation (#703)",
            up: "ALTER TABLE plans ADD COLUMN planner_agent_id TEXT DEFAULT '';",
        },
        Migration {
            version: 4,
            description: "tasks.claimed_files for workspace context and file conflict detection",
            up: "ALTER TABLE tasks ADD COLUMN claimed_files TEXT DEFAULT '[]';",
        },
    ]
}

/// Self-heal older SQLite DBs where the registry drifted and the column was not added.
pub fn ensure_required_capabilities_column(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|row| row.ok())
        .any(|name| name == "required_capabilities");
    if !has_column {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN required_capabilities TEXT",
            [],
        )?;
    }
    Ok(())
}

/// Self-heal: ensure claimed_files column exists (migration v4 may not have applied).
pub fn ensure_claimed_files_column(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|row| row.ok())
        .any(|name| name == "claimed_files");
    if !has_column {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN claimed_files TEXT DEFAULT '[]'",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_ordered() {
        let m = migrations();
        assert!(m.len() >= 4);
        for (i, mig) in m.iter().enumerate() {
            assert_eq!(mig.version, (i + 1) as u32);
        }
    }

    #[test]
    fn migrations_apply_cleanly() {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        convergio_db::migration::ensure_registry(&conn).unwrap();
        let applied =
            convergio_db::migration::apply_migrations(&conn, "orchestrator", &migrations())
                .unwrap();
        assert!(applied >= 4);
    }

    #[test]
    fn ensure_required_capabilities_self_heals() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT,
                plan_id INTEGER NOT NULL DEFAULT 0,
                wave_id INTEGER,
                title TEXT NOT NULL DEFAULT '',
                description TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                executor_agent TEXT
            );",
        )
        .unwrap();

        ensure_required_capabilities_column(&conn).unwrap();

        let mut stmt = conn.prepare("PRAGMA table_info(tasks)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();
        assert!(cols.iter().any(|c| c == "required_capabilities"));
    }
}
