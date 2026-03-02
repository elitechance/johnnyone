import { Routes } from '@angular/router';

export const routes: Routes = [
  {
    path: '',
    redirectTo: 'chat',
    pathMatch: 'full',
  },
  {
    path: 'chat',
    loadComponent: () =>
      import('./pages/chat/chat.page').then((m) => m.ChatPage),
  },
  {
    path: 'sessions',
    loadComponent: () =>
      import('./pages/sessions/sessions.page').then((m) => m.SessionsPage),
  },
  {
    path: 'tools',
    loadComponent: () =>
      import('./pages/tools/tools.page').then((m) => m.ToolsPage),
  },
  {
    path: 'settings',
    loadComponent: () =>
      import('./pages/settings/settings.page').then((m) => m.SettingsPage),
  },
  {
    path: '**',
    redirectTo: 'chat',
  },
];
