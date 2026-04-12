//! Lifecycle gates — enforce plan/task/wave status transitions.
use convergio_db::pool::ConnPool;
use rusqlite::Connection;

/// Includes `expected` hint so agents know what format/action is needed.
#[derive(Debug)]
pub struct GateError {
    pub gate: &'static str,
    pub reason: String,
    /// Human-readable hint describing what the gate expects.
    pub expected: String,
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.gate, self.reason)
    }
}

pub fn import_gate(conn: &Connection, plan_id: i64) -> Result<(), GateError> {
    let status: String = conn
        .query_row("SELECT status FROM plans WHERE id = ?1", [plan_id], |r| {
            r.get(0)
        })
        .map_err(|_| GateError {
            gate: "ImportGate",
            reason: "plan not found".into(),
            expected: "plan must exist in database".into(),
        })?;
    match status.as_str() {
        "todo" | "draft" | "approved" => Ok(()),
        _ => Err(GateError {
            gate: "ImportGate",
            reason: format!("cannot add tasks to plan in status '{status}'"),
            expected: "plan status must be 'todo', 'draft', or 'approved'".into(),
        }),
    }
}

pub fn start_gate(conn: &Connection, plan_id: i64) -> Result<(), GateError> {
    let tasks: i64 = conn
        .query_row(
            "SELECT count(*) FROM tasks WHERE plan_id = ?1",
            [plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if tasks == 0 {
        return Err(GateError {
            gate: "StartGate",
            reason: "plan has zero tasks — cannot start".into(),
            expected: "import at least one task before starting the plan".into(),
        });
    }
    Ok(())
}

pub fn test_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    // task_evidence uses task_db_id and evidence_type columns
    let has_test: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_evidence \
             WHERE task_db_id = ?1 AND evidence_type = 'test_pass')",
            [task_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_test {
        return Err(GateError {
            gate: "TestGate",
            reason: format!("task {task_id} has no test_pass evidence"),
            expected: "record evidence with evidence_type='test_pass' via \
                       cvg_record_evidence or cvg_complete_task"
                .into(),
        });
    }
    Ok(())
}

pub fn evidence_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM task_evidence WHERE task_db_id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if count == 0 {
        return Err(GateError {
            gate: "EvidenceGate",
            reason: format!("task {task_id} has zero evidence records"),
            expected: "record at least one evidence entry via cvg_record_evidence \
                       (evidence_type: 'test_result' or 'test_pass')"
                .into(),
        });
    }
    Ok(())
}

pub fn validator_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let has_verdict: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM validation_verdicts v \
             JOIN validation_queue q ON q.id = v.queue_id \
             WHERE q.task_id = ?1 AND v.verdict = 'pass')",
            [task_id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_verdict {
        return Err(GateError {
            gate: "ValidatorGate",
            reason: format!("task {task_id} has no passing validation verdict"),
            expected: "run cvg_validate_plan to trigger Thor validation — \
                       all wave tasks must be 'submitted' first"
                .into(),
        });
    }
    Ok(())
}

pub fn pr_commit_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let notes: Option<String> = conn
        .query_row("SELECT notes FROM tasks WHERE id = ?1", [task_id], |r| {
            r.get(0)
        })
        .unwrap_or(None);
    let notes = notes.unwrap_or_default();
    if has_pr_or_commit(&notes) {
        return Ok(());
    }
    Err(GateError {
        gate: "PrCommitGate",
        reason: format!("task {task_id} notes missing PR URL or commit hash"),
        expected: "task notes must contain a GitHub PR URL \
                   (https://github.com/<owner>/<repo>/pull/<N>) or a \
                   7-40 char hex commit hash"
            .into(),
    })
}

fn has_pr_or_commit(text: &str) -> bool {
    if text.contains("github.com/") && text.contains("/pull/") {
        return true;
    }
    text.split_whitespace().any(|word| {
        let w = word.trim_matches(|c: char| !c.is_ascii_hexdigit());
        w.len() >= 7 && w.len() <= 40 && w.chars().all(|c| c.is_ascii_hexdigit())
    })
}

