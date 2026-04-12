use super::*;

fn setup_pool() -> ConnPool {
    let pool = convergio_db::pool::create_memory_pool().unwrap();
    let conn = pool.get().unwrap();
    convergio_db::migration::ensure_registry(&conn).unwrap();
    let mut all = crate::schema::migrations();
    all.extend(crate::schema_merge::merge_guardian_migrations());
    all.extend(crate::schema_wave_branch::wave_branch_migrations());
    all.sort_by_key(|m| m.version);
    convergio_db::migration::apply_migrations(&conn, "orchestrator", &all).unwrap();
    pool
}

#[test]
fn start_plan_sets_first_wave_in_progress() {
    let pool = setup_pool();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO plans (id, project_id, name, status) \
         VALUES (1, 'test', 'Alpha', 'todo')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO waves (id, wave_id, plan_id, name, status) \
         VALUES (10, 'W1', 1, 'First', 'pending')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO waves (id, wave_id, plan_id, name, status) \
         VALUES (11, 'W2', 1, 'Second', 'pending')",
        [],
    )
    .unwrap();

    start_plan(&conn, 1).unwrap();

    let plan_status: String = conn
        .query_row("SELECT status FROM plans WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(plan_status, "in_progress");

    let w1_status: String = conn
        .query_row("SELECT status FROM waves WHERE id = 10", [], |r| r.get(0))
        .unwrap();
    assert_eq!(w1_status, "in_progress");

    let w2_status: String = conn
        .query_row("SELECT status FROM waves WHERE id = 11", [], |r| r.get(0))
        .unwrap();
    assert_eq!(w2_status, "pending");
}

#[test]
fn check_completion_updates_tasks_done() {
    let pool = setup_pool();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO plans (id, project_id, name, status, tasks_done, tasks_total) \
         VALUES (2, 'test', 'Beta', 'in_progress', 0, 2)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO waves (id, wave_id, plan_id, name, status) \
         VALUES (20, 'W1', 2, 'Wave', 'in_progress')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, status) \
         VALUES (100, 'T1', 2, 20, 'Task 1', 'done')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, status) \
         VALUES (101, 'T2', 2, 20, 'Task 2', 'submitted')",
        [],
    )
    .unwrap();

    check_plan_completion(&conn).unwrap();

    let done: i64 = conn
        .query_row("SELECT tasks_done FROM plans WHERE id = 2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(done, 2);
}

#[test]
fn sequencer_tick_starts_unblocked_plan() {
    let pool = setup_pool();
    {
        let conn = pool.get().unwrap();
        // Plan A: done (dependency)
        conn.execute(
            "INSERT INTO plans (id, project_id, name, status) \
             VALUES (10, 'test', 'Plan A', 'done')",
            [],
        )
        .unwrap();
        // Plan B: todo, depends on Plan A
        conn.execute(
            "INSERT INTO plans (id, project_id, name, status, depends_on) \
             VALUES (11, 'test', 'Plan B', 'todo', '10')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO waves (id, wave_id, plan_id, name, status) \
             VALUES (30, 'W1', 11, 'First', 'pending')",
            [],
        )
        .unwrap();
    }
    sequencer_tick(&pool).unwrap();

    let conn = pool.get().unwrap();
    let status: String = conn
        .query_row("SELECT status FROM plans WHERE id = 11", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "in_progress");
}
