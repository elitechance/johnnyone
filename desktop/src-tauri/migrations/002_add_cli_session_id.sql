-- Add cli_session_id column to sessions table for CLI tool session continuity (--resume)
-- This is safe to run even if the column already exists (from fresh installs with 001_initial.sql)
ALTER TABLE sessions ADD COLUMN cli_session_id TEXT;
