-- Plans tab (initiative 8ad3ae71, phase 00): a briefing Initiative owns an APPEND-ONLY history of
-- structured brief versions. Every write — a human edit over GraphQL or the plan agent over MCP —
-- goes through `services::brief_store::write_version` (the ONE chokepoint), which inserts version
-- n+1 and re-renders `agent_plans.brief`. Nothing in `brief_versions` is ever UPDATEd: an undo or
-- revert is a NEW version that copies old content.
--
-- `mcp_tokens` holds the plan agent's per-session MCP bearer credential. Only the hex SHA-256 of the
-- raw token is stored (brief §"MCP security" 1) so a DB read never yields a usable credential; the
-- partial unique index keeps at most ONE live (non-revoked) token per plan.
--
-- `agent_plans.accepted_brief_version` records the version frozen by Start planning (NULL until
-- accept); `agent_plans.planning_queued_after` is a JSON array of initiative ids a queued Start
-- planning waits on (NULL = not queued).
CREATE TABLE IF NOT EXISTS brief_versions (
  initiative_id  TEXT NOT NULL,
  version        INTEGER NOT NULL,                 -- 1..n, dense, per initiative
  author         TEXT NOT NULL CHECK (author IN ('you','agent')),
  fields_json    TEXT NOT NULL,                    -- {outcome, why, where:[..], done_when, out_of_scope, decided}
  outline_json   TEXT NOT NULL,                    -- {phases:[{id:"00",title,author,addedIn,tasks:[{id:"00/01",title,author,addedIn}]}]}
  changed_json   TEXT NOT NULL DEFAULT '[]',       -- fields/outline ids that differ from version-1 (computed at write)
  change_note    TEXT NOT NULL DEFAULT '',
  created_at     TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (initiative_id, version),
  FOREIGN KEY (initiative_id) REFERENCES agent_plans(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS mcp_tokens (
  token_hash     TEXT PRIMARY KEY,                 -- hex SHA-256 of the raw token; the raw token is NEVER stored
  session_id     TEXT NOT NULL,                    -- the plan-agent chat session (agent_plans.briefing_session_id)
  initiative_id  TEXT NOT NULL,
  role           TEXT NOT NULL CHECK (role IN ('plan_agent')),
  created_at     TEXT NOT NULL DEFAULT (datetime('now')),
  revoked_at     TEXT,
  FOREIGN KEY (initiative_id) REFERENCES agent_plans(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mcp_tokens_active ON mcp_tokens(initiative_id) WHERE revoked_at IS NULL;
ALTER TABLE agent_plans ADD COLUMN accepted_brief_version INTEGER;
ALTER TABLE agent_plans ADD COLUMN planning_queued_after TEXT;
