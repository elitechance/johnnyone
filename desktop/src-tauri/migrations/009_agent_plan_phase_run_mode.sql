-- Stores how a development start should advance after the selected phase.
-- 'continue' preserves the existing behavior; 'single' stops after the
-- currently selected phase passes review.

ALTER TABLE agent_plans ADD COLUMN phase_run_mode TEXT NOT NULL DEFAULT 'continue';
