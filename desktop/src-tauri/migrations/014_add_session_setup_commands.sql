-- 014_add_session_setup_commands.sql
-- For `shell` sessions, commands run in the pane on first spawn (e.g. cd into
-- the app + launch an agent CLI). NULL/empty for all other sessions.
ALTER TABLE sessions ADD COLUMN setup_commands TEXT;
