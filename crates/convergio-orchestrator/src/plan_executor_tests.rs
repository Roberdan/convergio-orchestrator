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

fn seed_plan(pool: &ConnPool) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO plans (id, project_id, name, status) \
         VALUES (99, 'test', 'Test Plan', 'in_progress')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO waves (id, wave_id, plan_id, name, status) \
         VALUES (501, 'W1', 99, 'Wave 1', 'in_progress')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, description, status) \
         VALUES (901, 'T1', 99, 501, 'Test task', 'Do something', 'pending')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, status) \
         VALUES (902, 'T2', 99, 501, 'Another task', 'in_progress')",
        [],
    )
    .unwrap();
    // conn is dropped here, freeing the pool slot
}

#[test]
fn find_pending_tasks_returns_only_pending_in_active_waves() {
    let pool = setup_pool();
    seed_plan(&pool);
    let tasks = find_pending_tasks(&pool).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].db_id, 901);
    assert_eq!(tasks[0].title, "Test task");
}

#[test]
fn find_pending_tasks_includes_pre_assigned_executor() {
    // Tasks with executor_agent pre-assigned by the planner should still be
    // picked up by the plan executor (claiming is via status = 'pending').
    let pool = setup_pool();
    seed_plan(&pool);
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET executor_agent = 'baccio' WHERE id = 901",
            [],
        )
        .unwrap();
    }
    let tasks = find_pending_tasks(&pool).unwrap();
    assert_eq!(tasks.len(), 1, "pre-assigned tasks must still be found");
    assert_eq!(tasks[0].db_id, 901);
}

#[test]
fn find_pending_tasks_empty_on_no_active_plan() {
    let pool = setup_pool();
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO plans (id, project_id, name, status) \
             VALUES (100, 'test', 'Idle Plan', 'todo')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO waves (id, wave_id, plan_id, name, status) \
             VALUES (502, 'W1', 100, 'Wave', 'in_progress')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, status) \
             VALUES (903, 'T1', 100, 502, 'Task', 'pending')",
            [],
        )
        .unwrap();
    } // drop conn
    let tasks = find_pending_tasks(&pool).unwrap();
    assert!(tasks.is_empty());
}

#[test]
fn build_task_instructions_includes_context() {
    let pool = setup_pool();
    seed_plan(&pool);
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE waves SET branch_name = 'wave/99-W1' WHERE id = 501",
            [],
        )
        .unwrap();
    } // drop conn before task_instructions::build calls pool.get()
    let task = PendingTask {
        db_id: 901,
        task_id: "T1".into(),
        plan_id: 99,
        wave_db_id: 501,
        title: "Test task".into(),
        description: "Do something".into(),
        notes: String::new(),
        required_capabilities: None,
        executor_agent: None,
    };
    let instructions = task_instructions::build(&pool, &task).unwrap();
    assert!(instructions.contains("Test task"));
    assert!(instructions.contains("wave/99-W1"));
    assert!(instructions.contains("complete-flow"));
    assert!(instructions.contains("901"));
}
