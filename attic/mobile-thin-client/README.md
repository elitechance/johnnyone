# mobile-thin-client (archived)

**Archived:** 2026-05-23
**Reason:** Mobile thin client deferred — focus narrowed to the new hosted `web/` client for v1 of the multi-user SaaS pivot. The mobile surface will return reusing `ui/` components once the web client is stable and a multi-user host registration flow is shipped.
**Plan:** [`personal/docs/johnnyone/plans/multi-user-saas/`](../../../../docs/johnnyone/plans/multi-user-saas/)

## What's here

The full Ionic/Angular + Capacitor mobile project that used to live at `personal/apps/johnnyone/mobile/`:

- `src/` — Angular app (chat / sessions / settings pages built for mobile form factor)
- `capacitor.config.ts` — Capacitor Android config
- `resources/` — Android-specific resources
- `project.json`, `package.json`, `tsconfig.json`, `tsconfig.app.json`, `proxy.conf.json`

## How it's hidden from the active project graph

The workspace-level `.nxignore` (added 2026-05-23) excludes `attic/`, so this `project.json` is not auto-discovered by Nx. `nx show projects` returns only `desktop`, `shared`, `worker`, `ui` — `mobile` no longer appears.

## Reviving

When the mobile thin client is reintroduced, do **not** lift this directory wholesale. Generate a fresh Nx project (`nx g @nx/angular:app mobile --routing` + Ionic + Capacitor schematics), compose pages from `ui/` components, and reference this directory only for historical context (page shapes, Capacitor config patterns).
