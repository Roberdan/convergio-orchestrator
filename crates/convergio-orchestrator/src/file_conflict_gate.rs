//! FileConflictGate — warn (or block) when tasks claim overlapping files.

use rusqlite::Connection;

/// Check if a task's claimed_files overlap with other in-progress tasks.
/// Default: WARN only. Set CONVERGIO_FILE_GATE_MODE=block to hard-block.
pub fn file_conflict_check(conn: &Connection, task_id: i64) -> Result<(), super::gates::GateError> {
    let my_files = get_claimed_files(conn, task_id);
    if my_files.is_empty() {
        return Ok(());
    }
    let mut conflicts = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, title, claimed_files FROM tasks \
             WHERE status = 'in_progress' AND id != ?1 AND claimed_files != '[]'",
        )
        .map_err(|e| super::gates::GateError {
            gate: "FileConflictGate",
            reason: format!("query failed: {e}"),
            expected: "database accessible".into(),
        })?;
    let rows = stmt
        .query_map([task_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| super::gates::GateError {
            gate: "FileConflictGate",
            reason: format!("query failed: {e}"),
            expected: "database accessible".into(),
        })?;
    for row in rows.flatten() {
        let (other_id, other_title, other_files_json) = row;
        let other_files = parse_json_array(&other_files_json);
        let overlap: Vec<&str> = my_files
            .iter()
            .filter(|f| other_files.iter().any(|o| o == *f))
            .map(|s| s.as_str())
            .collect();
        if !overlap.is_empty() {
            conflicts.push(format!(
                "task {} ({}) on {:?}",
                other_id, other_title, overlap
            ));
        }
    }
    if conflicts.is_empty() {
        return Ok(());
    }
    let msg = format!("task {} overlaps with: {}", task_id, conflicts.join("; "));
    if std::env::var("CONVERGIO_FILE_GATE_MODE").as_deref() == Ok("block") {
        return Err(super::gates::GateError {
            gate: "FileConflictGate",
            reason: msg,
            expected: "no file overlap with in-progress tasks".into(),
        });
    }
    tracing::warn!("FileConflict (WARN): {msg}");
    Ok(())
}

fn get_claimed_files(conn: &Connection, task_id: i64) -> Vec<String> {
    conn.query_row(
        "SELECT claimed_files FROM tasks WHERE id = ?1",
        [task_id],
        |r| r.get::<_, String>(0),
    )
    .map(|s| parse_json_array(&s))
    .unwrap_or_default()
}

fn parse_json_array(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                id INTEGER PRIMARY KEY, title TEXT, status TEXT,
                claimed_files TEXT DEFAULT '[]', plan_id INTEGER DEFAULT 0,
                wave_id INTEGER DEFAULT 0
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn no_conflict_when_no_overlap() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO tasks (id,title,status,claimed_files) VALUES (1,'a','in_progress','[\"x.rs\"]')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (id,title,status,claimed_files) VALUES (2,'b','pending','[\"y.rs\"]')",
            [],
        ).unwrap();
        assert!(file_conflict_check(&conn, 2).is_ok());
    }

    #[test]
    fn warns_on_overlap() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO tasks (id,title,status,claimed_files) VALUES (1,'a','in_progress','[\"shared.rs\"]')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (id,title,status,claimed_files) VALUES (2,'b','pending','[\"shared.rs\"]')",
            [],
        ).unwrap();
        // WARN mode (default) — should pass
        assert!(file_conflict_check(&conn, 2).is_ok());
    }
}
