-- 007_add_amend_brief.sql
-- Adds an `amend_brief` column to `agent_plans`. Set to the user's amendment
-- text when they trigger "Amend" on a planning run; cleared (NULL) when the
-- amend cycle completes (T2 PASS). Used by the planner prompt templates to
-- switch T1/T2 into "edit mode" — applying changes against an existing plan
-- rather than creating one from scratch.

ALTER TABLE agent_plans ADD COLUMN amend_brief TEXT;
