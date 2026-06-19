# Runbook — Public API Versioning Policy

**Audience:** maintainers of the JohnnyOne public GraphQL API and the third parties consuming it.
**Scope:** how the public API surface (terminals / plans / sessions, error signals)
evolves without breaking integrations.
**Source convention:** `common/conventions/backend.md` §"Schema fields ARE breaking-change boundaries".
**Linked from:** master `overview.md` §4 (Public API surface).

---

## 1. The endpoint is unversioned-but-additive

There is **no `/v2` path** and there will not be one for routine evolution. The public API is the
same GraphQL endpoint the first-party client uses; clients send queries that name exactly the fields
they need, so adding types, fields, queries, mutations, scopes, or enum values never disturbs an
existing query. Versioning a GraphQL endpoint by URL path is an anti-pattern here — it would fork the
schema, double the resolver surface, and split `lokal cf worker validate` parity across two contracts.

The contract a third party depends on is therefore **the set of fields their queries select**, not a
version number. We keep that contract stable by treating field removal/rename as the only breaking
operation, and gating it behind a removal sweep (§3).

## 2. Additive changes are always safe — ship them freely

The following require no notice period and no deprecation cycle:

- New type, query, or mutation.
- New **nullable** field on an existing type.
- New optional argument (with a default) on an existing query/mutation.
- New `enum` value (consumers must already tolerate unknown values; see §5).
- New scope, or a new operation gated behind an existing scope.
- New error `extensions` metadata that doesn't change existing `code` values.

Adding a **non-nullable** field to an *input* type is breaking (old callers omit it) — treat it like
§3. Adding a non-nullable field to an *output* type is safe.

## 3. Removing or renaming a field is breaking — use the removal sweep

Per `common/conventions/backend.md` §"Schema fields ARE breaking-change boundaries", you may never
delete-and-rename a publicly-consumed field in one commit. The sequence is **add → migrate callers →
remove**, spread across separate phases (the methodology's removal-sweep pattern):

1. **Add** the replacement field/type alongside the old one (additive — safe per §2). Mark the old
   field deprecated (§4).
2. **Migrate callers** — update first-party UI and shared client (`ui/src/services/johnny-api.service.ts`),
   and give third parties the deprecation window to move (§4). Watch `api_request_log.operation` to
   confirm traffic has drained off the deprecated field.
3. **Remove** the old field in a *separate later phase*, only after the deprecation window has elapsed
   and usage has dropped to zero. The removal lands as its own migration/schema change with its own
   `lokal cf worker validate` gate.

Renaming is the same dance: add the new name, migrate, remove the old name. Never collapse the three
steps; `lokal cf worker validate` plus first-party codegen catch self-breakage, but third-party
breakage is invisible to our build, so the human window in step 2 is mandatory.

## 4. Deprecation labeling

- Use GraphQL's native `@deprecated(reason: "...")` directive on the field or enum value. The reason
  names the replacement and the earliest removal phase, e.g.
  `@deprecated(reason: "Use refreshToken(input: RefreshTokenInput!); the old refresh(refreshToken: String!) removed no earlier than Phase 06")`.
- Deprecated fields keep working for the entire window — deprecation is a *label*, not a behavior change.
- The integration guide (the in-app `/integration` page, `web/src/app/pages/integration/`; design copy
  in `docs/api-partner/index.html`) renders deprecated fields with a visible "Deprecated" marker and the
  migration target, so a developer reading the docs sees it without introspecting the schema.

## 5. Guidance we publish to consumers (so additive stays safe)

Third parties are told, on the partner docs site, to build forward-compatibly:

- **Select only the fields you use** — don't depend on field ordering or on fields you don't read.
- **Tolerate unknown enum values and unknown response fields** — new ones will appear additively.
- **Don't hard-code error message strings**; branch on the stable signals the API emits:
  GraphQL `errors[].message`, WSS upgrade HTTP status (401/503), and `terminal_command_ack.error`
  (e.g. `'forbidden_session'` or `'FORBIDDEN_SCOPE'`). These are the real contract.
  `RATE_LIMITED` is **reserved** in the contract (for a later hardening pass per
  overview §2.1) but is not emitted in v1. The current real op is `refreshToken(input: RefreshTokenInput!)`.

## 6. How third parties are notified

1. **Guide changelog** — the in-app integration guide (`/integration`,
   `web/src/app/pages/integration/`; design copy `docs/api-partner/index.html`) carries a dated
   changelog section; additive changes and new deprecations are appended there at ship time, then
   shipped with the web app (`npm run deploy:web`).
2. **`@deprecated` reason in the schema** — visible via introspection and rendered on the docs site,
   stating the replacement and the earliest removal phase.
3. **Deprecation window before any removal** — a removal (step 3 of §3) is announced when its
   deprecation lands (step 1) and is not executed until the window passes. For breaking removals the
   window is published in the changelog with a target date; until then the old field keeps responding.

## 7. Checklist before shipping a schema change to the public surface

- [ ] Change is additive per §2 — or, if it removes/renames a consumed field, it is **step 1 or step 3**
      of a removal sweep (§3), never both at once.
- [ ] Removed/renamed fields were `@deprecated` with a reason for a full window first (§4).
- [ ] `lokal cf worker validate` passes (schema ↔ resolver parity).
- [ ] Integration guide (`/integration` page + `docs/api-partner/index.html` design copy) + changelog
      updated and shipped (`npm run deploy:web`); deprecated fields marked.
- [ ] No new `/vN` path was introduced.
