//! Task context extractor — builds minimal, isolated context per task (#687).
//!
//! Instead of giving spawned agents the full plan tree + all session context,
//! this module extracts ONLY what a task needs:
//! - Task description and metadata
//! - Target file paths (from description)
//! - Verify commands
//! - Outputs from dependency tasks
//! - Relevant rules (by task type, not all rules)
//!
//! Token budget: ~3-5K tokens per task instead of 50K+ shared context.

use convergio_db::pool::ConnPool;
use rusqlite::params;
use serde::Serialize;

use crate::plan_executor::PendingTask;

/// Extracted context for a single task — everything the agent needs, nothing more.
#[derive(Debug, Serialize)]
pub struct TaskContext {
    pub task_id: i64,
    pub task_title: String,
    pub task_description: String,
    pub plan_name: String,
    pub wave_branch: String,
    pub commit_strategy: String,
    pub target_files: Vec<String>,
    pub verify_commands: Vec<String>,
    pub dependency_outputs: Vec<DepOutput>,
    pub relevant_rules: Vec<String>,
    pub estimated_tokens: usize,
}

/// Output from a dependency task that this task may need.
#[derive(Debug, Serialize)]
pub struct DepOutput {
    pub task_id: String,
    pub title: String,
    pub summary: String,
}

/// Extract minimal context for a task. This replaces the full plan tree read.
pub(crate) fn extract(
    pool: &ConnPool,
    task: &PendingTask,
) -> Result<TaskContext, Box<dyn std::error::Error + Send + Sync>> {
    let conn = pool.get()?;

    let (branch, strategy): (String, String) = conn
        .query_row(
            "SELECT COALESCE(branch_name, ''), COALESCE(commit_strategy, 'via_pr') \
             FROM waves WHERE id = ?1",
            params![task.wave_db_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or_default();

    let plan_name: String = conn
        .query_row(
            "SELECT name FROM plans WHERE id = ?1",
            params![task.plan_id],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "unknown".into());

    let target_files = extract_file_paths(&task.description);
    let verify_commands = extract_verify_commands(&task.description);
    let dependency_outputs = fetch_dep_outputs(&conn, task.db_id);
    let relevant_rules = select_rules_for_task(&task.description, &target_files);

    let estimated_tokens = estimate_tokens(&task.description, &dependency_outputs, &relevant_rules);

    Ok(TaskContext {
        task_id: task.db_id,
        task_title: task.title.clone(),
        task_description: task.description.clone(),
        plan_name,
        wave_branch: branch,
        commit_strategy: strategy,
        target_files,
        verify_commands,
        dependency_outputs,
        relevant_rules,
        estimated_tokens,
    })
}

/// Extract file paths mentioned in the task description.
fn extract_file_paths(description: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for word in description.split_whitespace() {
        let clean = word.trim_matches(|c: char| c == '`' || c == '\'' || c == '"' || c == ',');
        if (clean.contains('/') || clean.contains('.'))
            && (clean.ends_with(".rs")
                || clean.ends_with(".ts")
                || clean.ends_with(".tsx")
                || clean.ends_with(".js")
                || clean.ends_with(".toml")
                || clean.ends_with(".yml")
                || clean.ends_with(".yaml")
                || clean.ends_with(".json")
                || clean.ends_with(".sql")
                || clean.ends_with(".md"))
            && !paths.contains(&clean.to_string())
        {
            paths.push(clean.to_string());
        }
    }
    paths
}

/// Extract verify commands from task description (lines starting with `cargo`, `pnpm`, etc.)
fn extract_verify_commands(description: &str) -> Vec<String> {
    let mut cmds = Vec::new();
    for line in description.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches("- ")
            .trim_start_matches("* ");
        if trimmed.starts_with("cargo ")
            || trimmed.starts_with("pnpm ")
            || trimmed.starts_with("npm ")
            || trimmed.starts_with("curl ")
        {
            cmds.push(trimmed.to_string());
        }
    }
    cmds
}

/// Fetch summary outputs from dependency tasks (tasks this one depends on).
fn fetch_dep_outputs(conn: &rusqlite::Connection, task_db_id: i64) -> Vec<DepOutput> {
    conn.prepare(
        "SELECT t.task_id, t.title, COALESCE(t.summary, t.notes, '') \
         FROM task_dependencies td \
         JOIN tasks t ON t.id = td.depends_on_task_id \
         WHERE td.task_id = ?1 AND t.status = 'submitted'",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![task_db_id], |r| {
            Ok(DepOutput {
                task_id: r.get::<_, String>(0)?,
                title: r.get(1)?,
                summary: r.get(2)?,
            })
        })?
        .collect()
    })
    .unwrap_or_default()
}

