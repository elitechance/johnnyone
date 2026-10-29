-- Add watchdog columns to sessions table for per-session configurable nudges.
ALTER TABLE sessions ADD COLUMN watchdog_enabled       INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN watchdog_interval_secs INTEGER NOT NULL DEFAULT 1200;
ALTER TABLE sessions ADD COLUMN watchdog_prompt        TEXT    NOT NULL DEFAULT '';
ALTER TABLE sessions ADD COLUMN watchdog_last_nudge_at TEXT;
ALTER TABLE sessions ADD COLUMN watchdog_nudge_count   INTEGER NOT NULL DEFAULT 0;