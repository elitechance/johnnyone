-- The sweep can decide a nudge is due and then WITHHOLD it because the two-capture idle probe saw
-- the pane still changing. Nothing recorded that, so "being held back" was indistinguishable from
-- "quiet, nothing to say" — the same silence that hid the original watchdog bug.
ALTER TABLE sessions ADD COLUMN watchdog_held_count INTEGER NOT NULL DEFAULT 0;