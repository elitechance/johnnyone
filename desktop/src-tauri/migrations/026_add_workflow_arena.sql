-- The workflow arena: the three tables the search runs on, plus the decision log the halt is
-- computed from.
--
-- A "workflow" is the searched artifact: the four prompt parts (role / instruction / contract /
-- hop) that a phase-type is run with, versioned, with `parent_id` lineage. A champion is keyed
-- `(repo, phase_type, family)` and materialises as an ordinary plan file, so J1's execution path
-- runs unchanged — nothing in `agent_plans` or `agent_plan_phases` is touched by this migration.
--
-- Why each unusual column is here (the design doc is plans/johnnyone/mass/schema.md):
--
--   `family` on `workflows`, not on the run. A Hop tuned around one model's context window does
--   not transfer to another's. Treating the model as environment is how you promote a champion
--   that is worse everywhere it is actually used, so the model family is part of IDENTITY.
--
--   `gate_collected` next to `gate_exit`. `gate_exit` is the score and the ONLY score.
--   `gate_collected` is ADVISORY and NULLABLE: a gate that prints nothing a parser recognises (a
--   lint, a build, a bespoke script) is a perfectly legitimate gate with a perfectly well-known
--   exit code, so NULL is a normal result and not an error. Nothing may gate, compare or rank on
--   it — see `services::workflows::promotion_block`, which reads `gate_exit` alone. Its one job is
--   to let a human notice a suite that collects nothing: such a suite exits 0 and scores every
--   workflow identically, so the search would follow pure noise with nothing reporting it.
--
--   `seats_held` and `fleet_busy`. Two concurrent runs contend for the same fleet, there is no 429
--   to notice, and the slower run simply looks like the worse workflow. Without these the first
--   thing a busy fleet does is promote a champion because the other box happened to be quiet.
--
--   `comparable` on `workflow_prefs`. Makes the above ENFORCEABLE rather than advisory: a
--   preference recorded across mismatched seat budgets still exists (it is data), but it cannot
--   promote. The rule lives in `services::workflows::budgets_are_comparable` and the refusal in
--   `promotion_block` — one function each, because this repo has already been bitten by three
--   independent writers of one clause drifting apart.
--
--   NO `score` column. Deliberate, and the single most important omission here. The score is
--   `gate_exit`: zero or non-zero. A numeric score needs a scale, a writer and an interpreter, and
--   every one of those is a place for an opinion — or a false pass — to hide. No latency, no token
--   count, no model verdict.

-- the searched artifact
CREATE TABLE workflows (
  id            TEXT PRIMARY KEY,
  repo          TEXT NOT NULL,      -- a champion belongs to one repo
  phase_type    TEXT NOT NULL,      -- 'dev-phase', 'ui-phase', …
  family        TEXT NOT NULL,      -- 'glm-5.3-flash' — part of IDENTITY, not environment
  version       INTEGER NOT NULL,
  parent_id     TEXT REFERENCES workflows(id),   -- lineage
  role          TEXT NOT NULL,
  instruction   TEXT NOT NULL,
  contract      TEXT NOT NULL,
  hop           TEXT NOT NULL,      -- json: steps, parallel groups, returns
  fan_out       INTEGER NOT NULL,   -- seat cost, bounded by the fleet
  is_champion   INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT NOT NULL
);

-- EXACTLY ONE champion per (repo, phase_type, family). This is a PARTIAL index: it constrains only
-- the rows with is_champion = 1, so the many non-champion versions of a family are unaffected.
--
-- It is the last line of defence, not the first. `services::workflows::promote_workflow` demotes
-- the incumbent and promotes the challenger inside ONE transaction, in that order. Written the
-- other way round, or as two statements outside a transaction, this index rejects the promote —
-- which is the correct failure and is pinned by a test.
CREATE UNIQUE INDEX workflows_one_champion
  ON workflows(repo, phase_type, family) WHERE is_champion = 1;

-- one execution of one workflow
CREATE TABLE workflow_runs (
  id            TEXT PRIMARY KEY,
  workflow_id   TEXT NOT NULL REFERENCES workflows(id),
  plan_id       TEXT REFERENCES agent_plans(id) ON DELETE SET NULL,
  seed          INTEGER NOT NULL DEFAULT 0,      -- replay index
  gate_exit     INTEGER,            -- the score. NULL = never ran
  gate_collected INTEGER,           -- 0 collected = the gate discriminates nothing. NULL = unparseable, which is FINE
  seats_held    TEXT NOT NULL,      -- json {"glm":3,"deepseek":1}
  fleet_busy    TEXT NOT NULL,      -- json, contention at start — comparability
  started_at    TEXT NOT NULL,
  ended_at      TEXT
);

-- outcomes: also the project-level measurement
CREATE TABLE workflow_prefs (
  id            TEXT PRIMARY KEY,
  winner_run_id TEXT NOT NULL REFERENCES workflow_runs(id),
  loser_run_id  TEXT NOT NULL REFERENCES workflow_runs(id),
  comparable    INTEGER NOT NULL,   -- 0 when the seat budgets differed
  reason        TEXT,               -- free text from the optimizer's feedback
  created_at    TEXT NOT NULL
);

