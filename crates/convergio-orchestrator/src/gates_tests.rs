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
         INSERT INTO plans VALUES (1, 'todo');
         INSERT INTO plans VALUES (2, 'in_progress');
         INSERT INTO tasks (id, plan_id) VALUES (1, 1);",
    )
    .unwrap();
    pool
}

#[test]
fn import_gate_allows_todo() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    assert!(import_gate(&conn, 1).is_ok());
}

#[test]
fn import_gate_blocks_in_progress() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    assert!(import_gate(&conn, 2).is_err());
}

#[test]
fn start_gate_allows_with_tasks() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    assert!(start_gate(&conn, 1).is_ok());
}

#[test]
fn start_gate_blocks_without_tasks() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    assert!(start_gate(&conn, 2).is_err());
}

// --- PrCommitGate tests ---

#[test]
fn has_pr_or_commit_matches_pr_url() {
    assert!(has_pr_or_commit(
        "See https://github.com/Roberdan/convergio/pull/163"
    ));
}

#[test]
fn has_pr_or_commit_matches_commit_hash() {
    assert!(has_pr_or_commit("Fixed in 03d11a3"));
    assert!(has_pr_or_commit("abcdef1234567890abcdef1234567890abcdef12"));
}

#[test]
fn has_pr_or_commit_rejects_empty() {
    assert!(!has_pr_or_commit(""));
    assert!(!has_pr_or_commit("just some notes"));
    assert!(!has_pr_or_commit("abc12")); // too short
}

#[test]
fn pr_commit_gate_blocks_without_reference() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO tasks (id, plan_id, notes) VALUES (10, 1, 'no ref here')",
        [],
    )
    .unwrap();
    let err = pr_commit_gate(&conn, 10).unwrap_err();
    assert_eq!(err.gate, "PrCommitGate");
    assert!(err.expected.contains("github.com"));
}

#[test]
fn pr_commit_gate_passes_with_pr_url() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO tasks (id, plan_id, notes) VALUES (11, 1, \
         'https://github.com/Roberdan/convergio/pull/163')",
        [],
    )
    .unwrap();
    assert!(pr_commit_gate(&conn, 11).is_ok());
}

// --- WaveSequenceGate tests ---

#[test]
fn wave_sequence_allows_first_wave() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute_batch(
        "INSERT INTO waves VALUES (1, 1, 'in_progress');
         INSERT INTO tasks (id, plan_id, wave_id, status) \
             VALUES (20, 1, 1, 'pending');",
    )
    .unwrap();
    assert!(wave_sequence_gate(&conn, 20).is_ok());
}

#[test]
fn wave_sequence_blocks_when_predecessor_incomplete() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute_batch(
        "INSERT INTO waves VALUES (10, 1, 'in_progress');
         INSERT INTO waves VALUES (11, 1, 'pending');
         INSERT INTO tasks (id, plan_id, wave_id, status) \
             VALUES (30, 1, 10, 'pending');
         INSERT INTO tasks (id, plan_id, wave_id, status) \
             VALUES (31, 1, 11, 'pending');",
    )
    .unwrap();
    let err = wave_sequence_gate(&conn, 31).unwrap_err();
    assert_eq!(err.gate, "WaveSequenceGate");
    assert!(err.expected.contains("done"));
}

#[test]
fn wave_sequence_allows_when_predecessor_done() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute_batch(
        "INSERT INTO waves VALUES (10, 1, 'done');
         INSERT INTO waves VALUES (11, 1, 'pending');
         INSERT INTO tasks (id, plan_id, wave_id, status) \
             VALUES (30, 1, 10, 'done');
         INSERT INTO tasks (id, plan_id, wave_id, status) \
             VALUES (31, 1, 11, 'pending');",
    )
    .unwrap();
    assert!(wave_sequence_gate(&conn, 31).is_ok());
}

// --- EvidenceGate tests ---

#[test]
fn evidence_gate_blocks_without_evidence() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    let err = evidence_gate(&conn, 1).unwrap_err();
    assert_eq!(err.gate, "EvidenceGate");
    assert!(err.expected.contains("cvg_record_evidence"));
}

#[test]
fn evidence_gate_passes_with_evidence() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO task_evidence (task_db_id, evidence_type) VALUES (1, 'test_result')",
        [],
    )
    .unwrap();
    assert!(evidence_gate(&conn, 1).is_ok());
}

// --- TestGate tests ---

#[test]
fn test_gate_blocks_without_test_pass() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO task_evidence (task_db_id, evidence_type) VALUES (1, 'test_result')",
        [],
    )
    .unwrap();
    let err = test_gate(&conn, 1).unwrap_err();
    assert_eq!(err.gate, "TestGate");
    assert!(err.expected.contains("test_pass"));
}

#[test]
fn test_gate_passes_with_test_pass() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO task_evidence (task_db_id, evidence_type) VALUES (1, 'test_pass')",
        [],
    )
    .unwrap();
    assert!(test_gate(&conn, 1).is_ok());
}

// --- ValidatorGate tests ---

#[test]
fn validator_gate_blocks_without_verdict() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    let err = validator_gate(&conn, 1).unwrap_err();
    assert_eq!(err.gate, "ValidatorGate");
    assert!(err.expected.contains("cvg_validate_plan"));
}

#[test]
fn validator_gate_passes_with_pass_verdict() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO validation_queue (id, task_id) VALUES (1, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO validation_verdicts (id, queue_id, verdict) VALUES (1, 1, 'pass')",
        [],
    )
    .unwrap();
    assert!(validator_gate(&conn, 1).is_ok());
}

#[test]
fn validator_gate_blocks_with_fail_verdict() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO validation_queue (id, task_id) VALUES (1, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO validation_verdicts (id, queue_id, verdict) VALUES (1, 1, 'fail')",
        [],
    )
    .unwrap();
    assert!(validator_gate(&conn, 1).is_err());
}
