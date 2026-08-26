-- Local-small mode (initiative 2). Nullable JSON on the Initiative.
-- NULL/empty = commercial. Phase 02 reads this to mode-gate plan-check (D4).
-- Phase 03 owns GraphQL create/update; this column is the storage.
ALTER TABLE agent_plans ADD COLUMN executor_config TEXT;
