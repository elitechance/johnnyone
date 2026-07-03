import { Route } from '@angular/router';
import { authGuard } from './services/auth.guard';

export const appRoutes: Route[] = [
  {
    path: '',
    pathMatch: 'full',
    redirectTo: 'terminal',
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
    path: 'terminal',
    title: 'Terminal',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/terminal/terminal.page').then((m) => m.TerminalPage),
  },
  // Planner (T1=Planner, T2=Plan Reviewer) — produces methodology plans.
  // Same component renders both modes; route data tells it which.
  {
    path: 'planning',
    title: 'Planning',
    canActivate: [authGuard],
    data: { mode: 'planning' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  // Deep link to a specific selected run, e.g. /planning/<planId>.
  {
    path: 'planning/:planId',
    title: 'Planning',
    canActivate: [authGuard],
    data: { mode: 'planning' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  // Briefing (overhaul P4) — the clarification conversation before planning. `new` must precede
  // `:initiativeId` so it is not captured as an id. Accept advances the SAME initiative to planning.
  {
    path: 'briefing/new',
    title: 'New briefing',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/briefing/briefing.page').then((m) => m.BriefingPage),
  },
  {
    path: 'briefing/:initiativeId',
    title: 'Briefing',
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
  // Shells (overhaul P6) — the list of launched raw shells + attachable external tmux panes. Opening a
  // row navigates to the existing terminal surface (`/terminal?sessionId=`); no second terminal here.
  {
    path: 'shells',
    title: 'Shells',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/shells/shells.page').then((m) => m.ShellsPage),
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
  // Development (T1=Worker, T2=Reviewer) — executes approved plans.
  {
    path: 'development',
    title: 'Development',
    canActivate: [authGuard],
    data: { mode: 'development' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  {
    path: 'development/:planId',
    title: 'Development',
    canActivate: [authGuard],
    data: { mode: 'development' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
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
    redirectTo: 'terminal',
  },
];
