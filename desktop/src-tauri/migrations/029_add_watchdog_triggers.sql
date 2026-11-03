-- The console's "Also nudge when" section renders two checkboxes — "the pane is blocked on a
-- question" and "nothing has been reported to the chat for twice the interval" — that only flipped
-- a local signal. Nothing persisted them, so they read false on every page load and the sweep never
-- consulted them; the first label's promise ("fires at once, ignores the interval") was kept by no
-- code. Two flags so the sweep can honour them and the console can hydrate what it shows.
ALTER TABLE sessions ADD COLUMN watchdog_on_blocked      INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN watchdog_on_stale_report INTEGER NOT NULL DEFAULT 0;