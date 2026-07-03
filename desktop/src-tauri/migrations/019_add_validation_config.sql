-- Configurable validation (overhaul phase 7). Nullable JSON array of lenses on the Initiative.
-- NULL/empty resolves to the default template (product/qa/lead) in code, so an unconfigured
-- Initiative behaves exactly as before this migration. Mirrors the findings_json JSON-in-TEXT pattern.
ALTER TABLE agent_plans ADD COLUMN validation_config TEXT;
