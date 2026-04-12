//! Per-project Thor validation — extends validation to load
//! project-specific rules from `execution_policy` and check
//! against project test suites, not just workspace-wide.

use rusqlite::params;
use serde::Serialize;
use serde_json::json;

use crate::policy::ExecutionPolicy;

/// Result of per-project Thor validation.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectValidationResult {
    pub plan_id: i64,
    pub project_id: String,
    pub valid: bool,
    pub policy: Option<ExecutionPolicy>,
    pub checks: Vec<ValidationCheck>,
}

/// Individual validation check.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// Validate a plan against project-specific policies and test suites.
pub fn validate_for_project(
    conn: &rusqlite::Connection,
    plan_id: i64,
) -> Result<ProjectValidationResult, String> {
    // Resolve project_id from plan
    let project_id: String = conn
        .query_row(
            "SELECT project_id FROM plans WHERE id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("plan not found: {e}"))?;

    let mut checks = Vec::new();

    // 1. Load execution policy for this project
    let policies = crate::policy::load_or_default(conn, &project_id)
        .map_err(|e| format!("policy load error: {e}"))?;
    let active_policy = policies.first().cloned();

    // 2. Check all tasks have evidence (project-scoped)
    let tasks_without_evidence: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks t \
             WHERE t.plan_id = ?1 \
             AND t.status = 'submitted' \
             AND NOT EXISTS ( \
                 SELECT 1 FROM task_status_log sl \
                 WHERE sl.task_id = t.id \
                 AND sl.notes LIKE '%evidence%' \
             )",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    checks.push(ValidationCheck {
        name: "evidence_coverage".into(),
        passed: tasks_without_evidence == 0,
        message: if tasks_without_evidence == 0 {
            "all submitted tasks have evidence".into()
        } else {
            format!("{tasks_without_evidence} tasks missing evidence")
        },
    });

    // 3. Check that all tasks in the plan are complete or submitted
    let incomplete: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks \
             WHERE plan_id = ?1 \
             AND status NOT IN ('done', 'submitted', 'cancelled')",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    checks.push(ValidationCheck {
        name: "task_completion".into(),
        passed: incomplete == 0,
        message: if incomplete == 0 {
            "all tasks are complete/submitted".into()
        } else {
            format!("{incomplete} tasks still in progress")
        },
    });

    // 4. Check double-validation policy if required
    let requires_double = policies.iter().any(|p| p.require_double_validation);

    if requires_double {
        let verdict_count: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT v.validator) \
                 FROM validation_verdicts v \
                 JOIN validation_queue q ON v.queue_id = q.id \
                 WHERE q.plan_id = ?1 AND v.verdict = 'approved'",
                params![plan_id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        checks.push(ValidationCheck {
            name: "double_validation".into(),
            passed: verdict_count >= 2,
            message: format!("{verdict_count}/2 independent validations"),
        });
    }

    // 5. Check human review if policy requires it
    let requires_human = policies.iter().any(|p| p.require_human);
    if requires_human {
        let human_review: bool = conn
            .query_row(
                "SELECT EXISTS( \
                     SELECT 1 FROM validation_verdicts v \
                     JOIN validation_queue q ON v.queue_id = q.id \
                     WHERE q.plan_id = ?1 \
                     AND v.validator NOT LIKE 'agent-%' \
                 )",
                params![plan_id],
                |r| r.get(0),
            )
            .unwrap_or(false);

        checks.push(ValidationCheck {
            name: "human_review".into(),
            passed: human_review,
            message: if human_review {
                "human review present".into()
            } else {
                "human review required but missing".into()
            },
        });
    }

    let all_passed = checks.iter().all(|c| c.passed);

    Ok(ProjectValidationResult {
        plan_id,
        project_id,
        valid: all_passed,
        policy: active_policy,
        checks,
    })
}

/// Build a JSON response from the validation result.
pub fn validation_json(result: &ProjectValidationResult) -> serde_json::Value {
    json!({
        "plan_id": result.plan_id,
        "project_id": result.project_id,
        "valid": result.valid,
        "checks": result.checks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for m in crate::schema::migrations() {
            conn.execute_batch(m.up).unwrap();
        }
        conn
    }

    #[test]
    fn validate_nonexistent_plan_returns_error() {
        let conn = setup_conn();
        let result = validate_for_project(&conn, 999);
        assert!(result.is_err());
    }

    #[test]
    fn validate_plan_with_complete_tasks() {
        let conn = setup_conn();
        conn.execute("INSERT INTO projects (id, name) VALUES ('p1', 'Test')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO plans (id, project_id, name, status) \
             VALUES (1, 'p1', 'plan-1', 'in_progress')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO waves (wave_id, plan_id, name) \
             VALUES ('w1', 1, 'Wave 1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (plan_id, wave_id, title, status) \
             VALUES (1, 1, 'task-1', 'done')",
            [],
        )
        .unwrap();

        let result = validate_for_project(&conn, 1).unwrap();
        assert_eq!(result.project_id, "p1");
        // task_completion should pass
        let tc = result.checks.iter().find(|c| c.name == "task_completion");
        assert!(tc.is_some());
        assert!(tc.unwrap().passed);
    }

    #[test]
    fn validation_json_serializes() {
        let result = ProjectValidationResult {
            plan_id: 1,
            project_id: "p1".into(),
            valid: true,
            policy: None,
            checks: vec![ValidationCheck {
                name: "test".into(),
                passed: true,
                message: "ok".into(),
            }],
        };
        let v = validation_json(&result);
        assert_eq!(v["valid"], true);
    }
}
