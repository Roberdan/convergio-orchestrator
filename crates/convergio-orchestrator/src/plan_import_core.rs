//! Core import logic: YAML spec types and wave/task insertion.

use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct SpecYaml {
    #[serde(default)]
    pub waves: Vec<SpecWave>,
}

#[derive(Debug, Deserialize)]
pub struct SpecWave {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub depends_on_wave: String,
    #[serde(default)]
    pub tasks: Vec<SpecTask>,
}

#[derive(Debug, Deserialize)]
pub struct SpecTask {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, alias = "do")]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, alias = "assignee")]
    pub executor_agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<i64>,
    #[serde(default)]
    pub output_type: Option<String>,
    #[serde(default)]
    pub validator_agent: Option<String>,
    #[serde(default)]
    pub verify: Vec<String>,
}

pub struct ImportStats {
    pub waves_created: u32,
    pub tasks_created: u32,
    pub waves_skipped: u32,
    pub tasks_skipped: u32,
    pub errors: Vec<String>,
}

/// Parse YAML handling the `do` keyword (reserved in serde_yaml).
pub fn parse_spec(yaml_str: &str) -> Result<SpecYaml, String> {
    serde_yaml::from_str(yaml_str).map_err(|e| e.to_string())
}

/// Insert waves and tasks from a parsed spec into the DB.
pub fn import_waves_and_tasks(
    conn: &Connection,
    plan_id: i64,
    spec: &SpecYaml,
    mode: &str,
) -> ImportStats {
    let mut stats = ImportStats {
        waves_created: 0,
        tasks_created: 0,
        waves_skipped: 0,
        tasks_skipped: 0,
        errors: Vec::new(),
    };

    for (idx, wave) in spec.waves.iter().enumerate() {
        let wave_name = if wave.name.is_empty() {
            wave.id.clone()
        } else {
            wave.name.clone()
        };

        let wave_db_id = match resolve_wave(conn, plan_id, wave, &wave_name, mode, &mut stats) {
            Some(id) => id,
            None => continue,
        };

        import_tasks(
            conn,
            plan_id,
            wave_db_id,
            &wave.tasks,
            idx,
            mode,
            &mut stats,
        );
    }

    // Update plan tasks_total
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE plan_id = ?1",
            params![plan_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let _ = conn.execute(
        "UPDATE plans SET tasks_total = ?1 WHERE id = ?2",
        params![total, plan_id],
    );

    stats
}

fn resolve_wave(
    conn: &Connection,
    plan_id: i64,
    wave: &SpecWave,
    wave_name: &str,
    mode: &str,
    stats: &mut ImportStats,
) -> Option<i64> {
    if mode == "merge" {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM waves WHERE plan_id = ?1 AND wave_id = ?2",
                params![plan_id, wave.id],
                |r| r.get(0),
            )
            .ok();
        if let Some(wid) = existing {
            let _ = conn.execute(
                "UPDATE waves SET name = ?1, depends_on_wave = ?2 WHERE id = ?3",
                params![wave_name, wave.depends_on_wave, wid],
            );
            stats.waves_skipped += 1;
            return Some(wid);
        }
        match conn.execute(
            "INSERT INTO waves (wave_id, plan_id, name, depends_on_wave) VALUES (?1, ?2, ?3, ?4)",
            params![wave.id, plan_id, wave_name, wave.depends_on_wave],
        ) {
            Ok(_) => {
                stats.waves_created += 1;
                Some(conn.last_insert_rowid())
            }
            Err(e) => {
                stats.errors.push(format!("wave {}: {e}", wave.id));
                None
            }
        }
    } else {
        match conn.execute(
            "INSERT OR IGNORE INTO waves (wave_id, plan_id, name, depends_on_wave) \
             VALUES (?1, ?2, ?3, ?4)",
            params![wave.id, plan_id, wave_name, wave.depends_on_wave],
        ) {
            Ok(n) if n > 0 => {
                stats.waves_created += 1;
                Some(conn.last_insert_rowid())
            }
            Ok(_) => {
                stats.waves_skipped += 1;
                conn.query_row(
                    "SELECT id FROM waves WHERE plan_id = ?1 AND wave_id = ?2",
                    params![plan_id, wave.id],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(|e| stats.errors.push(format!("wave {} lookup: {e}", wave.id)))
                .ok()
            }
            Err(e) => {
                stats.errors.push(format!("wave {}: {e}", wave.id));
                None
            }
        }
    }
}

fn import_tasks(
    conn: &Connection,
    plan_id: i64,
    wave_db_id: i64,
    tasks: &[SpecTask],
    wave_order: usize,
    mode: &str,
    stats: &mut ImportStats,
) {
    for task in tasks {
        let title = task.title.as_deref().unwrap_or("untitled");
        let title = if title.len() > 500 {
            &title[..500]
        } else {
            title
        };
        let task_id = task.id.as_deref().unwrap_or("");

        let metadata = json!({
            "effort": task.effort,
            "model": task.model,
            "description": task.description,
            "output_type": task.output_type,
            "validator_agent": task.validator_agent,
            "verify": task.verify,
            "order": wave_order,
        });

        // Merge mode: try to update existing task first
        if mode == "merge" && !task_id.is_empty() {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM tasks WHERE plan_id = ?1 AND task_id = ?2",
                    params![plan_id, task_id],
                    |r| r.get(0),
                )
                .ok();
            if let Some(tid) = existing {
                match conn.execute(
                    "UPDATE tasks SET title = ?1, executor_agent = ?2, \
                     metadata = ?3, wave_id = ?4 WHERE id = ?5",
                    params![
                        title,
                        task.executor_agent,
                        metadata.to_string(),
                        wave_db_id,
                        tid
                    ],
                ) {
                    Ok(_) => stats.tasks_skipped += 1,
                    Err(e) => stats.errors.push(format!("task {task_id} merge: {e}")),
                }
                continue;
            }
        }

        let insert_sql = if mode == "append" {
            "INSERT OR IGNORE INTO tasks (plan_id, wave_id, task_id, title, \
             status, executor_agent, metadata) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6)"
        } else {
            "INSERT INTO tasks (plan_id, wave_id, task_id, title, \
             status, executor_agent, metadata) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6)"
        };

        match conn.execute(
            insert_sql,
            params![
                plan_id,
                wave_db_id,
                task_id,
                title,
                task.executor_agent,
                metadata.to_string()
            ],
        ) {
            Ok(n) if n > 0 => stats.tasks_created += 1,
            Ok(_) => stats.tasks_skipped += 1,
            Err(e) => stats.errors.push(format!("task {task_id}: {e}")),
        }
    }
}

#[cfg(test)]
#[path = "plan_import_core_tests.rs"]
mod tests;
