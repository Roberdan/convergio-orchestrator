//! Task instruction builder + agent dispatch for the plan executor.
//!
//! Generates the full TASK.md content that spawned agents receive.
//! Uses task_context for isolated, budget-aware context extraction (#687).
//! Also resolves which agent should execute a task via role dispatcher.

use convergio_db::pool::ConnPool;
use rusqlite::params;
use serde_json::json;

use super::{PendingTask, DAEMON_BASE};

/// Build full task instructions for the agent.
/// Uses task_context::extract for minimal, isolated context (#687).
pub fn build(
    pool: &ConnPool,
    task: &PendingTask,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let ctx = crate::task_context::extract(pool, task)?;

    let mut out = format!(
        "# Task: {title}\n\n\
         Plan: {plan} (ID {plan_id})\n\
         Task DB ID: {db_id}\n\
         Task ID: {task_id}\n\
         Wave branch: {branch}\n\
         Commit strategy: {strategy}\n\
         Context budget: ~{tokens} tokens\n\n\
         ## Description\n\n\
         {description}\n\n",
        title = ctx.task_title,
        plan = ctx.plan_name,
        plan_id = task.plan_id,
        db_id = ctx.task_id,
        task_id = task.task_id,
        branch = ctx.wave_branch,
        strategy = ctx.commit_strategy,
        tokens = ctx.estimated_tokens,
        description = ctx.task_description,
    );

    // Inject task notes if present (extra context from planner or user)
    if !task.notes.is_empty() {
        out.push_str("## Notes\n\n");
        out.push_str(&task.notes);
        out.push_str("\n\n");
    }

    // Inject dependency outputs if any
    if !ctx.dependency_outputs.is_empty() {
        out.push_str("## Dependency Outputs\n\n");
        for dep in &ctx.dependency_outputs {
            out.push_str(&format!(
                "### {} ({})\n{}\n\n",
                dep.title, dep.task_id, dep.summary
            ));
        }
    }

    // Inject target files hint
    if !ctx.target_files.is_empty() {
        out.push_str("## Target Files\n\n");
        for f in &ctx.target_files {
            out.push_str(&format!("- `{f}`\n"));
        }
        out.push('\n');
    }

    // Inject relevant rules (not ALL rules — only task-type-specific)
    if !ctx.relevant_rules.is_empty() {
        out.push_str("## Rules\n\n");
        for r in &ctx.relevant_rules {
            out.push_str(&format!("- {r}\n"));
        }
        out.push('\n');
    }

    // Inject project context (org, members, KB) — existing logic
    let conn = pool.get()?;
    let project_id: String = conn
        .query_row(
            "SELECT project_id FROM plans WHERE id = ?1",
            params![task.plan_id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "unknown".into());
    let project_context = build_project_context(&conn, &project_id);
    out.push_str(&project_context);

    // Completion instructions
    out.push_str(&format!(
        "## Completion\n\n\
         When done, call the daemon API to submit:\n\
         ```bash\n\
         curl -X POST -H 'Authorization: Bearer '\"${{CONVERGIO_AUTH_TOKEN:?must be set}}\" \\\n\
           -H 'Content-Type: application/json' \\\n\
           http://localhost:8420/api/plan-db/task/complete-flow \\\n\
           -d '{{\"task_db_id\": {db_id}, \"agent_id\": \"$CONVERGIO_AGENT_ID\", \
         \"pr_url\": \"<PR_URL>\", \"test_command\": \"cargo test\", \
         \"test_output\": \"<output>\", \"test_exit_code\": 0}}'\n\
         ```\n",
        db_id = ctx.task_id,
    ));

    Ok(out)
}

fn build_project_context(conn: &rusqlite::Connection, project_id: &str) -> String {
    let mission: Option<String> = conn
        .query_row(
            "SELECT mission FROM ipc_orgs WHERE id = ?1",
            params![project_id],
            |r| r.get(0),
        )
        .ok();
    let members: Vec<String> = conn
        .prepare(
            "SELECT agent, role FROM ipc_org_members WHERE org_id = ?1 \
             ORDER BY joined_at DESC LIMIT 10",
        )
        .and_then(|mut stmt| {
            stmt.query_map(params![project_id], |r| {
                Ok(format!(
                    "- {} ({})",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?
                ))
            })?
            .collect()
        })
        .unwrap_or_default();
    let knowledge: Vec<(String, String)> = conn
        .prepare(
            "SELECT title, content FROM knowledge_base \
             WHERE domain IN (?1, ?2) ORDER BY title LIMIT 5",
        )
        .and_then(|mut stmt| {
            let prefixed = format!("org:{project_id}");
            stmt.query_map(params![project_id, prefixed], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect()
        })
        .unwrap_or_default();

    if mission.is_none() && members.is_empty() && knowledge.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Project Context\n\n");
    if let Some(mission) = mission {
        out.push_str(&format!("Mission: {mission}\n\n"));
    }
    for (title, content) in knowledge {
        out.push_str(&format!("### {title}\n{content}\n\n"));
    }
    if !members.is_empty() {
        out.push_str("### Active Team Members\n");
        out.push_str(&members.join("\n"));
        out.push_str("\n\n");
    }
    out
}

/// Enrich task instructions with semantically relevant KB entries (#702).
/// Queries the knowledge vector store via HTTP and prepends matching context.
/// Falls back silently to original instructions if unavailable.
pub async fn enrich_with_knowledge(instructions: &str, task_description: &str) -> String {
    let url = format!("{DAEMON_BASE}/api/knowledge/search");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(_) => return instructions.to_string(),
    };

    let query: String = task_description.chars().take(300).collect();
    let mut req = client.post(&url).json(&json!({
        "query": query,
        "limit": 3,
        "min_score": 0.4,
    }));
    if let Ok(t) = std::env::var("CONVERGIO_AUTH_TOKEN") {
        req = req.bearer_auth(t);
    }
    // No fallback token — fail closed if CONVERGIO_AUTH_TOKEN is not set

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "KB enrichment unavailable for task instructions");
            return instructions.to_string();
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return instructions.to_string(),
    };

    let results = match body.get("results").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return instructions.to_string(),
    };

    let mut section = String::from("## Relevant Knowledge\n\n");
    let mut total_chars = 0usize;
    const MAX_CHARS: usize = 2000; // ~500 tokens budget

    for r in results {
        let title = r
            .pointer("/entry/title")
            .and_then(|v| v.as_str())
            .unwrap_or("—");
        let content = r
            .pointer("/entry/content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let score = r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if score < 0.4 || content.is_empty() {
            continue;
        }
        if total_chars + content.len() > MAX_CHARS {
            break;
        }
        section.push_str(&format!("### {title}\n{content}\n\n"));
        total_chars += content.len();
    }

    if total_chars > 0 {
        tracing::info!(
            chars = total_chars,
            "injected KB context into task instructions"
        );
        format!("{instructions}\n{section}")
    } else {
        instructions.to_string()
    }
}

