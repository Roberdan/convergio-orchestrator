// OrchestratorExtension — impl Extension for the orchestrator module.

use std::sync::Arc;
use tokio::sync::Notify;

use convergio_db::pool::ConnPool;
use convergio_types::extension::{
    AppContext, ExtResult, Extension, Health, McpToolDef, Metric, Migration, ScheduledTask,
};
use convergio_types::manifest::{Capability, Manifest, ModuleKind};

pub struct OrchestratorExtension {
    pool: ConnPool,
    notify: Arc<Notify>,
}

impl OrchestratorExtension {
    pub fn new(pool: ConnPool, notify: Arc<Notify>) -> Self {
        Self { pool, notify }
    }

    pub fn pool(&self) -> &ConnPool {
        &self.pool
    }
}

impl Extension for OrchestratorExtension {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "convergio-orchestrator".to_string(),
            description: "Plans, tasks, waves, Thor gate, reaper".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            kind: ModuleKind::Platform,
            provides: vec![
                Capability {
                    name: "plan-management".to_string(),
                    version: "1.0".to_string(),
                    description: "Create and manage execution plans".to_string(),
                },
                Capability {
                    name: "task-orchestration".to_string(),
                    version: "1.0".to_string(),
                    description: "Task lifecycle, wave progression, delegation".to_string(),
                },
                Capability {
                    name: "validation".to_string(),
                    version: "1.0".to_string(),
                    description: "Thor gate validation queue".to_string(),
                },
            ],
            requires: vec![],
            agent_tools: vec![],
            required_roles: vec![],
        }
    }

    fn migrations(&self) -> Vec<Migration> {
        let mut m = crate::schema::migrations();
        m.extend(crate::schema_merge::merge_guardian_migrations());
        m.extend(crate::schema_wave_branch::wave_branch_migrations());
        m.extend(crate::schema_wave_branch::sync_fix_migrations());
        m.extend(crate::schema_file_locks::file_locks_migrations());
        m.extend(crate::schema_pr_deps::pr_deps_migrations());
        m.extend(crate::schema_workflow::workflow_migrations());
        m.extend(crate::schema_wave_deps::wave_deps_migrations());
        m.extend(crate::schema_task_lock::task_lock_migrations());
        m.extend(crate::schema_plan_reviews::plan_reviews_migrations());
        m.extend(convergio_ceo::ceo_audit::ceo_audit_migrations());
        m.sort_by_key(|mig| mig.version);
        m
    }

    fn routes(&self, ctx: &AppContext) -> Option<axum::Router> {
        let sink = ctx.get_arc::<Arc<dyn convergio_types::events::DomainEventSink>>();
        let state = Arc::new(crate::plan_routes::PlanState {
            pool: self.pool.clone(),
            event_sink: sink.map(|s| (*s).clone()),
            notify: self.notify.clone(),
        });
        let router = convergio_scaffold::scaffold_routes()
            .merge(crate::plan_routes::plan_routes(Arc::clone(&state)))
            .merge(crate::plan_routes_ext::plan_routes_ext(Arc::clone(&state)))
            .merge(crate::plan_import::import_routes(Arc::clone(&state)))
            .merge(crate::plan_readiness::readiness_routes(Arc::clone(&state)))
            .merge(crate::plan_validate::validate_routes(Arc::clone(&state)))
            .merge(crate::plan_review::review_routes(Arc::clone(&state)))
            .merge(crate::task_routes::task_routes(Arc::clone(&state)))
            .merge(crate::audit::audit_routes(Arc::clone(&state)))
            .merge(crate::wave_branch_routes::wave_branch_routes(Arc::clone(
                &state,
            )))
            .merge(crate::heartbeat_routes::heartbeat_routes(state.clone()))
            .merge(crate::tracking_routes::tracking_routes(self.pool.clone()))
            .merge(crate::pm_routes::pm_routes(self.pool.clone()))
            .merge(crate::aggregation_routes::aggregation_routes(
                self.pool.clone(),
            ))
            .merge(crate::artifact_routes::artifact_routes(self.pool.clone()))
            .merge(crate::bundle_routes::bundle_routes(self.pool.clone()))
            .merge(crate::compensation_routes::compensation_routes(
                self.pool.clone(),
            ))
            .merge(crate::approval_routes::approval_routes(self.pool.clone()))
            .merge(crate::evaluation_routes::evaluation_routes(
                self.pool.clone(),
            ))
            .merge(crate::plan_context::context_routes(self.pool.clone()))
            .merge(crate::plan_kb::kb_routes(self.pool.clone()))
            .merge(crate::merge_guardian::merge_guardian_routes(
                self.pool.clone(),
            ))
            .merge(crate::skill_routes::skill_routes())
            .merge(crate::file_locks::file_lock_routes(self.pool.clone()))
            .merge(crate::pr_dependencies::pr_dependency_routes(
                self.pool.clone(),
            ))
            .merge(crate::project_routes::project_routes(self.pool.clone()))
            .merge(crate::workflow_routes::workflow_routes(self.pool.clone()))
            .merge(crate::file_size_scan::lint_routes(Arc::clone(&state)))
            .merge(crate::plan_force_ops::force_ops_routes(Arc::clone(&state)))
            .merge(crate::claim_routes::claim_routes(Arc::clone(&state)))
            .merge(convergio_ceo::ceo_routes::ceo_routes(
                &std::env::var("CONVERGIO_DAEMON_URL")
                    .unwrap_or_else(|_| "http://localhost:8420".to_string()),
                std::env::var("CONVERGIO_API_TOKEN").ok().as_deref(),
            ))
            .merge(convergio_ceo::ceo_audit::ceo_audit_routes(
                self.pool.clone(),
            ));
        Some(router)
    }

    fn on_start(&self, ctx: &AppContext) -> ExtResult<()> {
        // Self-heal: ensure columns exist even if migration registry drifted
        if let Ok(conn) = self.pool.get() {
            if let Err(e) = crate::schema::ensure_required_capabilities_column(&conn) {
                tracing::warn!("schema self-heal (capabilities): {e}");
            }
            if let Err(e) = crate::schema::ensure_claimed_files_column(&conn) {
                tracing::warn!("schema self-heal (claimed_files): {e}");
            }
        }
        tracing::info!("orchestrator: starting reactor, validator, reaper");

        // Spawn Ali reactor with event sink for domain events
        let pool = self.pool.clone();
        let notify = self.notify.clone();
        let sink = ctx
            .get_arc::<Arc<dyn convergio_types::events::DomainEventSink>>()
            .map(|s| (*s).clone());
        tokio::spawn(async move {
            crate::reactor::run(pool, notify, sink).await;
        });

        // Spawn validator loop
        crate::validator::spawn_validator_loop(self.pool.clone());

        // Spawn reaper
        crate::reaper::spawn_reaper(self.pool.clone());

        // Spawn plan zombie reaper (closes stale plans, repairs done-with-open-tasks)
        crate::plan_zombie_reaper::spawn_plan_zombie_reaper(self.pool.clone());

        // Spawn autonomous plan executor loop (Plan Zero W2)
        crate::plan_executor::spawn_plan_executor_loop(self.pool.clone());

        // Spawn agent health monitor (Plan Zero W3)
        crate::agent_health::spawn_health_monitor(self.pool.clone());

        // Spawn plan sequencer (Plan Zero W4)
        crate::plan_sequencer::spawn_plan_sequencer(self.pool.clone());

        // Spawn advisory lock expiry loop (Epsilon W1)
        crate::file_locks::spawn_lock_expiry_loop(self.pool.clone());

        Ok(())
    }

    fn health(&self) -> Health {
        match self.pool.get() {
            Ok(conn) => {
                let ok = conn
                    .query_row("SELECT COUNT(*) FROM plans", [], |r| r.get::<_, i64>(0))
                    .is_ok();
                if ok {
                    Health::Ok
                } else {
                    Health::Degraded {
                        reason: "plans table inaccessible".into(),
                    }
                }
            }
            Err(e) => {
                tracing::error!("orchestrator health check pool error: {e}");
                Health::Down {
                    reason: "database pool unavailable".into(),
                }
            }
        }
    }

    fn metrics(&self) -> Vec<Metric> {
        let conn = match self.pool.get() {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        let mut metrics = Vec::new();
        if let Ok(n) = conn.query_row("SELECT COUNT(*) FROM plans", [], |r| r.get::<_, f64>(0)) {
            metrics.push(Metric {
                name: "orchestrator.plans.total".into(),
                value: n,
                labels: vec![],
            });
        }
        if let Ok(n) = conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE status='in_progress'",
            [],
            |r| r.get::<_, f64>(0),
        ) {
            metrics.push(Metric {
                name: "orchestrator.tasks.active".into(),
                value: n,
                labels: vec![],
            });
        }
        if let Ok(n) = conn.query_row(
            "SELECT COUNT(*) FROM validation_queue WHERE status='pending'",
            [],
            |r| r.get::<_, f64>(0),
        ) {
            metrics.push(Metric {
                name: "orchestrator.validations.pending".into(),
                value: n,
                labels: vec![],
            });
        }
        metrics
    }

    fn scheduled_tasks(&self) -> Vec<ScheduledTask> {
        vec![
            ScheduledTask {
                name: "reaper",
                cron: "*/5 * * * *",
            },
            ScheduledTask {
                name: "validator",
                cron: "* * * * *",
            },
        ]
    }

    fn mcp_tools(&self) -> Vec<McpToolDef> {
        crate::mcp_defs::orchestrator_tools()
    }
}
