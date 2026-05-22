ALTER TABLE agent_plans ADD COLUMN run_type TEXT NOT NULL DEFAULT 'development';
ALTER TABLE agent_plans ADD COLUMN brief TEXT;
ALTER TABLE agent_plans ADD COLUMN app_scope TEXT;
ALTER TABLE agent_plans ADD COLUMN docs_scope TEXT;
ALTER TABLE agent_plans ADD COLUMN reference_paths TEXT;

CREATE INDEX IF NOT EXISTS idx_agent_plans_run_type ON agent_plans(run_type, updated_at);
