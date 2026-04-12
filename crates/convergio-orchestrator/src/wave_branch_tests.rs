use super::*;
use convergio_db::pool::ConnPool;

fn setup_db() -> ConnPool {
    let pool = convergio_db::pool::create_memory_pool().unwrap();
    let conn = pool.get().unwrap();
    convergio_db::migration::ensure_registry(&conn).unwrap();
    let mut all = crate::schema::migrations();
    all.extend(crate::schema_merge::merge_guardian_migrations());
    all.extend(crate::schema_wave_branch::wave_branch_migrations());
    all.sort_by_key(|m| m.version);
    convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
    conn.execute(
        "INSERT INTO plans (id, project_id, name) VALUES (30, 'convergio', 'Zero')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO waves (id, wave_id, plan_id, name) \
         VALUES (203, 'W1', 30, 'Core Rules')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, task_id, plan_id, wave_id, title) \
         VALUES (615, 'T1-01', 30, 203, 'Enforce one-branch-per-wave')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, task_id, plan_id, wave_id, title) \
         VALUES (616, 'T1-02', 30, 203, 'Add test for branch enforcement')",
        [],
    )
    .unwrap();
    pool
}

#[test]
fn branch_name_format() {
    assert_eq!(wave_branch_name(30, "W1"), "wave/30-W1");
    assert_eq!(wave_branch_name(42, "W3"), "wave/42-W3");
}

#[test]
fn strategy_roundtrip() {
    assert_eq!(
        CommitStrategy::parse("direct_to_main"),
        CommitStrategy::DirectToMain
    );
    assert_eq!(CommitStrategy::parse("via_pr"), CommitStrategy::ViaPr);
    assert_eq!(CommitStrategy::parse("anything"), CommitStrategy::ViaPr);
}

#[test]
fn direct_to_main_detection() {
    assert!(is_direct_to_main_task("Add unit tests for gates module"));
    assert!(is_direct_to_main_task("Update README with new API docs"));
    assert!(is_direct_to_main_task("Fix typo in error message"));
    assert!(is_direct_to_main_task("Update baseline config"));
    assert!(!is_direct_to_main_task("Implement spawn_real_agent()"));
    assert!(!is_direct_to_main_task("Add wave branch enforcement"));
}

#[test]
fn assign_and_get_wave_branch() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    let branch = assign_wave_branch(&conn, 203).unwrap();
    assert_eq!(branch, "wave/30-W1");
    let branch2 = assign_wave_branch(&conn, 203).unwrap();
    assert_eq!(branch2, "wave/30-W1");
    let got = get_wave_branch(&conn, 203).unwrap();
    assert_eq!(got, Some("wave/30-W1".to_string()));
}

#[test]
fn commit_strategy_all_tests() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    let strategy = determine_commit_strategy(&conn, 203).unwrap();
    assert_eq!(strategy, CommitStrategy::ViaPr);
}

#[test]
fn commit_strategy_assign_and_get() {
    let pool = setup_db();
    let conn = pool.get().unwrap();
    let strategy = assign_commit_strategy(&conn, 203).unwrap();
    assert_eq!(strategy, CommitStrategy::ViaPr);
    let got = get_commit_strategy(&conn, 203).unwrap();
    assert_eq!(got, CommitStrategy::ViaPr);
}
