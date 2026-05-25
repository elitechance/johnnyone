-- 008_session_kind.sql
-- Tags each session with what created it so the /terminal UI doesn't show
-- planner/development T1/T2 sessions in its tab strip.
--
--   'user'   — created from /terminal (the user's manual sessions)
--   'agent'  — created by the planner / development coordinator
--              (worker + reviewer for each plan)
--
-- `list_sessions` defaults to `kind = 'user'` so the /terminal UI stays
-- clean. The planner reads agent sessions directly via agent_plans.worker_session_id
-- and reviewer_session_id — it never calls list_sessions.

ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'user';

-- Backfill: any existing planner/development sessions already created have
-- titles starting with 'T1 ' or 'T2 '. Mark those as 'agent' so the user
-- sees a clean /terminal list immediately after this migration.
UPDATE sessions SET kind = 'agent' WHERE title LIKE 'T1 %' OR title LIKE 'T2 %';
