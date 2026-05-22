import { Routes } from '@angular/router';
import { authGuard } from './services/auth.guard';

export const routes: Routes = [
  {
    path: '',
    redirectTo: 'terminal',
    pathMatch: 'full',
  },
  {
    path: 'terminal',
    redirectTo: 'chat',
    pathMatch: 'full',
  },
  {
    path: 'login',
    loadComponent: () =>
      import('./pages/login/login.page').then((m) => m.LoginPage),
  },
  {
    path: 'chat',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/chat/chat.page').then((m) => m.ChatPage),
  },
  {
    path: 'sessions',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/sessions/sessions.page').then((m) => m.SessionsPage),
  },
  {
    path: 'tools',
    canActivate: [authGuard],
    loadComponent: () =>
      import('./pages/tools/tools.page').then((m) => m.ToolsPage),
  },
  {
    path: 'planner',
    redirectTo: 'development',
    pathMatch: 'full',
  },
  {
    path: 'planning',
    canActivate: [authGuard],
    data: { mode: 'planning' },
    loadComponent: () =>
      import('./pages/planner/planner.page').then((m) => m.PlannerPage),
  },
  {
    path: 'development',
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
    path: '**',
    redirectTo: 'terminal',
  },
];
