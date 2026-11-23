-- Findings ledger (F1): one row per review finding of a development phase. The DB is the source of
-- truth; `<plan_path>/phases/<phaseId>/findings.json` and `<plan_path>/followups.md` are mirrors.
-- Every state change goes through `services::findings::apply` (the ONE `transition` function).
-- PK is (plan_id, id) because phase ids such as `00-x` repeat across plans.
CREATE TABLE IF NOT EXISTS plan_findings (
  plan_id       TEXT NOT NULL,
  id            TEXT NOT NULL,                 -- F-<phaseId>-<n>, coordinator-assigned
  phase_id      TEXT NOT NULL,
  lens          TEXT NOT NULL,                 -- the filing lens's configured name (legacy imports keep it too)
  severity      TEXT NOT NULL CHECK (severity IN ('blocking','followup')),
  state         TEXT NOT NULL CHECK (state IN ('open','claimed','verified','reopened','waived','waive_proposed','escalated','carried')),
  round_opened  INTEGER NOT NULL,
  round_updated INTEGER NOT NULL,
  claim         TEXT NOT NULL,
  expected      TEXT NOT NULL DEFAULT '',
  saw           TEXT NOT NULL DEFAULT '',
  where_        TEXT NOT NULL DEFAULT '',
  fence         TEXT NOT NULL DEFAULT '',
  evidence      TEXT,
  waive_reason  TEXT,
  reopen_count  INTEGER NOT NULL DEFAULT 0,
  last_saw      TEXT,
  created_at    TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (plan_id, id),
  FOREIGN KEY (plan_id) REFERENCES agent_plans(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_plan_findings_phase ON plan_findings(plan_id, phase_id, state);
