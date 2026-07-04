-- Backfill for the lifecycle-completion fix. Idempotent (guards on status/run_type), so it is
-- safe to run once from the migration and again from a unit test.
--
-- Before the fix, `pass_phase` set status='approved' on the final phase but never advanced
-- `initiative_status`, and `health_from_status` mapped approved -> 'in-progress'. A fully
-- finished development run therefore stayed stuck reading "development / in-progress" forever.
-- Correct the already-approved rows so the console reflects a completed initiative.

-- A development run whose execution status is 'approved' is complete -> advance to 'done'.
UPDATE agent_plans SET initiative_status = 'done', updated_at = datetime('now')
  WHERE run_type = 'development' AND status = 'approved' AND initiative_status <> 'done';

-- Health axis now recognizes terminal completion (must agree with health_from_status()).
UPDATE agent_plans SET health = 'complete', updated_at = datetime('now')
  WHERE status IN ('approved', 'done', 'complete', 'completed', 'passed') AND health <> 'complete';
