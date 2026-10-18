import { Route } from '@angular/router';
import { authGuard } from './services/auth.guard';
import { matchShellRoute } from './pages/shells/shells-route';

export const appRoutes: Route[] = [
  {
    path: '',
    pathMatch: 'full',
    redirectTo: 'initiatives',
  },
  {
    path: 'login',
    title: 'Sign in',
    loadComponent: () =>
      import('./pages/login/login.page').then((m) => m.LoginPage),
  },
  // Public partner / third-party API integration guide. No authGuard — this is
  // the in-app replacement for the old johnnyone-partner-api.pages.dev site.
  {
    path: 'integration',
    title: 'Integration',
    loadComponent: () =>
      import('./pages/integration/integration.page').then((m) => m.IntegrationPage),
  },
  {
    path: 'initiatives',
    title: 'Initiatives',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/terminal/terminal.page').then((m) => m.TerminalPage),
  },
  // Back-compat: the console used to live at /terminal. Redirect old links/bookmarks
  // (incl. ?initiativeId&tab query params, which survive the redirect) to /initiatives.
  {
    path: 'terminal',
    redirectTo: 'initiatives',
    pathMatch: 'full',
  },
  // Legacy /planning + /development (the split PlannerPage) were REMOVED: planning and development are
  // now stages of one Initiative, run and controlled entirely on the /initiatives console (progression
  // is automatic; Stop/Amend live inline there). Old links redirect to the console.
  { path: 'planning', redirectTo: 'initiatives', pathMatch: 'full' },
  { path: 'planning/:planId', redirectTo: 'initiatives', pathMatch: 'full' },
  { path: 'development', redirectTo: 'initiatives', pathMatch: 'full' },
  { path: 'development/:planId', redirectTo: 'initiatives', pathMatch: 'full' },
  // New-initiative create form (overhaul P4, briefing step removed). Creating provisions the
  // initiative and immediately accepts it, so the lifecycle starts at planning — there is no
  // interactive briefing conversation, hence no `briefing/:initiativeId` route.
  {
    path: 'briefing/new',
    title: 'New initiative',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/briefing/briefing.page').then((m) => m.BriefingPage),
  },
  // Files (overhaul P5) — the two-pane file manager over the host `files_root` (browse/preview here;
  // edit/CRUD + upload land in Phases 02/03). Standalone entry point; the briefing/launcher links are
  // Phase 8 (D7).
  {
    path: 'files',
    title: 'Files',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/files/files.page').then((m) => m.FilesPage),
  },
  // Shells (overhaul P6) — the list of launched raw shells + attachable external tmux panes.
  {
    path: 'shells',
    title: 'Shells',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/shells/shells.page').then((m) => m.ShellsPage),
  },
  // A single shell / attached-tmux session opens on its OWN plain-terminal destination — NOT the
  // initiative console at /terminal (a shell is not an initiative). `data.surface = 'shell'` makes the
  // terminal page render the plain surface (no lifecycle bar / tabs / validation) from first paint; the
  // session id is the `:sessionId` path segment. Must follow `shells` so `/shells` alone still lists.
  //
  // A MATCHER, not a `path`, because one config has to serve `/shells/:id`, `/shells/:id/transcript`
  // and `/shells/:id/raw`: Angular recreates the routed component when the matched config changes, so
  // sibling/child routes per view would rebuild the whole TerminalPage on every Transcript/Raw tap —
  // fresh ngOnInit, fresh `sessions()`/`terminalScreens()`/transcript buffer, re-read pane layouts,
  // full re-subscribe of the visual + stream lanes. (The xterm view is recreated either way: the
  // template swaps `johnny-terminal-screen` out for the transcript block. It is the PAGE around it
  // that the matcher preserves.) With one matcher the component is REUSED and only the `view` param
  // moves, which `TerminalPage` watches via `paramMap`. `matchShellRoute` is imported statically on
  // purpose — URL recognition runs before any lazy chunk loads, so a matcher cannot be lazy; the
  // module is small and component-free, so it costs nothing in the initial bundle. It also matches an
  // unknown third segment on purpose (the page canonicalises the URL instead of the `**` wildcard
  // bouncing the operator to the initiatives console).
  {
    matcher: matchShellRoute,
    title: 'Shell',
    canActivate: [authGuard],
    data: { surface: 'shell' },
    loadComponent: () =>
      import('./pages/terminal/terminal.page').then((m) => m.TerminalPage),
  },
  // Validation config (overhaul P7) — the §07 "Validation · configure" surface for one Initiative:
  // the ordered N-lens array (provider/model/vision/BLOCK-WARN) persisted to `validationConfig`.
  // One lazy authGuard route (mirrors /files, /shells — D12); reached from the planner run settings.
  {
    path: 'initiatives/:id/validation',
    title: 'Validation',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/validation-config/validation-config.page').then(
        (m) => m.ValidationConfigPage,
      ),
  },
  // The workflow arena (MASS). Four lazy authGuard routes, mirroring /files, /shells and
  // /initiatives/:id/validation. There is deliberately NO per-phase route: phases render inside
  // TerminalPage's `plan` tab, so the arena's entry point is a row there plus an added `arena` tab,
  // and the setup wizard hangs off the INITIATIVE, carrying the phase as `?phaseId=`.
  //
  // Each loads the `*.shell`, which wraps the page body in `ion-content`. Without that wrapper the
  // routed page is clipped to the viewport and does not scroll — measured in Chrome at 400x860,
  // with the promote/reject row 464px past the clip. See the shell files.
  //
  // `ArenaApiService` is provided by each SHELL component, not here: a `providers: [X]` on a route
  // needs `X` imported by this eagerly-loaded table, which would drag the service — and its ten
  // GraphQL documents — straight back into the initial bundle this move exists to keep them out of.
  {
    path: 'initiatives/:id/arena',
    title: 'Arena setup',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/arena/arena-setup.shell').then((m) => m.ArenaSetupShell),
  },
  {
    path: 'workflows',
    title: 'Workflows',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/arena/workflows.shell').then((m) => m.WorkflowsShell),
  },
  // The 3-segment arena route is declared BEFORE `workflows/:id` for readability; Angular matches
  // on segment count, so neither can shadow the other, and `arena-routes.spec.ts` navigates both
  // for real rather than trusting that.
  {
    path: 'workflows/:id/arena',
    title: 'Workflow arena',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/arena/workflow-arena.shell').then((m) => m.WorkflowArenaShell),
  },
  {
    path: 'workflows/:id',
    title: 'Workflow',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/arena/workflow-detail.shell').then((m) => m.WorkflowDetailShell),
  },
  // Developer console — authenticated UI for the partner API surface
  // (auth/keys, agent-session CRUD, live WSS terminal). The public docs live
  // at /integration; this is the logged-in console that drives the same API.
  {
    path: 'developer',
    title: 'Developer',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/developer/developer.page').then((m) => m.DeveloperPage),
  },
  {
    path: 'settings/prompts',
    title: 'Prompts',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/prompt-library/prompt-library.page').then(
        (m) => m.PromptLibraryPage,
      ),
  },
  {
    path: 'settings',
    title: 'Settings',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/settings/settings.page').then((m) => m.SettingsPage),
  },
  {
    path: 'install',
    title: 'Install',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/install/install.page').then((m) => m.InstallPage),
  },
  {
    path: '**',
    redirectTo: 'initiatives',
  },
];
