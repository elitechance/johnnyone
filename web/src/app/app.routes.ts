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
