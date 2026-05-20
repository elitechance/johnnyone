CREATE TABLE IF NOT EXISTS agent_plans (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    workspace_path TEXT NOT NULL,
    plan_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'draft',
    worker_session_id TEXT,
    reviewer_session_id TEXT,
    worker_provider TEXT NOT NULL,
    reviewer_provider TEXT NOT NULL,
    current_phase_id TEXT,
    current_phase_index INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY(worker_session_id) REFERENCES sessions(id) ON DELETE SET NULL,
    FOREIGN KEY(reviewer_session_id) REFERENCES sessions(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS agent_plan_phases (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    phase_id TEXT NOT NULL,
    phase_title TEXT NOT NULL,
    phase_index INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'locked',
    worker_started_at TEXT,
    worker_idle_at TEXT,
    reviewer_started_at TEXT,
    reviewer_idle_at TEXT,
    gate_verdict TEXT NOT NULL DEFAULT 'unknown',
    clarification_attempts INTEGER NOT NULL DEFAULT 0,
    summary TEXT,
    findings_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(plan_id, phase_id),
    FOREIGN KEY(plan_id) REFERENCES agent_plans(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS agent_plan_tasks (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    phase_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    task_title TEXT NOT NULL,
    task_index INTEGER NOT NULL,
    prompt_path TEXT NOT NULL,
    status_path TEXT,
    decisions_path TEXT,
    status TEXT NOT NULL DEFAULT 'planned',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(plan_id, phase_id, task_id),
    FOREIGN KEY(plan_id) REFERENCES agent_plans(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS agent_plan_events (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    phase_id TEXT,
    type TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY(plan_id) REFERENCES agent_plans(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_agent_plans_status ON agent_plans(status, updated_at);
CREATE INDEX IF NOT EXISTS idx_agent_plan_phases_plan ON agent_plan_phases(plan_id, phase_index);
CREATE INDEX IF NOT EXISTS idx_agent_plan_tasks_plan_phase ON agent_plan_tasks(plan_id, phase_id, task_index);
CREATE INDEX IF NOT EXISTS idx_agent_plan_events_plan ON agent_plan_events(plan_id, created_at);
