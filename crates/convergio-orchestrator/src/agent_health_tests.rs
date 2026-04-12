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
    // Create art_heartbeats table (from agent-runtime schema)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS art_heartbeats ( \
         agent_id TEXT PRIMARY KEY, last_seen TEXT, interval_s INTEGER);",
    )
    .unwrap();
    pool
}

#[test]
fn no_stuck_tasks_when_empty() {
    let pool = setup_pool();
    let conn = pool.get().unwrap();
    let stuck = find_stuck_tasks(&conn).unwrap();
    assert!(stuck.is_empty());
}

#[test]
fn detects_stuck_task() {
    let pool = setup_pool();
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO plans (id, project_id, name, status) \
             VALUES (1, 'test', 'P', 'in_progress')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO waves (id, wave_id, plan_id, name, status) \
             VALUES (1, 'W1', 1, 'W', 'in_progress')",
            [],
        )
        .unwrap();
        // Task started 2 hours ago (well past threshold)
        conn.execute(
            "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, status, \
             executor_agent, started_at) \
             VALUES (1, 'T1', 1, 1, 'Stuck task', 'in_progress', \
             'dead-agent', datetime('now', '-7200 seconds'))",
            [],
        )
        .unwrap();
    }
    let conn = pool.get().unwrap();
    let stuck = find_stuck_tasks(&conn).unwrap();
    assert_eq!(stuck.len(), 1);
    assert_eq!(stuck[0].0, 1);
}

#[test]
fn handle_stuck_reverts_to_pending() {
    let pool = setup_pool();
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO plans (id, project_id, name, status) \
             VALUES (2, 'test', 'P2', 'in_progress')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO waves (id, wave_id, plan_id, name, status) \
             VALUES (2, 'W1', 2, 'W', 'in_progress')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, task_id, plan_id, wave_id, title, status, \
             executor_agent, started_at) \
             VALUES (2, 'T1', 2, 2, 'Task', 'in_progress', \
             'agent-x', datetime('now'))",
            [],
        )
        .unwrap();
        handle_stuck_task(&conn, 2, "agent-x").unwrap();
        let status: String = conn
            .query_row("SELECT status FROM tasks WHERE id = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "pending");
    }
}
