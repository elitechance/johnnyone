-- Initiative spine (overhaul phase 1). Three additive axes on agent_plans:
--   initiative_id      groups a planning stage-run + a development stage-run.
--   initiative_status  lifecycle stage (briefing|planning|development|review|done).
--   health             condition (in-progress|needs-attention|blocked), independent of stage.
-- The legacy `status` column (execution state) is intentionally left untouched.
-- Backfill lives in 017_backfill_initiative_axes.sql (shared with the unit test).

ALTER TABLE agent_plans ADD COLUMN initiative_id TEXT;
ALTER TABLE agent_plans ADD COLUMN initiative_status TEXT;
ALTER TABLE agent_plans ADD COLUMN health TEXT NOT NULL DEFAULT 'in-progress';

CREATE INDEX IF NOT EXISTS idx_agent_plans_initiative ON agent_plans(initiative_id, updated_at);
