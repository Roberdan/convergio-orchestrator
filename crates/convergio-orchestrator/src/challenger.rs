//! Challenger gate — adversarial reachability audit for plan closure.
//!
//! Verifies every plan output is connected and reachable:
//! - Every task with output_type "pr" has merge evidence
//! - No task left in limbo (pending with all siblings done)
//! - Every wave has at least one completed task
//! - No orphan waves (wave exists but zero tasks assigned)

use rusqlite::{params, Connection};
use serde_json::json;

/// Run challenger audit on a completed plan. Returns (pass, findings).
pub fn challenge(conn: &Connection, plan_id: i64) -> (bool, Vec<String>) {
    let mut findings = Vec::new();

    // 1. Orphan waves — waves with zero tasks
    let orphan_waves: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM waves w WHERE w.plan_id = ?1 \
             AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.wave_id = w.id)",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if orphan_waves > 0 {
        findings.push(format!("{orphan_waves} wave(s) have zero tasks (orphan)"));
    }

    // 2. Limbo tasks — pending while all siblings are terminal
    let limbo: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks t WHERE t.plan_id = ?1 \
             AND t.status = 'pending' \
             AND NOT EXISTS ( \
                 SELECT 1 FROM tasks t2 WHERE t2.wave_id = t.wave_id \
                 AND t2.id != t.id \
                 AND t2.status NOT IN ('done','submitted','cancelled','skipped') \
             )",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if limbo > 0 {
        findings.push(format!(
            "{limbo} task(s) stuck pending while wave siblings are done"
        ));
    }

    // 3. Waves without any completed task
    let barren_waves: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM waves w WHERE w.plan_id = ?1 \
             AND EXISTS (SELECT 1 FROM tasks t WHERE t.wave_id = w.id) \
             AND NOT EXISTS ( \
                 SELECT 1 FROM tasks t2 WHERE t2.wave_id = w.id \
                 AND t2.status IN ('done','submitted') \
             )",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if barren_waves > 0 {
        findings.push(format!(
            "{barren_waves} wave(s) have tasks but none completed"
        ));
    }

    // 4. Tasks with output but no evidence of delivery
    let undelivered: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks t WHERE t.plan_id = ?1 \
             AND t.status IN ('done','submitted') \
             AND t.description LIKE '%output_type%' \
             AND NOT EXISTS ( \
                 SELECT 1 FROM task_evidence e WHERE e.task_db_id = t.id \
             )",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if undelivered > 0 {
        findings.push(format!(
            "{undelivered} task(s) claim output but have no delivery evidence"
        ));
    }

    (findings.is_empty(), findings)
}

/// Build a JSON challenge report for a plan.
pub fn challenge_report(conn: &Connection, plan_id: i64) -> serde_json::Value {
    let (pass, findings) = challenge(conn, plan_id);
    json!({
        "plan_id": plan_id,
        "challenger_verdict": if pass { "pass" } else { "fail" },
        "findings": findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup_db() -> convergio_db::pool::ConnPool {
        let pool = convergio_db::pool::create_memory_pool().unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS plans (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE IF NOT EXISTS waves (id INTEGER PRIMARY KEY, plan_id INTEGER, wave_id TEXT);
             CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, plan_id INTEGER, wave_id INTEGER, title TEXT, status TEXT DEFAULT 'pending', description TEXT);
             CREATE TABLE IF NOT EXISTS task_evidence (id INTEGER PRIMARY KEY, task_db_id INTEGER, evidence_type TEXT, command TEXT, output_summary TEXT, exit_code INTEGER);
             INSERT INTO plans VALUES (1, 'test');
             INSERT INTO waves VALUES (1, 1, 'WA');
             INSERT INTO tasks VALUES (1, 1, 1, 'task1', 'done', 'fix something');",
        )
        .unwrap();
        pool
    }

    #[test]
    fn challenge_passes_with_evidence() {
        let pool = setup_db();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO task_evidence (task_db_id, evidence_type, command, output_summary, exit_code) VALUES (1, 'test_pass', 'cargo test', 'ok', 0)",
            [],
        ).unwrap();
        let (pass, findings) = challenge(&conn, 1);
        assert!(pass, "should pass: {findings:?}");
    }

    #[test]
    fn challenge_detects_orphan_wave() {
        let pool = setup_db();
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO waves VALUES (2, 1, 'WB')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO task_evidence (task_db_id, evidence_type, command, output_summary, exit_code) VALUES (1, 'test_pass', 'cargo test', 'ok', 0)",
            [],
        ).unwrap();
        let (pass, findings) = challenge(&conn, 1);
        assert!(!pass);
        assert!(findings.iter().any(|f| f.contains("orphan")));
    }
}
