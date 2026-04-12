//! Concurrency gates — prevent duplicate work by parallel agents.
//!
//! TaskLockGate: blocks in_progress if another agent already holds the task.
//! WavePrDedupGate: blocks submit if another task in the wave has a different PR.

use rusqlite::Connection;

use crate::gates::GateError;

/// TaskLockGate: prevents two agents from claiming the same task.
/// When transitioning to in_progress, checks if locked_by is set to another agent.
pub fn task_lock_gate(
    conn: &Connection,
    task_id: i64,
    agent_id: Option<&str>,
) -> Result<(), GateError> {
    let locked_by: String = conn
        .query_row(
            "SELECT COALESCE(locked_by, '') FROM tasks WHERE id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .unwrap_or_default();

    if locked_by.is_empty() {
        return Ok(());
    }

    let requesting = agent_id.unwrap_or("");
    if requesting.is_empty() || locked_by == requesting {
        return Ok(());
    }

    Err(GateError {
        gate: "TaskLockGate",
        reason: format!(
            "task {task_id} is locked by agent '{locked_by}' — \
             agent '{requesting}' cannot claim it"
        ),
        expected: format!(
            "wait for agent '{locked_by}' to release the task \
             (submit or fail), or ask an admin to clear the lock"
        ),
    })
}

/// Set locked_by when a task moves to in_progress. Clear on submit/done/failed.
pub fn update_task_lock(conn: &Connection, task_id: i64, new_status: &str, agent_id: &str) {
    let lock_value = match new_status {
        "in_progress" => agent_id,
        _ => "", // clear lock on any terminal/non-progress state
    };
    let _ = conn.execute(
        "UPDATE tasks SET locked_by = ?1 WHERE id = ?2",
        rusqlite::params![lock_value, task_id],
    );
}

/// WavePrDedupGate: ensures all tasks in a wave use the same PR.
/// If another task in the wave already has a PR URL in notes, this task's
/// notes must reference the same PR (or be the first to set one).
pub fn wave_pr_dedup_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let (wave_id, notes): (Option<i64>, String) = conn
        .query_row(
            "SELECT wave_id, COALESCE(notes, '') FROM tasks WHERE id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| GateError {
            gate: "WavePrDedupGate",
            reason: format!("task {task_id} not found"),
            expected: "task must exist".into(),
        })?;

    let Some(wave_id) = wave_id else {
        return Ok(());
    };

    let my_pr = extract_pr_url(&notes);
    let Some(my_pr) = my_pr else {
        return Ok(());
    };

    // Find any other task in the same wave with a different PR URL
    let mut stmt = conn
        .prepare(
            "SELECT id, COALESCE(notes, '') FROM tasks \
             WHERE wave_id = ?1 AND id != ?2 AND notes IS NOT NULL",
        )
        .map_err(|e| GateError {
            gate: "WavePrDedupGate",
            reason: e.to_string(),
            expected: String::new(),
        })?;

    let conflicts: Vec<(i64, String)> = stmt
        .query_map(rusqlite::params![wave_id, task_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    for (other_id, other_notes) in &conflicts {
        if let Some(other_pr) = extract_pr_url(other_notes) {
            if other_pr != my_pr {
                return Err(GateError {
                    gate: "WavePrDedupGate",
                    reason: format!(
                        "task {task_id} PR '{my_pr}' conflicts with task {other_id} \
                         PR '{other_pr}' — wave requires one PR"
                    ),
                    expected: format!(
                        "all tasks in wave {wave_id} must use the same PR. \
                         Use '{other_pr}' or batch-complete via cvg_wave_complete"
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Extract GitHub PR URL from notes text.
fn extract_pr_url(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        if word.contains("github.com/") && word.contains("/pull/") {
            return Some(word.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_pr_url_works() {
        assert_eq!(
            extract_pr_url("PR https://github.com/org/repo/pull/42 done"),
            Some("https://github.com/org/repo/pull/42".into())
        );
        assert_eq!(extract_pr_url("no pr here"), None);
    }
}
