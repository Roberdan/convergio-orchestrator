//! Integration tests for check_task_transition — full gate chain.

use super::*;
use convergio_db::pool::create_memory_pool;

fn setup_db() -> ConnPool {
    let pool = create_memory_pool().unwrap();
    let conn = pool.get().unwrap();
    conn.execute_batch(
        "CREATE TABLE plans (id INTEGER PRIMARY KEY, status TEXT);
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY, plan_id INTEGER,
             wave_id INTEGER, notes TEXT, status TEXT DEFAULT 'pending'
         );
         CREATE TABLE waves (
             id INTEGER PRIMARY KEY, plan_id INTEGER, status TEXT DEFAULT 'pending'
         );
         CREATE TABLE task_evidence (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             task_db_id INTEGER, evidence_type TEXT
         );
         CREATE TABLE validation_verdicts (
             id INTEGER PRIMARY KEY, queue_id INTEGER, verdict TEXT
         );
         CREATE TABLE validation_queue (
             id INTEGER PRIMARY KEY, task_id INTEGER
         );
         INSERT INTO plans VALUES (1, 'in_progress');
         INSERT INTO tasks (id, plan_id) VALUES (1, 1);",
    )
    .unwrap();
    pool
}

#[test]
fn transition_to_submitted_checks_all_gates() {
    let pool = setup_db();

    // No evidence → EvidenceGate blocks
    let result = check_task_transition(&pool, 1, "submitted");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().gate, "EvidenceGate");

    // Add evidence but no test_pass → TestGate blocks
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO task_evidence (task_db_id, evidence_type) VALUES (1, 'test_result')",
            [],
        )
        .unwrap();
    }
    let result = check_task_transition(&pool, 1, "submitted");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().gate, "TestGate");

    // Add test_pass but no PR → PrCommitGate blocks
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO task_evidence (task_db_id, evidence_type) VALUES (1, 'test_pass')",
            [],
        )
        .unwrap();
    }
    let result = check_task_transition(&pool, 1, "submitted");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().gate, "PrCommitGate");

    // Add PR URL in notes → passes all
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET notes = 'PR https://github.com/Roberdan/convergio/pull/177' WHERE id = 1",
            [],
        )
        .unwrap();
    }
    assert!(check_task_transition(&pool, 1, "submitted").is_ok());
}

#[test]
fn transition_to_done_requires_validator() {
    let pool = setup_db();

    // No verdict → blocks
    assert!(check_task_transition(&pool, 1, "done").is_err());

    // Add pass verdict → allows
    {
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO validation_queue (id, task_id) VALUES (1, 1);
             INSERT INTO validation_verdicts (id, queue_id, verdict) VALUES (1, 1, 'pass');",
        )
        .unwrap();
    }
    assert!(check_task_transition(&pool, 1, "done").is_ok());
}