/// Select only the rules relevant to this task's domain.
/// Instead of loading ALL rules (7 files, ~3K tokens), pick by task content.
fn select_rules_for_task(description: &str, target_files: &[String]) -> Vec<String> {
    let desc_lower = description.to_lowercase();
    let is_rust = target_files
        .iter()
        .any(|f| f.ends_with(".rs") || f.ends_with(".toml"))
        || desc_lower.contains("cargo")
        || desc_lower.contains("rust")
        || desc_lower.contains("crate");
    let is_frontend = target_files
        .iter()
        .any(|f| f.ends_with(".ts") || f.ends_with(".tsx") || f.ends_with(".js"))
        || desc_lower.contains("pnpm")
        || desc_lower.contains("react")
        || desc_lower.contains("next");
    let is_test = desc_lower.contains("test") || desc_lower.contains("spec");

    let mut rules = vec![
        "Conventional commits: feat:, fix:, docs:, chore:, refactor:".to_string(),
        "Max 300 lines per file".to_string(),
        "Never merge without user approval — leave PR open".to_string(),
    ];
    if is_rust {
        rules.push("RUSTFLAGS=\"-Dwarnings\" cargo test --workspace before push".to_string());
        rules.push("cargo fmt --all -- --check".to_string());
        rules.push("axum 0.7: path params use :id syntax, not {id}".to_string());
    }
    if is_frontend {
        rules.push("Use Maranello DS exact .d.ts type signatures".to_string());
        rules.push("pnpm build && pnpm lint before push".to_string());
    }
    if is_test {
        rules.push(
            "Never assert_eq!(collection.len(), N) for system data — use assert!(len >= N)"
                .to_string(),
        );
        rules.push("Never hardcode versions — use env!(\"CARGO_PKG_VERSION\")".to_string());
        rules.push("Test helpers shared across binaries → #![allow(dead_code)]".to_string());
    }
    rules
}

/// Estimate token count for the context.
fn estimate_tokens(description: &str, dep_outputs: &[DepOutput], rules: &[String]) -> usize {
    let desc_tokens = description.len() / 4; // ~4 chars per token
    let dep_tokens: usize = dep_outputs.iter().map(|d| d.summary.len() / 4).sum();
    let rule_tokens: usize = rules.iter().map(|r| r.len() / 4).sum();
    desc_tokens + dep_tokens + rule_tokens + 200 // 200 for boilerplate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_file_paths() {
        let desc = "Modify `daemon/crates/convergio-knowledge/src/store.rs` and Cargo.toml";
        let paths = extract_file_paths(desc);
        assert!(!paths.is_empty());
        assert!(paths.iter().any(|p| p.ends_with("store.rs")));
    }

    #[test]
    fn extracts_verify_commands() {
        let desc = "Do this:\n- cargo test -p convergio-knowledge\n- cargo fmt --all";
        let cmds = extract_verify_commands(desc);
        assert_eq!(cmds.len(), 2);
    }

    #[test]
    fn selects_rust_rules_for_rust_task() {
        let rules = select_rules_for_task("add endpoint in crate", &["src/routes.rs".into()]);
        assert!(rules.iter().any(|r| r.contains("cargo fmt")));
        assert!(!rules.iter().any(|r| r.contains("pnpm")));
    }

    #[test]
    fn selects_frontend_rules_for_ts_task() {
        let rules = select_rules_for_task("fix button", &["src/Button.tsx".into()]);
        assert!(rules.iter().any(|r| r.contains("pnpm")));
        assert!(!rules.iter().any(|r| r.contains("cargo")));
    }

    #[test]
    fn selects_test_rules_when_testing() {
        let rules = select_rules_for_task("write unit tests for auth", &[]);
        assert!(rules.iter().any(|r| r.contains("assert_eq")));
    }

    #[test]
    fn token_estimate_is_reasonable() {
        let tokens = estimate_tokens("short task", &[], &["one rule".into()]);
        assert!(tokens < 500);
    }
}
