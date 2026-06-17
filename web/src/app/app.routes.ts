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
    loadComponent: () =>
      import('./pages/login/login.page').then((m) => m.LoginPage),
  },
  {
    path: 'terminal',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/terminal/terminal.page').then((m) => m.TerminalPage),
  },
  // Planner (T1=Planner, T2=Plan Reviewer) — produces methodology plans.
  // Same component renders both modes; route data tells it which.
  {
    path: 'planning',
    canActivate: [authGuard],
    data: { mode: 'planning' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  // Deep link to a specific selected run, e.g. /planning/<planId>.
  {
    path: 'planning/:planId',
    canActivate: [authGuard],
    data: { mode: 'planning' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  // Development (T1=Worker, T2=Reviewer) — executes approved plans.
  {
    path: 'development',
    canActivate: [authGuard],
    data: { mode: 'development' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  {
    path: 'development/:planId',
    canActivate: [authGuard],
    data: { mode: 'development' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  {
    path: 'settings',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/settings/settings.page').then((m) => m.SettingsPage),
  },
  {
    path: 'install',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/install/install.page').then((m) => m.InstallPage),
  },
  {
    path: '**',
    redirectTo: 'terminal',
  },
];
