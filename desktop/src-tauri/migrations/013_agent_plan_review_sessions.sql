-- Ephemeral agent sessions registered to a plan (lens reviewers, docs agent, etc.).
-- reportAgentResult trusts a session only if it's the plan's worker/reviewer OR is
-- listed here. The role/lens is derived from this table — never self-declared by
-- the agent — so attribution is tamper-proof.
CREATE TABLE IF NOT EXISTS agent_plan_review_sessions (
    session_id TEXT PRIMARY KEY,
    plan_id    TEXT NOT NULL,
    role       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_agent_plan_review_sessions_plan
    ON agent_plan_review_sessions(plan_id);
