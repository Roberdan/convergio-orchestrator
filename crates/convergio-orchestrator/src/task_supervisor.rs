//! TaskSupervisor — tracks tokio::spawn calls, monitors health, restarts on failure.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Policy for what to do when a supervised task exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Re-spawn the task using the stored factory.
    Restart,
    /// Log and ignore — the task is non-critical.
    Ignore,
    /// Shut down the daemon — the task is load-bearing.
    Shutdown,
}

/// Metadata for a supervised task.
struct TaskEntry {
    handle: JoinHandle<()>,
    policy: RestartPolicy,
    #[allow(dead_code)]
    registered_at: std::time::Instant,
    restart_count: u32,
}

/// Supervises background tokio tasks.
///
/// Register tasks with a name + policy. A background monitor loop
/// checks liveness every 30 s and applies the restart policy.
#[derive(Clone)]
pub struct TaskSupervisor {
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
}

impl TaskSupervisor {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a task handle with a name and restart policy.
    pub async fn register(
        &self,
        name: impl Into<String>,
        handle: JoinHandle<()>,
        policy: RestartPolicy,
    ) {
        let name = name.into();
        tracing::info!(task = %name, ?policy, "task_supervisor: registered");
        self.tasks.lock().await.insert(
            name,
            TaskEntry {
                handle,
                policy,
                registered_at: std::time::Instant::now(),
                restart_count: 0,
            },
        );
    }

    /// Number of currently tracked tasks.
    pub async fn task_count(&self) -> usize {
        self.tasks.lock().await.len()
    }

    /// Check all tasks once: returns (alive, dead) counts.
    pub async fn check_all(&self) -> (usize, usize) {
        let tasks = self.tasks.lock().await;
        let mut alive = 0usize;
        let mut dead = 0usize;
        for (name, entry) in tasks.iter() {
            if entry.handle.is_finished() {
                tracing::warn!(task = %name, policy = ?entry.policy, "task_supervisor: task finished");
                dead += 1;
            } else {
                alive += 1;
            }
        }
        (alive, dead)
    }

    /// Spawn the background monitor loop (30 s interval).
    ///
    /// For each finished task:
    /// - `Restart` → logs a warning (actual re-spawn needs a factory; placeholder)
    /// - `Ignore`  → removes the entry
    /// - `Shutdown` → logs critical error (caller should wire to graceful shutdown)
    pub fn spawn_monitor_loop(&self) -> JoinHandle<()> {
        let tasks = Arc::clone(&self.tasks);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let mut guard = tasks.lock().await;
                let mut to_remove = Vec::new();

                for (name, entry) in guard.iter() {
                    if !entry.handle.is_finished() {
                        continue;
                    }
                    match entry.policy {
                        RestartPolicy::Restart => {
                            tracing::warn!(
                                task = %name,
                                restarts = entry.restart_count,
                                "task_supervisor: task died, needs restart (factory TBD)"
                            );
                            // Future: call factory closure to re-spawn.
                            // For now, remove so we don't spam logs.
                            to_remove.push(name.clone());
                        }
                        RestartPolicy::Ignore => {
                            tracing::info!(
                                task = %name,
                                "task_supervisor: task finished (Ignore policy), removing"
                            );
                            to_remove.push(name.clone());
                        }
                        RestartPolicy::Shutdown => {
                            tracing::error!(
                                task = %name,
                                "task_supervisor: CRITICAL task died! Shutdown required."
                            );
                            // Future: trigger graceful shutdown signal.
                            to_remove.push(name.clone());
                        }
                    }
                }

                for name in to_remove {
                    guard.remove(&name);
                }
            }
        })
    }
}

impl Default for TaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_tracks_task() {
        let sup = TaskSupervisor::new();
        let handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        sup.register("long-task", handle, RestartPolicy::Ignore)
            .await;
        assert_eq!(sup.task_count().await, 1);
    }

    #[tokio::test]
    async fn check_all_detects_finished() {
        let sup = TaskSupervisor::new();
        let handle = tokio::spawn(async {}); // finishes immediately
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        sup.register("quick", handle, RestartPolicy::Ignore).await;
        let (alive, dead) = sup.check_all().await;
        assert_eq!(alive, 0);
        assert_eq!(dead, 1);
    }

    #[tokio::test]
    async fn check_all_alive_task() {
        let sup = TaskSupervisor::new();
        let handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        sup.register("sleeper", handle, RestartPolicy::Restart)
            .await;
        let (alive, dead) = sup.check_all().await;
        assert_eq!(alive, 1);
        assert_eq!(dead, 0);
    }

    #[tokio::test]
    async fn default_creates_empty() {
        let sup = TaskSupervisor::default();
        assert_eq!(sup.task_count().await, 0);
    }
}
