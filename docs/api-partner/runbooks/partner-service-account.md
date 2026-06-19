# Runbook — Partner Service-Account Provisioning (Hosted Desktop)

**Audience:** operators who onboard or rotate API partners under the hosted-service-account model.
**Scope:** create dedicated `tenant_id` + `user_id`, run one `johnnyone-desktop` bound to that identity, issue creds out-of-band, hand the partner the base URL + docs site, rotate/tear down.
**Linked from:** Phase 06 overview, master `overview.md` §9 (ops row).

---

## 1. Identity creation (one dedicated `tenant_id` + `user_id` per partner)

Each partner gets its own tenant + user. Never share across partners (isolation for WSS node resolution and session ownership).

- Tenants live in D1 (`worker/d1/migrations/0000_initial.sql` `tenants` table).
- Users are managed by the builtin `auth` module (`users`/`sessions`).
- Use `lokal cf db ... --env <env>` (explicit env; never bare `wrangler d1 execute` for deploy paths — see `conventions/tooling.md`).

Commands (example for `dev`; replace UUIDs with fresh per-partner values):

```bash
# Ensure tenant row (idempotent)
lokal cf db query --env dev --sql "
INSERT OR IGNORE INTO tenants (tenant_id, preferred_tenant_id, display_name, is_active, created_at, updated_at, plan, settings)
VALUES ('<PARTNER_TENANT_ID>', '<PARTNER_TENANT_ID>', 'Partner <NAME>', 1, datetime('now'), datetime('now'), 'pro', '{}');
"
```

# Create the user via the builtin auth module (recommended): use the worker's GraphQL adminCreateUser mutation (or host ops equivalent) to create the user under the tenant with an initial password. Example using the worker GraphQL (with appropriate admin context):
# mutation { adminCreateUser(input: {tenantId: "<PARTNER_TENANT_ID>", email: "partner-<NAME>@example.com", password: "initial-pass", roles: ["USER"]}) { id } }
# Do not use raw D1 INSERT for users (the users table is managed by the auth module).

Record the pair:
- `JOHNNYONE_TENANT_ID=<PARTNER_TENANT_ID>`
- `JOHNNYONE_USER_ID=<PARTNER_USER_ID>`

The committed `lokal.yaml` values (`000...001` / `000...002`) are LOCAL dev defaults only — each partner gets fresh UUIDs.

## 2. Run the hosted desktop

Start exactly one `johnnyone-desktop` bound to the partner's identity. The desktop registers its node; the worker resolves nodes **server-side from the JWT** (`SELECT id FROM desktop_nodes WHERE tenant_id=? AND user_id=? AND status='online' AND is_deleted = 0`).