-- The human's decisions, in order. NOT in plans/johnnyone/mass/schema.md — see the deviation note
-- below.
--
-- Why a fourth table was needed. The arena has three human decisions — promote, reject and resume
-- — and "THREE CONSECUTIVE REJECTIONS FOR THE SAME (repo, phase_type, family) HALT THE SEARCH" is
-- a requirement about the ORDER of them. The three tables above cannot express it:
--   * `workflow_prefs` has a `reason`, but a rejection has no winner. Encoding one as a row with
--     winner_run_id = loser_run_id would corrupt the table the design calls "the project-level
--     measurement" — a self-preference poisons any pairwise ranking computed from it later.
--   * `workflows.is_champion` records THAT a version is champion, not WHEN it was promoted, so
--     there is no event to reset a streak against.
--   * `workflow_runs` has nowhere to put a free-text reason.
-- Adding a `promoted_at`/`rejected_reason` column would have meant editing the three tables the
-- design specifies verbatim, which is worse. So the decisions share one append-only log, and
-- "consecutive" means exactly "trailing, in `seq` order, counting DISTINCT runs" — see
-- `services::workflows::workflow_search_state`, the single reader of this rule.
--
-- `seq` exists because `created_at` ties. Two decisions recorded in the same second are
-- indistinguishable by timestamp, and "consecutive" is precisely an ordering question, so the
-- order is an explicit monotonic integer rather than a timestamp or an implicit rowid.
--
-- CONCURRENCY. Every arena write goes through `Database::with_conn`, which holds one
-- `std::sync::Mutex` around ONE `rusqlite::Connection` for the whole closure, so today every
-- promote, reject and resume is fully serialised and no two can interleave. The
-- `workflows_one_champion` partial index is therefore defence-in-depth rather than the thing
-- holding the invariant. If J1 ever opens a second connection to this database — a second process,
-- a pool, or a read replica promoted to writer — then the read-then-write pairs in
-- `promote_workflow` (check the halt, read the incumbent, then swap) and in
-- `workflow_search_state` become genuinely racy, and they will need `BEGIN IMMEDIATE` plus a
-- re-check inside the transaction rather than the ambient serialisation they rely on now.
CREATE TABLE workflow_decisions (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  id         TEXT NOT NULL UNIQUE,
  run_id     TEXT NOT NULL REFERENCES workflow_runs(id),
  -- 'promote' makes a workflow champion. 'reject' is a human saying no to a run, with a reason.
  -- 'resume' is a human explicitly lifting a halt, with a reason.
  --
  -- `resume` exists because a PROMOTION used to be what cleared a halt, and that made the halt
  -- escapable with no new evidence at all: re-promoting the sitting champion changed no row but
  -- appended a 'promote', and the streak counted back only to the last one. Clearing a halt is now
  -- its own explicit, human-confirmed decision — "the search is halted until a human says why it
  -- should continue" is one event, not a side effect of an unrelated operation. A 'promote' row is
  -- SKIPPED by the streak walk; only 'resume' resets it.
  kind       TEXT NOT NULL CHECK (kind IN ('promote', 'reject', 'resume')),
  -- Mandatory on a rejection AND on a resume: "saying no" with no reason recorded teaches the
  -- search nothing, and lifting a halt without one is exactly the evidence-free escape `resume` was
  -- introduced to prevent. Enforced here as well as in `reject_workflow_run` / `resume_workflow_search`
  -- so the table cannot be made useless by a future second writer.
  --
  -- `trim(reason)` alone was not enough, and that was a real hole: a reason of U+200B (zero-width
  -- space), U+FEFF (BOM) or NUL passed every blank check, because none of them is Unicode
  -- White_Space — so a rejection whose reason renders as NOTHING was accepted and stored, and that
  -- reason is the one thing a halted search has to show a human. The trim set below names the
  -- characters SQLite can be told about (space, tab, LF, CR, NUL, the zero-width family, BOM, soft
  -- hyphen); SQL cannot express Unicode visibility in general, so this catches the common cases and
  -- `services::workflows::has_visible_content` is the real guard.
  --
  -- The length bound is new too: nothing at any layer bounded a reason, so one paste could put a
  -- megabyte in the decision log and then serve it on every read of that family's search state.
  reason     TEXT CHECK (
               kind = 'promote' OR (
                 reason IS NOT NULL
                 -- Measured, not assumed: SQLite's `trim(X,Y)` does NOT remove the multi-byte
                 -- members of Y (a reason of U+200B survived it), so the zero-width family and the
                 -- BOM are stripped with nested `replace()`, which is UTF-8 safe, and only the
                 -- single-byte whitespace is left to `trim`.
                 -- NUL is stripped by replace(), NOT named in the trim set: a `char(..., 0, ...)`
                 -- argument list TRUNCATES at the zero, which silently re-opened tab and newline
                 -- when NUL was moved earlier in the list. Measured, twice.
                 AND length(trim(replace(replace(replace(replace(replace(replace(replace(replace(reason, char(0), ''), char(8203), ''), char(8204), ''), char(8205), ''), char(8206), ''), char(8207), ''), char(65279), ''), char(173), ''),
                                 char(32, 127, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31))) > 0
                 AND length(reason) <= 2000
               )
             ),
  created_at TEXT NOT NULL
);
CREATE INDEX workflow_decisions_run ON workflow_decisions(run_id);
