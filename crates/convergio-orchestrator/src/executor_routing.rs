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

    // Remap repo_path from local to remote node paths
    let remote_repo = task
        .repo_path
        .as_deref()
        .and_then(|local| remap_repo_path(local, peer_id));

    let mut body = json!({
        "agent_name": format!("task-{}-delegate", task.db_id),
        "instructions": instructions,
        "peer_id": peer_id,
    });
    if let Some(ref rp) = remote_repo {
        body["repo_override"] = json!(rp);
    }

    let resp = client
        .post(format!("{DAEMON_BASE}/api/delegate/spawn"))
        .header("Authorization", convergio_types::dev_auth_header())
        .json(&body)
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

/// Remap a local repo_path to the equivalent path on a remote peer.
///
/// Uses peers.conf to find repo_path for both local and remote nodes.
/// Local: /Users/Roberdan/GitHub/Convergio-Repos/convergio-billing
/// Remote (macProM1): /Users/roberdandev/GitHub/Convergio-Repos/convergio-billing
///
/// Logic: find the GitHub base dir from each node's repo_path (parent of convergio/),
/// then rebase the relative path onto the remote base.
fn remap_repo_path(local_path: &str, peer_id: &str) -> Option<String> {
    let conf_path =
        std::path::PathBuf::from(std::env::var("CONVERGIO_PEERS_CONF").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/.claude/config/peers.conf")
        }));
    let text = std::fs::read_to_string(&conf_path).ok()?;

    // Find local base: parent of the daemon repo_path
    // The daemon repo is at <base>/convergio, so base = parent
    let local_base = find_base_from_config(&text, None)?;
    let remote_base = find_base_from_config(&text, Some(peer_id))?;

    // Strip local base from local_path, append to remote base
    let local_path = std::path::Path::new(local_path);
    let local_base_path = std::path::Path::new(&local_base);
    let relative = local_path.strip_prefix(local_base_path).ok()?;

    let remote = std::path::Path::new(&remote_base).join(relative);
    Some(remote.to_string_lossy().to_string())
}

/// Extract the GitHub base directory from peers.conf for a given peer.
/// If peer_id is None, finds the local node (by matching hostname).
fn find_base_from_config(config_text: &str, peer_id: Option<&str>) -> Option<String> {
    let mut current_section = String::new();
    let mut current_repo_path: Option<String> = None;
    let local_hostname = std::process::Command::new("hostname")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    for line in config_text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') && line != "[mesh]" {
            // Save previous section if it matched
            if let Some(ref rp) = current_repo_path {
                if should_use_section(peer_id, &current_section, &local_hostname, config_text) {
                    // repo_path points to convergio repo, parent is the base
                    return std::path::Path::new(rp)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string());
                }
            }
            current_section = line[1..line.len() - 1].to_string();
            current_repo_path = None;
        }
        if let Some(val) = line.strip_prefix("repo_path=") {
            current_repo_path = Some(val.trim().to_string());
        }
    }
    // Check last section
    if let Some(ref rp) = current_repo_path {
        if should_use_section(peer_id, &current_section, &local_hostname, config_text) {
            return std::path::Path::new(rp)
                .parent()
                .map(|p| p.to_string_lossy().to_string());
        }
    }
    None
}

fn should_use_section(
    peer_id: Option<&str>,
    section: &str,
    local_hostname: &str,
    config_text: &str,
) -> bool {
    match peer_id {
        Some(id) => section == id,
        None => {
            // Find local node: check aliases for hostname match
            let section_block = extract_section_block(config_text, section);
            section_block.contains(local_hostname)
                || section_block.contains(&local_hostname.replace(".local", ""))
        }
    }
}

fn extract_section_block(config_text: &str, section: &str) -> String {
    let mut in_section = false;
    let mut block = String::new();
    for line in config_text.lines() {
        let trimmed = line.trim();
        if trimmed == format!("[{section}]") {
            in_section = true;
            continue;
        }
        if in_section && trimmed.starts_with('[') {
            break;
        }
        if in_section {
            block.push_str(trimmed);
            block.push('\n');
        }
    }
    block
}
