-- 0005_action_links.sql
-- Discord action links (worker/lib/action-links/). A link is a 120 s HS256 JWT; these tables
-- hold no token, ever — only its jti.

-- Pruned opportunistically on each mint: mints + redemptions a day past exp, events after 30 days.

-- Single-use gate. The PRIMARY KEY on jti IS the guarantee: runActionLink inserts the row
-- BEFORE it acts (`INSERT ... ON CONFLICT(jti) DO NOTHING`, then requires changes = 1), so of any
-- number of concurrent redeems of one link exactly one proceeds. A row is kept after a failed
-- relay (status 'failed'): the desktop may still have acted, so the link stays used up.
CREATE TABLE IF NOT EXISTS action_link_redemptions (
  jti TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  act TEXT NOT NULL,
  exp INTEGER NOT NULL,
  -- claimed: in flight · done · failed: the desktop refused · uncertain: the relay timed out (it
  -- may have acted) · superseded: a sibling in the same group was redeemed first · answered: the
  -- agent's question was already answered elsewhere, nothing was sent
  status TEXT NOT NULL DEFAULT 'claimed' CHECK (status IN ('claimed', 'done', 'failed', 'uncertain', 'superseded', 'answered')),
  -- Nonce of the request that claimed this row; gates the sibling supersede to the winning claim.
  claim_id TEXT,
  redeemed_at TEXT NOT NULL DEFAULT (datetime('now')),
  finished_at TEXT
);

-- Audit: every mint, preview, redeem and rejection. tenant_id is NULL for a token whose
-- signature did not verify (its claims are untrusted).
CREATE TABLE IF NOT EXISTS action_link_events (
  id TEXT PRIMARY KEY,
  jti TEXT,
  tenant_id TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('mint', 'preview', 'redeem', 'reject')),
  act TEXT,
  reason TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_action_link_events_tenant ON action_link_events(tenant_id, created_at);
CREATE INDEX IF NOT EXISTS idx_action_link_events_jti ON action_link_events(jti);

-- What each link does. A Discord link button's URL is capped at 512 characters, which a JWT
-- carrying args + label does not fit, so the token holds only the link's identity and the worker
-- keeps its args + label here under the jti. Written only by createActionLink; preview and run
-- require it to match the token's tenant, user, act and exp. Rows are useless after exp.
CREATE TABLE IF NOT EXISTS action_link_mints (
  jti TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  act TEXT NOT NULL,
  args_json TEXT NOT NULL,
  label TEXT NOT NULL,
  exp INTEGER NOT NULL,
  -- Optional sibling group (a Yes/No pair): redeeming one link supersedes the others.
  group_id TEXT,
  -- Human target from the desktop (plan / agent titles), shown on /act instead of ids.
  target_title TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_action_link_mints_group ON action_link_mints(tenant_id, user_id, group_id);
CREATE INDEX IF NOT EXISTS idx_action_link_mints_exp ON action_link_mints(exp);
