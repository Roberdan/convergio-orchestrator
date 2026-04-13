//! Executor routing — spawn agents based on executor_agent field.
//!
//! Routes: "copilot"/empty → local, "delegate:<peer>" → mesh, "manual" → skip.

use convergio_db::pool::ConnPool;
use rusqlite::params;
use serde_json::json;

use super::{PendingTask, DAEMON_BASE, MAX_SPAWN_FAILURES};

/// T2-01: Spawn a real agent process for a task.
/// Routes based on executor_agent field:
///   - "copilot" or empty → local spawn via /api/agents/spawn (default)
///   - "delegate:<peer_id>" → mesh delegation via /api/delegate/spawn
///   - "manual" → mark awaiting_manual, skip spawn (needs human/specific agent)
pub(super) async fn spawn_real_agent(
    pool: &ConnPool,
    task: &PendingTask,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check if task should be routed to mesh delegation or manual
    let executor = task.executor_agent.as_deref().unwrap_or("copilot");
    if executor == "manual" || executor == "human" {
        tracing::info!(
            "plan_executor: task {} marked as manual — skipping auto-spawn",
            task.db_id
        );
        if let Ok(conn) = pool.get() {
            let _ = conn.execute(
                "UPDATE tasks SET status = 'awaiting_manual', \
                 notes = COALESCE(notes, '') || ' [auto-routing: manual]' WHERE id = ?1",
                params![task.db_id],
            );
        }
        return Ok(());
    }

    // Mark task as in_progress (claim it)
    {
        let conn = pool.get()?;
        let claimed = conn.execute(
            "UPDATE tasks SET status = 'in_progress', \
             started_at = datetime('now'), \
             executor_agent = 'plan-executor' \
             WHERE id = ?1 AND status = 'pending'",
            params![task.db_id],
        )?;
        if claimed == 0 {
            tracing::debug!(
                "plan_executor: task {} already claimed, skipping",
                task.db_id
            );
            return Ok(());
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Planner/executor separation enforcement (#687, was #703 warning-only)
    if let Ok(conn) = pool.get() {
        let planner: String = conn
            .query_row(
                "SELECT COALESCE(planner_agent_id, '') FROM plans WHERE id = ?1",
                params![task.plan_id],
                |r| r.get(0),
            )
            .unwrap_or_default();
        if !planner.is_empty() {
            let executor_name = task.executor_agent.as_deref().unwrap_or("copilot");
            if executor_name == planner {
                tracing::warn!(
                    plan_id = task.plan_id,
                    agent = planner.as_str(),
                    "planner/executor overlap blocked: spawning fresh executor instead"
                );
                // Don't use the planner as executor — force a fresh agent name
                // This ensures context isolation: the executor won't carry planning context
            }
        }
    }

    // Route to mesh delegation if executor_agent starts with "delegate:"
    if let Some(peer_id) = executor.strip_prefix("delegate:") {
        return spawn_mesh_delegate(pool, task, &client, peer_id).await;
    }

    // Default: local spawn via /api/agents/spawn
    spawn_local_agent(pool, task, &client).await
}

/// Spawn agent locally via /api/agents/spawn.
async fn spawn_local_agent(
    pool: &ConnPool,
    task: &PendingTask,
    client: &reqwest::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let agent_name = super::task_instructions::resolve_agent_name(task, client).await;
    let instructions = super::task_instructions::build(pool, task)?;
    let description = format!("{}: {}", task.title, task.description);
    let instructions =
        super::task_instructions::enrich_with_knowledge(&instructions, &description).await;

    let mut spawn_body = json!({
        "agent_name": agent_name,
        "org_id": "convergio-io",
        "task_id": task.db_id,
        "instructions": instructions,
        "tier": "t1",
        "budget_usd": 10,
        "timeout_secs": 3600,
    });
    // Pass repo_override so the spawner creates the worktree in the right repo
    if let Some(ref repo) = task.repo_path {
        spawn_body["repo_override"] = json!(repo);
    }

    let resp = client
        .post(format!("{DAEMON_BASE}/api/agents/spawn"))
        .header("Authorization", convergio_types::dev_auth_header())
        .json(&spawn_body)
        .send()
        .await?;

    handle_spawn_response(pool, task, resp).await
}

/// Delegate task to a mesh peer via /api/delegate/spawn.
async fn spawn_mesh_delegate(
    pool: &ConnPool,
    task: &PendingTask,
    client: &reqwest::Client,
    peer_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let instructions = super::task_instructions::build(pool, task)?;
    let description = format!("{}: {}", task.title, task.description);
    let instructions =
        super::task_instructions::enrich_with_knowledge(&instructions, &description).await;
    tracing::info!(
        "plan_executor: delegating task {} to mesh peer '{peer_id}'",
        task.db_id
    );

    let resp = client
        .post(format!("{DAEMON_BASE}/api/delegate/spawn"))
        .header("Authorization", convergio_types::dev_auth_header())
        .json(&json!({
            "agent_name": format!("task-{}-delegate", task.db_id),
            "instructions": instructions,
            "peer_id": peer_id,
        }))
        .send()
        .await?;

    handle_spawn_response(pool, task, resp).await
}

/// Process spawn response: handle success/failure, track spawn_failures.
async fn handle_spawn_response(
    pool: &ConnPool,
    task: &PendingTask,
    resp: reqwest::Response,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let body: serde_json::Value = resp.json().await?;

    if body.get("error").is_some() {
        let err = body["error"].as_str().unwrap_or("unknown");
        tracing::error!("plan_executor: spawn failed for task {}: {err}", task.db_id);
        if let Ok(conn) = pool.get() {
            let failures: i64 = conn
                .query_row(
                    "SELECT COALESCE(spawn_failures, 0) FROM tasks WHERE id = ?1",
                    params![task.db_id],
                    |r| r.get(0),
                )
                .unwrap_or(0)
                + 1;
            if failures >= MAX_SPAWN_FAILURES {
                tracing::error!(
                    "plan_executor: task {} failed after {failures} spawn attempts",
                    task.db_id
                );
                let _ = conn.execute(
                    "UPDATE tasks SET status = 'failed', \
                     spawn_failures = ?2, executor_agent = NULL WHERE id = ?1",
                    params![task.db_id, failures],
                );
            } else {
                let _ = conn.execute(
                    "UPDATE tasks SET status = 'pending', \
                     spawn_failures = ?2, executor_agent = NULL WHERE id = ?1",
                    params![task.db_id, failures],
                );
            }
        }
        return Err(err.into());
    }

    let agent_id = body["agent_id"]
        .as_str()
        .or_else(|| body["delegation_id"].as_str())
        .unwrap_or("unknown");
    tracing::info!(
        "plan_executor: spawned agent {agent_id} for task {} ({})",
        task.db_id,
        task.title
    );

    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "UPDATE tasks SET executor_agent = ?1 WHERE id = ?2",
            params![agent_id, task.db_id],
        );
    }

    Ok(())
}
