-- Overhaul P4: link a briefing Initiative to its conversation (a kind='user' chat session).
-- Nullable; no backfill — legacy rows are never in initiative_status='briefing'.
ALTER TABLE agent_plans ADD COLUMN briefing_session_id TEXT;
