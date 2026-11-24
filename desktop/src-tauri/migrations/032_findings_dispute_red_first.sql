-- Findings ledger, part 1 of "J1 runs the new workflow": the dispute path and typed red-first proof.
--
-- * Two new states: `disputed` (the worker disagrees with a finding; the filing lens must rule on it
--   next round) and `withdrawn` (the lens withdrew it; terminal, never blocking). SQLite cannot alter a
--   CHECK constraint, so the table is rebuilt with the wider CHECK and every row is copied verbatim.
-- * Red-first evidence on a claim: `claim_test` (the test named as proof), `broke_what` (one line:
--   what was broken to see it fail), `red_output` (the failing output, <= 2 KB, sanitized), and
--   `red_first` (1 = the claim carries a test AND its red output, 0 = it does not, NULL = never claimed).
-- * `dispute_reason`: the worker's reason for the latest dispute.
-- The runner records the `_migrations` row after this batch, outside this transaction. A crash
-- between COMMIT and that row reruns 032; the rerun is safe only because the copy below names its
-- columns (it reads the already-rebuilt table the same way).
BEGIN;
CREATE TABLE plan_findings_new (
  plan_id        TEXT NOT NULL,
  id             TEXT NOT NULL,
  phase_id       TEXT NOT NULL,
  lens           TEXT NOT NULL,
  severity       TEXT NOT NULL CHECK (severity IN ('blocking','followup')),
  state          TEXT NOT NULL CHECK (state IN ('open','claimed','verified','reopened','waived','waive_proposed','escalated','carried','disputed','withdrawn')),
  round_opened   INTEGER NOT NULL,
  round_updated  INTEGER NOT NULL,
  claim          TEXT NOT NULL,
  expected       TEXT NOT NULL DEFAULT '',
  saw            TEXT NOT NULL DEFAULT '',
  where_         TEXT NOT NULL DEFAULT '',
  fence          TEXT NOT NULL DEFAULT '',
  evidence       TEXT,
  waive_reason   TEXT,
  reopen_count   INTEGER NOT NULL DEFAULT 0,
  last_saw       TEXT,
  created_at     TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at     TEXT NOT NULL DEFAULT (datetime('now')),
  claim_test     TEXT,
  broke_what     TEXT,
  red_output     TEXT,
  red_first      INTEGER CHECK (red_first IN (0, 1)),
  dispute_reason TEXT,
  PRIMARY KEY (plan_id, id),
  FOREIGN KEY (plan_id) REFERENCES agent_plans(id) ON DELETE CASCADE
);
INSERT INTO plan_findings_new
  (plan_id, id, phase_id, lens, severity, state, round_opened, round_updated, claim, expected, saw,
   where_, fence, evidence, waive_reason, reopen_count, last_saw, created_at, updated_at)
SELECT plan_id, id, phase_id, lens, severity, state, round_opened, round_updated, claim, expected, saw,
       where_, fence, evidence, waive_reason, reopen_count, last_saw, created_at, updated_at
  FROM plan_findings;
DROP TABLE plan_findings;
ALTER TABLE plan_findings_new RENAME TO plan_findings;
CREATE INDEX IF NOT EXISTS idx_plan_findings_phase ON plan_findings(plan_id, phase_id, state);
COMMIT;