/// WaveSequenceGate: checks depends_on_wave first, falls back to sequential order.
pub fn wave_sequence_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let (wave_id, plan_id): (Option<i64>, i64) = conn
        .query_row(
            "SELECT wave_id, plan_id FROM tasks WHERE id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| GateError {
            gate: "WaveSequenceGate",
            reason: format!("task {task_id} not found"),
            expected: "task must exist in database".into(),
        })?;
    let Some(wave_id) = wave_id else {
        return Ok(());
    };
    // Check explicit depends_on_wave, fall back to sequential order
    let dep_wave: Option<String> = conn
        .query_row(
            "SELECT depends_on_wave FROM waves WHERE id = ?1",
            [wave_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let prev_wave = if let Some(ref dep) = dep_wave {
        if !dep.is_empty() {
            conn.query_row(
                "SELECT id FROM waves WHERE plan_id = ?1 AND wave_id = ?2",
                rusqlite::params![plan_id, dep],
                |r| r.get::<_, i64>(0),
            )
            .ok()
        } else {
            None
        }
    } else {
        None
    };
    let prev = prev_wave.or_else(|| {
        conn.query_row(
            "SELECT id FROM waves WHERE plan_id = ?1 AND id < ?2 \
             ORDER BY id DESC LIMIT 1",
            rusqlite::params![plan_id, wave_id],
            |r| r.get(0),
        )
        .ok()
    });
    let Some(prev) = prev else {
        return Ok(());
    };
    let pending: i64 = conn
        .query_row(
            "SELECT count(*) FROM tasks WHERE plan_id = ?1 AND wave_id = ?2 \
             AND status NOT IN ('done','submitted','cancelled','skipped')",
            rusqlite::params![plan_id, prev],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if pending > 0 {
        return Err(GateError {
            gate: "WaveSequenceGate",
            reason: format!(
                "wave {prev} has {pending} incomplete tasks — \
                 cannot start task {task_id} in wave {wave_id}"
            ),
            expected: format!(
                "all tasks in wave {prev} must reach 'done' or 'submitted' \
                 before starting tasks in wave {wave_id}"
            ),
        });
    }
    Ok(())
}

pub fn plan_status_gate(conn: &Connection, task_id: i64) -> Result<(), GateError> {
    let (plan_id, plan_status): (i64, String) = conn
        .query_row(
            "SELECT p.id, p.status FROM plans p \
             JOIN tasks t ON t.plan_id = p.id WHERE t.id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| GateError {
            gate: "PlanStatusGate",
            reason: format!("task {task_id} or its plan not found"),
            expected: "task and its parent plan must exist".into(),
        })?;
    if plan_status != "in_progress" {
        return Err(GateError {
            gate: "PlanStatusGate",
            reason: format!(
                "plan {plan_id} is '{plan_status}', not 'in_progress' — \
                 task transitions blocked"
            ),
            expected: "start the plan first via cvg_start_plan — \
                       plan status must be 'in_progress'"
                .into(),
        });
    }
    Ok(())
}

/// Run all applicable gates for a task status transition.
pub fn check_task_transition(
    pool: &ConnPool,
    task_id: i64,
    new_status: &str,
) -> Result<(), GateError> {
    let conn = pool.get().map_err(|e| GateError {
        gate: "db",
        reason: e.to_string(),
        expected: "database connection pool must be available".into(),
    })?;
    match new_status {
        "submitted" => {
            plan_status_gate(&conn, task_id)?;
            evidence_gate(&conn, task_id)?;
            test_gate(&conn, task_id)?;
            crate::spec_compliance_gate::spec_compliance_gate(&conn, task_id)?;
            if !crate::wave_branch::is_direct_to_main(&conn, task_id) {
                pr_commit_gate(&conn, task_id)?;
                crate::wave_branch::wave_branch_gate(&conn, task_id)?;
            }
            Ok(())
        }
        "in_progress" => {
            plan_status_gate(&conn, task_id)?;
            wave_sequence_gate(&conn, task_id)?;
            crate::file_conflict_gate::file_conflict_check(&conn, task_id)?;
            Ok(())
        }
        "done" => {
            validator_gate(&conn, task_id)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "gates_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "gates_transition_tests.rs"]
mod transition_tests;
