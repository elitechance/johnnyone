import { Route } from '@angular/router';

export const appRoutes: Route[] = [
  {
    path: '',
    pathMatch: 'full',
    redirectTo: 'status',
  },
  {
    path: 'login',
    loadComponent: () =>
      import('./pages/login/login.page').then((m) => m.LoginPage),
  },
  {
    path: 'status',
    loadComponent: () =>
      import('./pages/status/status.page').then((m) => m.StatusPage),
  },
  {
    path: 'providers',
    loadComponent: () =>
      import('./pages/providers/providers.page').then((m) => m.ProvidersPage),
  },
  {
    path: '**',
    redirectTo: 'status',
  },
];