If the desktop is not online for that identity, partners get `503` on WSS upgrade (design-doc acceptance #2).

- Desktop binary build (see `conventions/README.md §Validation And Deploy` — do **not** use plain `cargo build --release` without `--features tauri/custom-protocol` or the window will be blank).

Example env for the hosted process (per `lokal.yaml local.processes.desktop.env` contract; adapt for your VM/systemd/docker):

```bash
JOHNNYONE_TENANT_ID=<PARTNER_TENANT_ID>
JOHNNYONE_USER_ID=<PARTNER_USER_ID>
JOHNNYONE_HOST_ADDR=127.0.0.1:7788
JOHNNYONE_WORKER_URL=https://johnnyone.<account>.workers.dev
# For the hosted desktop on VM, point at the real deployed worker.
```

Start the binary (example):

```bash
# On the VM (headless or with display as needed)
# For release/prod-like (must include feature or window blank):
cargo build --release --bin johnnyone-desktop --features tauri/custom-protocol
./target/release/johnnyone-desktop
# or use helper: ./scripts/run-desktop.sh
# (see conventions/README.md and docs/operations.md for full build notes)
```

Confirm the desktop registers (via logs or `desktop_nodes` row with `status='online'` for the partner's tenant+user).

The WSS clientType for partners is `api` (or `mobile` for compat); the desktop must stay connected for the partner to have a live terminal.

## 3. Issue credentials out-of-band

The partner's primary credential is a JWT obtained by calling the builtin auth `login` mutation (after the user has a password set at creation time via adminCreateUser or equivalent) against their `tenant_id` + `user_id`.

- Optional M2M: issue a scoped `jk_*` API key via `createApiKey` (JWT-only; scopes e.g. `terminal:read`, `plans:write`); the secret is returned once and used as the token on GraphQL/WSS.
- **Credentials are generated and delivered through secret storage / out-of-band channel only.**
- **Literal token values, refresh tokens, or `JWT_SECRET` are never written into this runbook, the plan, or any committed file.**

In the hand-off to the partner, provide:
- The JWT (and refresh if applicable)
- Optional API key
- The base URL and docs site (see §4)

Reference `lokal.yaml secrets: [JWT_SECRET]` as the signing secret (never the value).

## 4. Base URL + docs site

The partner **only ever hits the deployed worker**. Never `lokal sim` for production integration.

- Worker base: `https://johnnyone-dev.ethan-353.workers.dev` (dev for live validation; prod uses non-dev).
- GraphQL: `POST /graphql` with `Authorization: Bearer <jwt>` (and `x-tenant-id` if required by the flow).
- Live terminal: WSS `wss://.../api/relay/ws` with `Authorization: Bearer <jwt>` on upgrade (or `?token=<jwt>` fallback for clients that cannot set headers). Use `clientType=api` or omit.
- Public self-serve entry point: the **partner-docs-site URL** deployed in Phase 04 Task 06: https://johnnyone-partner-api.pages.dev (content points to johnnyone.ethan-353.workers.dev ; for dev live validation use https://johnnyone-dev.ethan-353.workers.dev ). The site contains the integration guide, example clients, and the reconciled GraphQL/WSS surface.

Give the partner exactly these two URLs + their credential.

## 5. Rotation & teardown

**Rotation (compromised credential):**
- Re-issue JWT/refresh (or new API key) for the same `tenant_id`+`user_id`.
- Revoke the old API key (if any) via `revokeApiKey`.
- Update the partner out-of-band; no code change required.
- The desktop continues running; the partner just uses the new token on next connect.

**Full teardown (partner offboarded):**
1. Stop the partner's hosted `johnnyone-desktop` (or mark its node offline).
2. Update `desktop_nodes` for that `tenant_id`+`user_id` to `status='offline'`.
3. Disable or delete the user (builtin auth `users`).
4. (Optional) retire the tenant row or mark inactive.
5. Revoke any API keys.

Blast-radius note: one identity per partner means revoking or tearing down one partner cannot affect any other partner.

## Operator checklist (copy-paste)

- [ ] Fresh UUIDs minted for this partner (never reuse the committed local defaults).
- [ ] Tenant + user rows present in target env D1.
- [ ] Hosted desktop running and `status='online'` for the partner's tenant+user.
- [ ] Credential(s) delivered out-of-band only (JWT + optional key).
- [ ] Partner has: base URL (https://johnnyone-dev.ethan-353.workers.dev for this live), `/graphql`, WSS endpoint, **https://johnnyone-partner-api.pages.dev**, and credential.
- [ ] Rotation/teardown steps tested for this identity.
- [ ] All commands used `lokal ... --env <env>` (no bare wrangler/raw-D1).

## References

- Design doc: `api-partner-authenticated-wss.md` §Deployment/ops + acceptance #2.
- Master: `overview.md` §4.4, §9, §10 (server-side node resolution).
- Conventions: `tooling.md`, `README.md` §Validation And Deploy, `plan-validation.md` §State Honesty.
- `lokal.yaml` (env contract + secrets).
- Sibling: `runbooks/api-versioning.md`.

---

**No literal secrets appear in this file.** Placeholders only. All commands carry explicit `--env`. 

(End of runbook)
