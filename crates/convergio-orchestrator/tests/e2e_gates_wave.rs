//! E2E tests for WaveSequenceGate and wave-related edge cases.

mod gates_common;
mod helpers;

use gates_common::{
    activate_plan, create_plan, create_task, create_wave, full_setup, set_task_status,
};
use rusqlite::params;

// ===========================================================================
// WaveSequenceGate
// ===========================================================================

#[tokio::test]
async fn test_wave2_task_blocked_when_wave1_incomplete() {
    let (_pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let _t1 = create_task(&state, plan_id, w1, "W1 task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&_pool, plan_id);

    // Try to start W2 task — should be blocked by WaveSequenceGate
    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("WaveSequenceGate"),
        "expected WaveSequenceGate error, got: {resp}"
    );
}

#[tokio::test]
async fn test_wave2_task_allowed_when_wave1_done() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "expected W2 task to start after W1 done, got: {resp}"
    );
}

#[tokio::test]
async fn test_wave2_task_allowed_when_wave1_submitted() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'submitted' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "submitted is terminal for wave gate, got: {resp}"
    );
}

// ===========================================================================
// Edge cases
// ===========================================================================

#[tokio::test]
async fn test_task_with_no_wave_bypasses_wave_gate() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    // Insert a task with no wave_id directly in DB
    let task_id: i64 = {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO tasks (task_id, plan_id, wave_id, title, status) \
             VALUES ('T-orphan', ?1, NULL, 'Orphan task', 'pending')",
            params![plan_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    };

    activate_plan(&pool, plan_id);
    let resp = set_task_status(&state, task_id, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "task without wave should bypass wave gate, got: {resp}"
    );
}

#[tokio::test]
async fn test_multiple_tasks_in_wave1_all_must_complete() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1a = create_task(&state, plan_id, w1, "W1 task A").await;
    let t1b = create_task(&state, plan_id, w1, "W1 task B").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&pool, plan_id);

    // Complete only one W1 task
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![t1a],
        )
        .unwrap();
    }

    // W2 task should still be blocked (t1b is pending)
    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp["error"].as_str().unwrap().contains("WaveSequenceGate"),
        "W2 should be blocked with one W1 task still pending, got: {resp}"
    );

    // Now complete the second W1 task
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE id = ?1",
            params![t1b],
        )
        .unwrap();
    }

    // W2 task should now pass
    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "W2 should be allowed after both W1 tasks done, got: {resp}"
    );
}

#[tokio::test]
async fn test_cancelled_wave1_tasks_dont_block_wave2() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 cancelled task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'cancelled' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "cancelled is terminal — should not block W2, got: {resp}"
    );
}

#[tokio::test]
async fn test_skipped_wave1_tasks_dont_block_wave2() {
    let (pool, state, _tmp) = full_setup();
    let plan_id = create_plan(&state).await;

    let w1 = create_wave(&state, plan_id, "W-1", "wave-1").await;
    let w2 = create_wave(&state, plan_id, "W-2", "wave-2").await;

    let t1 = create_task(&state, plan_id, w1, "W1 skipped task").await;
    let t2 = create_task(&state, plan_id, w2, "W2 task").await;
    activate_plan(&pool, plan_id);

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'skipped' WHERE id = ?1",
            params![t1],
        )
        .unwrap();
    }

    let resp = set_task_status(&state, t2, "in_progress", "agent-test").await;
    assert!(
        resp.get("error").is_none(),
        "skipped is terminal — should not block W2, got: {resp}"
    );
}
