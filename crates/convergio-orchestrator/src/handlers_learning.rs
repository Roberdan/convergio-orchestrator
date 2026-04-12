//! Automatic learning extraction — generates key_learnings_json when plan completes.
//!
//! Called from on_plan_done(). Runs as a background tokio task.
//! Collects: task metrics, cost data, respawn counts, evidence gaps, Thor reviews.

#[path = "handlers_learning_metrics.rs"]
mod metrics;

use convergio_db::pool::ConnPool;
use rusqlite::params;
use serde_json::json;

use metrics::PlanMetrics;

/// Extract and persist learnings for a completed plan. Non-blocking.
pub fn extract_plan_learnings(pool: ConnPool, plan_id: i64) {
    tokio::spawn(async move {
        if let Err(e) = do_extract(&pool, plan_id) {
            tracing::warn!(plan_id, error = %e, "learning extraction failed");
        }
    });
}

fn do_extract(pool: &ConnPool, plan_id: i64) -> Result<(), Box<dyn std::error::Error>> {
    let conn = pool.get()?;

    let existing: Option<String> = conn
        .query_row(
            "SELECT key_learnings_json FROM plan_metadata WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    if existing.as_deref().is_some_and(|s| !s.is_empty()) {
        tracing::debug!(plan_id, "learnings already exist — skipping");
        return Ok(());
    }

    let m = metrics::collect_metrics(&conn, plan_id)?;
    let learnings = generate_learnings(&m);

    let json_str = serde_json::to_string(&learnings)?;
    conn.execute(
        "UPDATE plan_metadata SET key_learnings_json = ?1, \
         closed_at = datetime('now') WHERE plan_id = ?2",
        params![json_str, plan_id],
    )?;

    tracing::info!(plan_id, "auto-learnings extracted and stored");
    Ok(())
}

fn parse_review_verdicts(report_json: &Option<String>) -> (Option<String>, Option<String>) {
    let Some(json_str) = report_json else {
        return (None, None);
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return (None, None);
    };
    let verdict = val
        .get("verdict")
        .and_then(|v| v.as_str())
        .map(String::from);
    let review_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match review_type {
        "pre_review" => (verdict, None),
        "post_review" => (None, verdict),
        _ => (verdict.clone(), verdict),
    }
}

fn generate_learnings(m: &PlanMetrics) -> serde_json::Value {
    let mut insights: Vec<String> = Vec::new();

    // Completion rate
    let completion_pct = if m.tasks_total > 0 {
        (m.tasks_done as f64 / m.tasks_total as f64 * 100.0).round()
    } else {
        0.0
    };
    insights.push(format!(
        "Completion: {}/{} tasks done ({completion_pct}%)",
        m.tasks_done, m.tasks_total
    ));

    // Failures
    if m.tasks_failed > 0 {
        insights.push(format!(
            "Warning: {} task(s) failed — investigate root cause",
            m.tasks_failed
        ));
    }

    // Cancellations
    if m.tasks_cancelled > 0 {
        insights.push(format!("{} task(s) cancelled", m.tasks_cancelled));
    }

    // Context exhaustion / respawns
    if m.respawn_count > 0 {
        insights.push(format!(
            "{} respawn(s) — agents hit context limit. Consider splitting tasks",
            m.respawn_count
        ));
    }

    // Evidence gaps
    if m.tasks_without_evidence > 0 {
        insights.push(format!(
            "{} task(s) completed without evidence — enforcement gap",
            m.tasks_without_evidence
        ));
    }

    // Cost efficiency
    if m.tasks_done > 0 && m.cost_usd > 0.0 {
        let cost_per_task = m.cost_usd / m.tasks_done as f64;
        insights.push(format!(
            "Cost: ${:.2} total, ${:.2}/task",
            m.cost_usd, cost_per_task
        ));
    }

    // Duration
    if let Some(mins) = m.duration_minutes {
        if mins > 0.0 {
            let hours = mins / 60.0;
            insights.push(format!("Duration: {hours:.1}h"));
        }
    }

    json!({
        "auto_generated": true,
        "metrics": {
            "tasks_total": m.tasks_total,
            "tasks_done": m.tasks_done,
            "tasks_failed": m.tasks_failed,
            "tasks_cancelled": m.tasks_cancelled,
            "waves_total": m.waves_total,
            "cost_usd": m.cost_usd,
            "total_tokens": m.total_tokens,
            "duration_minutes": m.duration_minutes,
            "agents_used": m.agents_used,
            "respawn_count": m.respawn_count,
            "tasks_without_evidence": m.tasks_without_evidence,
            "completion_pct": completion_pct,
        },
        "reviews": {
            "pre_review": m.pre_review_verdict,
            "post_review": m.post_review_verdict,
        },
        "insights": insights,
    })
}

#[cfg(test)]
#[path = "handlers_learning_tests.rs"]
mod tests;
