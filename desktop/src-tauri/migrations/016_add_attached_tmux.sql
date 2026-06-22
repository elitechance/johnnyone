-- Sessions that attach to an existing EXTERNAL tmux session (e.g. "kloo",
-- "llm-app") rather than spawning a johnnyone_<id> pane. When 1, the stored
-- tmux_session_name is the external session and archive must NOT kill it.
ALTER TABLE sessions ADD COLUMN attached_tmux INTEGER NOT NULL DEFAULT 0;
