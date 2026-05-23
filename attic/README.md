# Attic

Archived projects with preserved git history. **Not built, not deployed, not part of the active Nx project graph.**

Contents kept here so historical work remains discoverable (and revivable) when a future plan needs to reintroduce something — for example, when the mobile or standalone-desktop thin clients are brought back, the reusable parts are lifted out of `attic/` and composed against the current `ui/` library.

| Directory | Archived | Reason | Reference |
|---|---|---|---|
| `desktop-thin-client/` | 2026-05-23 | Web app replaced standalone desktop frontend; Tauri shell + host backend continue to live at `../desktop/src-tauri/` | [`personal/docs/johnnyone/plans/multi-user-saas/`](../../../docs/johnnyone/plans/multi-user-saas/) |
| `mobile-thin-client/` | 2026-05-23 | Mobile thin client deferred; will return reusing `ui/` components | Same plan |

## Rules

- **Never edit attic contents.** They're frozen in place. If you need to revive something, copy out into the active tree.
- **Never delete attic contents.** Deletion would lose the git-history reference that's the whole point of archiving.
- **Adding a new archive**: `git mv <dir> attic/<name>/` plus a row in this table + a `attic/<name>/README.md` documenting the archive date and rationale.
