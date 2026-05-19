ALTER TABLE sessions ADD COLUMN tmux_session_name TEXT;
ALTER TABLE sessions ADD COLUMN tmux_pane_id TEXT;
ALTER TABLE sessions ADD COLUMN tmux_screen_cursor INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN terminal_status TEXT NOT NULL DEFAULT 'detached';
