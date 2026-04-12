//! Metrics collection for plan learning extraction.

use rusqlite::params;

#[derive(Debug)]
pub(super) struct PlanMetrics {
    pub tasks_total: i64,
    pub tasks_done: i64,
    pub tasks_failed: i64,
    pub tasks_cancelled: i64,
    pub waves_total: i64,
    pub cost_usd: f64,
    pub total_tokens: i64,
    pub duration_minutes: Option<f64>,
    pub agents_used: i64,
    pub respawn_count: i64,
    pub tasks_without_evidence: i64,
    pub pre_review_verdict: Option<String>,
    pub post_review_verdict: Option<String>,
}

pub(super) fn collect_metrics(
    conn: &rusqlite::Connection,
    plan_id: i64,
) -> Result<PlanMetrics, Box<dyn std::error::Error>> {
    let tasks_total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let tasks_done: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 AND status = 'done'",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let tasks_failed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 AND status = 'failed'",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let tasks_cancelled: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1 AND status = 'cancelled'",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let waves_total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM waves WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let cost_usd: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(cost_usd), 0) FROM token_usage WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0.0);

    let total_tokens: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(input_tokens + output_tokens), 0) \
             FROM token_usage WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let duration_minutes: Option<f64> = conn
        .query_row(
            "SELECT (julianday(completed_at) - julianday(started_at)) * 1440.0 \
             FROM plans WHERE id = ?1 AND started_at IS NOT NULL AND completed_at IS NOT NULL",
            params![plan_id],
            |r| r.get(0),
        )
        .ok();

    let agents_used: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT a.id) FROM art_agents a \
             JOIN tasks t ON a.task_id = t.id WHERE t.plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let respawn_count: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(a.respawn_count), 0) FROM art_agents a \
             JOIN tasks t ON a.task_id = t.id WHERE t.plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let tasks_without_evidence: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks t WHERE t.plan_id = ?1 \
             AND t.status IN ('done', 'submitted') \
             AND NOT EXISTS (SELECT 1 FROM task_evidence e WHERE e.task_db_id = t.id)",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let report_json: Option<String> = conn
        .query_row(
            "SELECT report_json FROM plan_metadata WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    let (pre_review_verdict, post_review_verdict) = super::parse_review_verdicts(&report_json);

    Ok(PlanMetrics {
        tasks_total,
        tasks_done,
        tasks_failed,
        tasks_cancelled,
        waves_total,
        cost_usd,
        total_tokens,
        duration_minutes,
        agents_used,
        respawn_count,
        tasks_without_evidence,
        pre_review_verdict,
        post_review_verdict,
    })
}
