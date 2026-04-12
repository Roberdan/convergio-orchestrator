// Wave lifecycle handlers — extracted to stay under 250 lines.
use super::AliResult;
use convergio_db::pool::ConnPool;
use convergio_types::events::DomainEventSink;
use rusqlite::params;
use std::sync::Arc;
use tokio::sync::Notify;

#[tracing::instrument(skip_all, fields(%wave_id, %plan_id))]
pub fn on_wave_done(
    pool: &ConnPool,
    notify: &Arc<Notify>,
    wave_id: i64,
    plan_id: i64,
) -> AliResult {
    tracing::info!("ali: wave {wave_id} complete for plan {plan_id}, requesting validation");
    crate::actions::emit(
        pool,
        notify,
        "wave_needs_validation",
        &serde_json::json!({"wave_id": wave_id, "plan_id": plan_id}),
    )
}

#[tracing::instrument(skip_all, fields(%wave_id, %plan_id))]
pub fn on_wave_validated(
    pool: &ConnPool,
    notify: &Arc<Notify>,
    event_sink: &Option<Arc<dyn DomainEventSink>>,
    wave_id: i64,
    plan_id: i64,
) -> AliResult {
    let conn = pool.get()?;

    // Thor evidence gate: verify each submitted task has evidence (#868)
    let no_evidence: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.title FROM tasks t \
             WHERE t.plan_id = ?1 AND t.wave_id = ?2 AND t.status = 'submitted' \
               AND NOT EXISTS ( \
                 SELECT 1 FROM task_evidence e WHERE e.task_db_id = t.id \
               )",
        )?;
        let rows: Vec<_> = stmt
            .query_map(params![plan_id, wave_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };
    if !no_evidence.is_empty() {
        let ids: Vec<String> = no_evidence.iter().map(|(id, _)| id.to_string()).collect();
        tracing::warn!(
            "Thor: wave {wave_id} has {} tasks without evidence: [{}]",
            no_evidence.len(),
            ids.join(", ")
        );
    }

    // Collect IDs before promoting (for audit trail)
    let pids: Vec<i64> = conn
        .prepare("SELECT id FROM tasks WHERE plan_id=?1 AND wave_id=?2 AND status='submitted'")?
        .query_map(params![plan_id, wave_id], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    let promoted = conn.execute(
        "UPDATE tasks SET status = 'done', completed_at = datetime('now') \
         WHERE plan_id = ?1 AND wave_id = ?2 AND status = 'submitted'",
        params![plan_id, wave_id],
    )?;
    for tid in &pids {
        crate::audit::log_status_change(
            &conn,
            *tid,
            "submitted",
            "done",
            "orchestrator",
            Some("wave promote"),
        );
    }
    if promoted > 0 {
        tracing::info!("ali: promoted {promoted} submitted tasks to done in wave {wave_id}");
    }

    conn.execute(
        "UPDATE waves SET status = 'done', completed_at = datetime('now') WHERE id = ?1",
        params![wave_id],
    )?;

    if let Some(ref sink) = event_sink {
        sink.emit(convergio_types::events::make_event(
            "orchestrator",
            convergio_types::events::EventKind::WaveCompleted { wave_id, plan_id },
            convergio_types::events::EventContext {
                plan_id: Some(plan_id),
                ..Default::default()
            },
        ));
    }

    let next_wave: Option<i64> = conn
        .query_row(
            "SELECT id FROM waves WHERE plan_id = ?1 AND id > ?2 \
             AND status NOT IN ('done', 'cancelled') \
             ORDER BY id LIMIT 1",
            params![plan_id, wave_id],
            |row| row.get(0),
        )
        .ok();

    if let Some(next) = next_wave {
        let next_status: String = conn
            .query_row(
                "SELECT status FROM waves WHERE id = ?1",
                params![next],
                |r| r.get(0),
            )
            .unwrap_or_default();
        if next_status == "pending" {
            tracing::info!(
                "ali: wave {wave_id} validated, starting wave {next} for plan {plan_id}"
            );
            crate::actions::emit(
                pool,
                notify,
                "wave_ready",
                &serde_json::json!({"wave_id": next, "plan_id": plan_id}),
            )?;
        } else {
            tracing::info!("ali: wave {wave_id} validated, wave {next} is {next_status}");
        }
    } else {
        tracing::info!("ali: all waves done for plan {plan_id}, plan done");
        crate::actions::emit(
            pool,
            notify,
            "plan_done",
            &serde_json::json!({"plan_id": plan_id}),
        )?;
    }

    Ok(())
}
