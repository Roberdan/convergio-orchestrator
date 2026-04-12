//! Wave branch management — enforce one-branch-per-wave and smart commit strategy.
//!
//! Plan Zero T1-01 + T1-02: waves are the atomic unit for branching.
//! - Each wave gets ONE branch, shared by all tasks in the wave.
//! - Commit strategy determines PR vs direct-to-main.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Commit strategy for a wave: either direct to main or via PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitStrategy {
    /// Tests, docs, config — commit directly to main.
    DirectToMain,
    /// Features, fixes — require a PR with CI validation.
    ViaPr,
}

impl CommitStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DirectToMain => "direct_to_main",
            Self::ViaPr => "via_pr",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "direct_to_main" => Self::DirectToMain,
            _ => Self::ViaPr,
        }
    }
}

/// Generate the canonical branch name for a wave.
pub fn wave_branch_name(plan_id: i64, wave_id: &str) -> String {
    format!("wave/{plan_id}-{wave_id}")
}

/// Generate a single branch name for an entire plan (single_branch mode).
/// All waves share this branch — zero rebase between waves.
pub fn plan_branch_name(plan_id: i64) -> String {
    format!("plan/{plan_id}")
}

/// Resolve the branch name for a wave, respecting the plan's execution_mode.
/// If execution_mode = "single_branch": all waves use plan/{plan_id}.
/// Otherwise: each wave gets wave/{plan_id}-{wave_id}.
pub fn resolve_branch_name(conn: &Connection, wave_db_id: i64) -> rusqlite::Result<String> {
    let (plan_id, wave_id): (i64, String) = conn.query_row(
        "SELECT plan_id, wave_id FROM waves WHERE id = ?1",
        params![wave_db_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let mode: Option<String> = conn
        .query_row(
            "SELECT execution_mode FROM plans WHERE id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    if mode.as_deref() == Some("single_branch") {
        Ok(plan_branch_name(plan_id))
    } else {
        Ok(wave_branch_name(plan_id, &wave_id))
    }
}

/// Assign a branch to a wave. Idempotent — returns existing branch if already set.
/// In single_branch mode, all waves of the same plan share one branch.
pub fn assign_wave_branch(conn: &Connection, wave_db_id: i64) -> rusqlite::Result<String> {
    // Check if already assigned
    let existing: Option<String> = conn
        .query_row(
            "SELECT branch_name FROM waves WHERE id = ?1",
            params![wave_db_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    if let Some(ref branch) = existing {
        if !branch.is_empty() {
            return Ok(branch.clone());
        }
    }

    // Resolve branch name based on plan's execution_mode
    let branch = resolve_branch_name(conn, wave_db_id)?;
    conn.execute(
        "UPDATE waves SET branch_name = ?1 WHERE id = ?2",
        params![branch, wave_db_id],
    )?;

    Ok(branch)
}

/// Get the assigned branch for a wave (if any).
pub fn get_wave_branch(conn: &Connection, wave_db_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT branch_name FROM waves WHERE id = ?1",
        params![wave_db_id],
        |r| r.get(0),
    )
}

/// Determine commit strategy for a wave based on its tasks.
/// A wave is DirectToMain ONLY if ALL its tasks are test/doc/config.
pub fn determine_commit_strategy(
    conn: &Connection,
    wave_db_id: i64,
) -> rusqlite::Result<CommitStrategy> {
    let mut stmt = conn.prepare("SELECT title FROM tasks WHERE wave_id = ?1")?;
    let titles: Vec<String> = stmt
        .query_map(params![wave_db_id], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    if titles.is_empty() {
        return Ok(CommitStrategy::ViaPr);
    }

    let all_direct = titles.iter().all(|t| is_direct_to_main_task(t));
    Ok(if all_direct {
        CommitStrategy::DirectToMain
    } else {
        CommitStrategy::ViaPr
    })
}

/// Assign commit strategy for a wave and store it in DB.
pub fn assign_commit_strategy(
    conn: &Connection,
    wave_db_id: i64,
) -> rusqlite::Result<CommitStrategy> {
    let strategy = determine_commit_strategy(conn, wave_db_id)?;
    conn.execute(
        "UPDATE waves SET commit_strategy = ?1 WHERE id = ?2",
        params![strategy.as_str(), wave_db_id],
    )?;
    Ok(strategy)
}

/// Get stored commit strategy for a wave.
pub fn get_commit_strategy(conn: &Connection, wave_db_id: i64) -> rusqlite::Result<CommitStrategy> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT commit_strategy FROM waves WHERE id = ?1",
            params![wave_db_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    Ok(raw
        .map(|s| CommitStrategy::parse(&s))
        .unwrap_or(CommitStrategy::ViaPr))
}

use crate::gates::GateError;

/// WaveBranchGate: tasks in the same wave must use the same branch.
pub fn wave_branch_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let (wave_id, notes): (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT wave_id, notes FROM tasks WHERE id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| GateError {
            gate: "WaveBranchGate",
            reason: format!("task {task_id} not found"),
            expected: "task must exist in database".into(),
        })?;

    let Some(wave_id) = wave_id else {
        return Ok(());
    };

    let branch: Option<String> = conn
        .query_row(
            "SELECT branch_name FROM waves WHERE id = ?1",
            [wave_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    let Some(branch) = branch else {
        return Ok(());
    };
    if branch.is_empty() {
        return Ok(());
    }

    let notes = notes.unwrap_or_default();
    if notes.contains(&branch) || notes.contains("direct_to_main") {
        return Ok(());
    }

    Err(GateError {
        gate: "WaveBranchGate",
        reason: format!(
            "task {task_id} notes must reference wave branch '{branch}' — \
             all tasks in wave {wave_id} share one branch"
        ),
        expected: format!(
            "task notes must contain the wave branch name '{branch}' \
             (set via PR URL on that branch or 'direct_to_main' if applicable)"
        ),
    })
}

/// Check if a task's wave uses direct-to-main commit strategy.
pub fn is_direct_to_main(conn: &Connection, task_id: i64) -> bool {
    let strategy: Option<String> = conn
        .query_row(
            "SELECT w.commit_strategy FROM waves w \
             JOIN tasks t ON t.wave_id = w.id \
             WHERE t.id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    strategy.as_deref() == Some("direct_to_main")
}

/// Check if a task title suggests it can go directly to main (no PR needed).
fn is_direct_to_main_task(title: &str) -> bool {
    let lower = title.to_lowercase();
    let direct_patterns = [
        "test",
        "tests",
        "doc",
        "docs",
        "readme",
        "comment",
        "typo",
        "lint",
        "fmt",
        "format",
        "config",
        "baseline",
        "changelog",
    ];
    direct_patterns.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
#[path = "wave_branch_tests.rs"]
mod tests;