/// Resolve agent name via role dispatcher (semi-auto mode).
/// If task has required_capabilities, dispatch based on those.
/// Falls back to generic task-{id}-executor if dispatch fails.
pub async fn resolve_agent_name(task: &PendingTask, client: &reqwest::Client) -> String {
    let generic = format!("task-{}-executor", task.db_id);

    let caps: Vec<String> = task
        .required_capabilities
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    if caps.is_empty() && task.executor_agent.as_deref().unwrap_or("").is_empty() {
        return generic;
    }

    let dispatch_body = json!({
        "task_description": format!("{}: {}", task.title, task.description),
        "required_capabilities": caps,
    });

    let resp = client
        .post(format!("{DAEMON_BASE}/api/org/convergio-io/dispatch"))
        .header("Authorization", convergio_types::dev_auth_header())
        .json(&dispatch_body)
        .send()
        .await;

    match resp {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(body) if body.get("assigned_agent").is_some() => {
                let agent = body["assigned_agent"].as_str().unwrap_or(&generic);
                tracing::info!(
                    task_id = task.db_id,
                    assigned = agent,
                    "dispatcher assigned agent"
                );
                agent.to_string()
            }
            _ => {
                tracing::debug!("dispatcher no match for task {}", task.db_id);
                task.executor_agent.clone().unwrap_or(generic)
            }
        },
        Err(e) => {
            tracing::debug!("dispatcher unreachable for task {}: {e}", task.db_id);
            task.executor_agent.clone().unwrap_or(generic)
        }
    }
}
