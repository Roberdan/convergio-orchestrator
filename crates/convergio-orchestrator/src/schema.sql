-- Orchestrator schema v1: all tables in a single migration.

CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    output_path TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS plans (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id       TEXT    NOT NULL,
    name             TEXT    NOT NULL,
    status           TEXT    NOT NULL DEFAULT 'todo',
    tasks_done       INTEGER NOT NULL DEFAULT 0,
    tasks_total      INTEGER NOT NULL DEFAULT 0,
    depends_on       TEXT,
    execution_mode   TEXT,
    is_master        INTEGER NOT NULL DEFAULT 0,
    parent_plan_id   INTEGER,
    execution_host   TEXT,
    started_at       TEXT,
    completed_at     TEXT,
    cancelled_at     TEXT,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_plans_status ON plans(status);
CREATE INDEX IF NOT EXISTS idx_plans_parent ON plans(parent_plan_id);

CREATE TABLE IF NOT EXISTS waves (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    wave_id    TEXT    NOT NULL,
    plan_id    INTEGER NOT NULL,
    name       TEXT    NOT NULL DEFAULT '',
    status     TEXT    NOT NULL DEFAULT 'pending',
    started_at TEXT,
    completed_at TEXT,
    cancelled_at TEXT,
    FOREIGN KEY (plan_id) REFERENCES plans(id)
);
CREATE INDEX IF NOT EXISTS idx_waves_plan ON waves(plan_id);

CREATE TABLE IF NOT EXISTS tasks (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id        TEXT,
    plan_id        INTEGER NOT NULL,
    wave_id        INTEGER,
    title          TEXT    NOT NULL DEFAULT '',
    description    TEXT,
    status         TEXT    NOT NULL DEFAULT 'pending',
    executor_agent TEXT,
    started_at     TEXT,
    completed_at   TEXT,
    notes          TEXT,
    tokens         INTEGER,
    output_data    TEXT,
    executor_host  TEXT,
    validated_at   TEXT,
    validator_agent TEXT,
    duration_minutes REAL,
    last_heartbeat TEXT,
    metadata       TEXT,
    FOREIGN KEY (plan_id) REFERENCES plans(id),
    FOREIGN KEY (wave_id) REFERENCES waves(id)
);
CREATE INDEX IF NOT EXISTS idx_tasks_plan ON tasks(plan_id);
CREATE INDEX IF NOT EXISTS idx_tasks_wave ON tasks(wave_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);

CREATE TABLE IF NOT EXISTS validation_queue (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id      INTEGER,
    wave_id      INTEGER,
    plan_id      INTEGER,
    status       TEXT    NOT NULL DEFAULT 'pending',
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    started_at   TEXT,
    completed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_vq_task ON validation_queue(task_id);
CREATE INDEX IF NOT EXISTS idx_vq_status ON validation_queue(status);

CREATE TABLE IF NOT EXISTS validation_verdicts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    queue_id   INTEGER NOT NULL,
    verdict    TEXT    NOT NULL,
    report     TEXT,
    validator  TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_vv_queue ON validation_verdicts(queue_id);

CREATE TABLE IF NOT EXISTS audit_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    action      TEXT NOT NULL,
    entity_type TEXT,
    entity_id   INTEGER,
    actor       TEXT,
    details     TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS execution_policy (
    id                        INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id                TEXT    NOT NULL,
    risk_level                TEXT    NOT NULL,
    auto_progress             INTEGER NOT NULL DEFAULT 1,
    require_human             INTEGER NOT NULL DEFAULT 0,
    require_double_validation INTEGER NOT NULL DEFAULT 0,
    UNIQUE (project_id, risk_level)
);

CREATE TABLE IF NOT EXISTS rollback_snapshots (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id       INTEGER,
    git_ref       TEXT NOT NULL,
    changed_files TEXT,
    db_rows_json  TEXT,
    created_at    TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS approval_cache (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    reason_code TEXT NOT NULL,
    task_type   TEXT NOT NULL,
    approved_by TEXT NOT NULL,
    created_at  TEXT DEFAULT (datetime('now')),
    UNIQUE (reason_code, task_type)
);

CREATE TABLE IF NOT EXISTS batch_approvals (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    approved_by TEXT NOT NULL,
    created_at  TEXT DEFAULT (datetime('now')),
    UNIQUE (task_id)
);

CREATE TABLE IF NOT EXISTS token_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id INTEGER REFERENCES plans(id),
    wave_id INTEGER,
    task_id INTEGER,
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    execution_host TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_tu_plan ON token_usage(plan_id);
CREATE INDEX IF NOT EXISTS idx_tu_agent ON token_usage(agent);
CREATE INDEX IF NOT EXISTS idx_tu_model ON token_usage(model);
CREATE INDEX IF NOT EXISTS idx_tu_created ON token_usage(created_at);

CREATE TABLE IF NOT EXISTS agent_activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL,
    agent_type TEXT,
    plan_id INTEGER REFERENCES plans(id),
    task_id INTEGER,
    action TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'started',
    model TEXT,
    tokens_in INTEGER NOT NULL DEFAULT 0,
    tokens_out INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at TEXT,
    duration_s REAL,
    host TEXT,
    exit_reason TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_aa_agent ON agent_activity(agent_id);
CREATE INDEX IF NOT EXISTS idx_aa_plan ON agent_activity(plan_id);
CREATE INDEX IF NOT EXISTS idx_aa_status ON agent_activity(status);

CREATE TABLE IF NOT EXISTS plan_metadata (
    plan_id INTEGER PRIMARY KEY REFERENCES plans(id),
    objective TEXT,
    motivation TEXT,
    requester TEXT,
    created_by TEXT,
    approved_by TEXT,
    key_learnings_json TEXT,
    report_json TEXT,
    closed_at TEXT,
    worktree_path TEXT
);

CREATE TABLE IF NOT EXISTS delegation_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id INTEGER REFERENCES plans(id),
    task_id INTEGER,
    peer_name TEXT,
    agent TEXT,
    delegated_at TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    cost_usd REAL NOT NULL DEFAULT 0.0,
    tokens_total INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_dl_plan ON delegation_log(plan_id);
CREATE INDEX IF NOT EXISTS idx_dl_status ON delegation_log(status);

CREATE TABLE IF NOT EXISTS artifacts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id       INTEGER NOT NULL,
    plan_id       INTEGER NOT NULL,
    name          TEXT NOT NULL,
    artifact_type TEXT NOT NULL DEFAULT 'document',
    path          TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL DEFAULT 0,
    mime_type     TEXT DEFAULT 'application/octet-stream',
    content_hash  TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_artifacts_task ON artifacts(task_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_plan ON artifacts(plan_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_type ON artifacts(artifact_type);

CREATE TABLE IF NOT EXISTS artifact_bundles (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id      INTEGER NOT NULL,
    name         TEXT NOT NULL,
    bundle_type  TEXT NOT NULL DEFAULT 'deliverable',
    status       TEXT NOT NULL DEFAULT 'draft',
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    published_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_bundles_plan ON artifact_bundles(plan_id);
CREATE INDEX IF NOT EXISTS idx_bundles_status ON artifact_bundles(status);

CREATE TABLE IF NOT EXISTS bundle_artifacts (
    bundle_id   INTEGER NOT NULL,
    artifact_id INTEGER NOT NULL,
    added_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (bundle_id, artifact_id)
);

CREATE TABLE IF NOT EXISTS compensation_actions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id         INTEGER NOT NULL,
    wave_id         INTEGER NOT NULL,
    task_id         INTEGER NOT NULL,
    action_type     TEXT NOT NULL,
    target          TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'pending',
    error_message   TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_compensations_wave ON compensation_actions(wave_id);
CREATE INDEX IF NOT EXISTS idx_compensations_plan ON compensation_actions(plan_id);
CREATE INDEX IF NOT EXISTS idx_compensations_status ON compensation_actions(status);

CREATE TABLE IF NOT EXISTS approval_requests (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id         INTEGER NOT NULL,
    task_id         INTEGER,
    approval_type   TEXT NOT NULL,
    requester       TEXT NOT NULL,
    reason          TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'pending',
    reviewer        TEXT,
    review_comment  TEXT,
    reviewed_at     TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_approvals_status ON approval_requests(status);
CREATE INDEX IF NOT EXISTS idx_approvals_plan ON approval_requests(plan_id);

CREATE TABLE IF NOT EXISTS approval_thresholds (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger_type       TEXT NOT NULL UNIQUE,
    threshold_value    REAL NOT NULL DEFAULT 0,
    require_approval   INTEGER NOT NULL DEFAULT 1,
    auto_approve_below REAL NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS plan_evaluations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id         INTEGER NOT NULL,
    evaluator       TEXT NOT NULL DEFAULT 'system',
    tasks_total     INTEGER NOT NULL DEFAULT 0,
    tasks_completed INTEGER NOT NULL DEFAULT 0,
    tasks_failed    INTEGER NOT NULL DEFAULT 0,
    false_positives INTEGER NOT NULL DEFAULT 0,
    false_negatives INTEGER NOT NULL DEFAULT 0,
    precision_score REAL NOT NULL DEFAULT 0,
    recall_score    REAL NOT NULL DEFAULT 0,
    f1_score        REAL NOT NULL DEFAULT 0,
    total_cost_usd  REAL NOT NULL DEFAULT 0,
    total_duration_secs INTEGER NOT NULL DEFAULT 0,
    evaluated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_evaluations_plan ON plan_evaluations(plan_id);

CREATE TABLE IF NOT EXISTS review_outcomes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id         INTEGER NOT NULL,
    task_id         INTEGER NOT NULL,
    thor_decision   TEXT NOT NULL,
    actual_outcome  TEXT NOT NULL,
    is_correct      INTEGER NOT NULL DEFAULT 0,
    recorded_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_review_outcomes_plan ON review_outcomes(plan_id);

CREATE TABLE IF NOT EXISTS task_status_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id    INTEGER NOT NULL,
    old_status TEXT NOT NULL,
    new_status TEXT NOT NULL,
    agent      TEXT NOT NULL DEFAULT '',
    notes      TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_tsl_task ON task_status_log(task_id);
CREATE INDEX IF NOT EXISTS idx_tsl_created ON task_status_log(created_at);
