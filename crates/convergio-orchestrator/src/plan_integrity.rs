//! Integrity checks that must hold before a plan is marked `done`.
//!
//! WHY: without these, plans were auto-promoted to `done` even when the
//! majority of their tasks were still pending/failed (observed: plan 2444
//! marked done with 5/28 tasks completed). Closing a plan early breaks
//! downstream accounting — tasks_done counters, billing, reports — and
//! hides real execution failures from operators.

use rusqlite::{params, Connection};

/// Tasks in any of these states are considered "safely terminal" for the
/// purpose of closing a plan. `failed` is deliberately excluded so that
/// a plan cannot auto-close while real failures are still on the books.
pub const TERMINAL_TASK_STATES: &[&str] = &["done", "submitted", "cancelled", "skipped"];

/// Returns true when every task on `plan_id` is in a terminal state and
/// the plan has at least one task.
pub fn plan_ready_to_close(conn: &Connection, plan_id: i64) -> Result<bool, rusqlite::Error> {
    let (total, open): (i64, i64) = conn.query_row(
        "SELECT \
           COUNT(*), \
           SUM(CASE WHEN status NOT IN ('done','submitted','cancelled','skipped') THEN 1 ELSE 0 END) \
         FROM tasks WHERE plan_id = ?1",
        params![plan_id],
        |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0))),
    )?;
    Ok(total > 0 && open == 0)
}

/// Describes why a plan cannot be closed, for user-facing errors.
pub fn describe_close_blockers(
    conn: &Connection,
    plan_id: i64,
) -> Result<Option<String>, rusqlite::Error> {
    let (total, open, failed): (i64, i64, i64) = conn.query_row(
        "SELECT \
           COUNT(*), \
           SUM(CASE WHEN status NOT IN ('done','submitted','cancelled','skipped') THEN 1 ELSE 0 END), \
           SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) \
         FROM tasks WHERE plan_id = ?1",
        params![plan_id],
        |r| {
            Ok((
                r.get(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        },
    )?;
    if total == 0 {
        return Ok(Some("plan has no tasks".into()));
    }
    if open == 0 {
        return Ok(None);
    }
    Ok(Some(format!(
        "{open} task(s) not terminal ({failed} failed); required states: {:?}",
        TERMINAL_TASK_STATES
    )))
}

/// SQL fragment to splice into an UPDATE on `plans` so the UPDATE only
/// matches rows whose tasks are all terminal. Use from `wave_advance`
/// where we auto-promote a plan to `done`.
pub const PLAN_TASKS_ALL_TERMINAL_SQL: &str = "\
    NOT EXISTS ( \
      SELECT 1 FROM tasks \
      WHERE tasks.plan_id = plans.id \
      AND tasks.status NOT IN ('done','submitted','cancelled','skipped') \
    ) AND EXISTS ( \
      SELECT 1 FROM tasks WHERE tasks.plan_id = plans.id \
    )";

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks(id INTEGER PRIMARY KEY, plan_id INTEGER, status TEXT);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn empty_plan_not_ready() {
        let conn = setup();
        assert!(!plan_ready_to_close(&conn, 1).unwrap());
        assert_eq!(
            describe_close_blockers(&conn, 1).unwrap().as_deref(),
            Some("plan has no tasks")
        );
    }

    #[test]
    fn all_done_ready() {
        let conn = setup();
        conn.execute(
            "INSERT INTO tasks(plan_id, status) VALUES (1,'done'),(1,'submitted')",
            [],
        )
        .unwrap();
        assert!(plan_ready_to_close(&conn, 1).unwrap());
        assert!(describe_close_blockers(&conn, 1).unwrap().is_none());
    }

    #[test]
    fn failed_blocks_close() {
        let conn = setup();
        conn.execute(
            "INSERT INTO tasks(plan_id, status) VALUES (1,'done'),(1,'failed')",
            [],
        )
        .unwrap();
        assert!(!plan_ready_to_close(&conn, 1).unwrap());
        let msg = describe_close_blockers(&conn, 1).unwrap().unwrap();
        assert!(msg.contains("1 task(s) not terminal"));
        assert!(msg.contains("1 failed"));
    }

    #[test]
    fn pending_blocks_close() {
        let conn = setup();
        conn.execute(
            "INSERT INTO tasks(plan_id, status) VALUES (1,'done'),(1,'pending')",
            [],
        )
        .unwrap();
        assert!(!plan_ready_to_close(&conn, 1).unwrap());
    }

    #[test]
    fn cancelled_and_skipped_count_as_terminal() {
        let conn = setup();
        conn.execute(
            "INSERT INTO tasks(plan_id, status) VALUES (1,'done'),(1,'cancelled'),(1,'skipped')",
            [],
        )
        .unwrap();
        assert!(plan_ready_to_close(&conn, 1).unwrap());
    }
}
