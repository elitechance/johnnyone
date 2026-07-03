-- Backfill for migration 017. Idempotent (all guards are `IS NULL`), so it is safe to run
-- once from the migration and again from the unit test against manually-inserted legacy rows.

-- Each legacy plan becomes its own single-stage Initiative.
UPDATE agent_plans SET initiative_id = id WHERE initiative_id IS NULL;

-- Lifecycle stage from run_type.
UPDATE agent_plans SET initiative_status = 'planning'
  WHERE initiative_status IS NULL AND run_type = 'planning';
UPDATE agent_plans SET initiative_status = 'development'
  WHERE initiative_status IS NULL AND run_type IS NOT NULL AND run_type <> 'planning';

-- Defensive sweep (T2 Lead finding): SQL three-valued logic means `run_type <> 'planning'`
-- never matches a NULL run_type, which would leave initiative_status NULL and violate the
-- non-null String! contract (row.get would then error and break list/get). This final catch-all
-- closes that hole for any row still NULL (including a NULL run_type).
UPDATE agent_plans SET initiative_status = 'development' WHERE initiative_status IS NULL;

-- Seed health from the current execution state (must agree with health_from_status()).
UPDATE agent_plans SET health = 'blocked'         WHERE status = 'blocked';
UPDATE agent_plans SET health = 'needs-attention' WHERE status IN ('needs_attention', 'phase_needs_changes');
