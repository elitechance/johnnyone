-- Per-stage provider binding for the continuous SDLC loop.
--
-- An initiative is two agent_plans rows sharing an initiative_id: a planning stage-run and a
-- development stage-run. Each row already carries its own worker_provider/reviewer_provider, but
-- `auto_start_development` cloned the planning row's pair onto the development row, so one agent
-- necessarily did both stages.
--
-- These two columns live on the PLANNING (or briefing) row and describe what the DEVELOPMENT
-- stage-run should use when the coordinator hands off. NULL means "inherit the planning row's
-- providers" — the pre-existing behaviour, so initiatives created before this migration are
-- unaffected.
--
-- Intended default shape: planning = claude (plan + review), development = grok builds,
-- claude validates. The development row's reviewer_provider is what
-- `default_validation_config` uses to spawn the review lens fan-out.
ALTER TABLE agent_plans ADD COLUMN dev_worker_provider TEXT;
ALTER TABLE agent_plans ADD COLUMN dev_reviewer_provider TEXT;
