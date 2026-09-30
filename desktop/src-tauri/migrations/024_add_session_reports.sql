-- Durable history for `reportAgentResult`.
--
-- Reports were only ever held in memory (`AppState.agent_reports`), one slot per session, and
-- pushed live as a StreamEvent. Nothing persisted, so a page reload showed an EMPTY transcript
-- even for a session that had reported all day — the console could only ever display what
-- happened to arrive while it was open. The pane-tail seeding added earlier was a workaround for
-- exactly this gap, not a fix.
--
-- Persisting each report makes the transcript hydratable: open a shell, read back what was
-- reported, then let the live lane append. It also gives the coordinator's control signals
-- ("ready"/"verdict"/"done") an audit trail they never had.
--
-- `markdown` holds the body the agent wrote to the file named in `evidence`, snapshotted at
-- report time. That file lives in the agent's workspace and may be rewritten or deleted, so the
-- transcript must not depend on it still being readable later.
CREATE TABLE IF NOT EXISTS session_reports (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL,
    kind          TEXT NOT NULL,
    role          TEXT,
    summary       TEXT,
    markdown      TEXT,
    verdict       TEXT,
    findings      TEXT,
    severity      TEXT,
    reason        TEXT,
    evidence      TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

-- The only read pattern: newest-first for one session.
CREATE INDEX IF NOT EXISTS idx_session_reports_session
    ON session_reports (session_id, created_at DESC);
