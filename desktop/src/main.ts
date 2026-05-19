import { provideZoneChangeDetection } from '@angular/core';
import { bootstrapApplication } from '@angular/platform-browser';
import { provideRouter } from '@angular/router';
import { provideIonicAngular } from '@ionic/angular/standalone';
import {
  GRAPHQL_API_URL,
  GRAPHQL_EXTRA_HEADERS,
  GRAPHQL_WS_URL,
} from '@johnnyone/ui';
import { AppComponent } from './app/app.component';
import { routes } from './app/app.routes';

function forceDarkMode(): void {
  const root = document.documentElement;
  root.classList.add('dark', 'ion-palette-dark');
  root.style.setProperty('color-scheme', 'dark');

  const applyBodyClass = () => document.body.classList.add('dark', 'ion-palette-dark');
  if (document.body) {
    applyBodyClass();
  } else {
    document.addEventListener('DOMContentLoaded', applyBodyClass, { once: true });
  }
}

forceDarkMode();

function getStoredSetting(key: string, fallback: string): string {
  const stored = typeof window !== 'undefined'
    ? window.localStorage.getItem(key)
    : null;

  return (stored && stored.trim()) || fallback;
}

function getWorkerBaseUrl(): string {
  const localFallback = 'http://127.0.0.1:7714';
  let fallback = localFallback;

  if (typeof window !== 'undefined' && !['localhost', '127.0.0.1'].includes(window.location.hostname)) {
    fallback = window.location.hostname.endsWith('.pages.dev')
      ? 'https://johnnyone-dev-johnnyone-hub.cf-static-5f5.workers.dev'
      : window.location.origin;
  }

  return getStoredSetting('johnnyone_worker_url', fallback);
}

function getWorkerGraphqlUrl(): string {
  return `${getWorkerBaseUrl().replace(/\/$/, '')}/graphql`;
}

function getWorkerGraphqlWsUrl(): string {
  const httpUrl = getWorkerGraphqlUrl();
  if (httpUrl.startsWith('https://')) {
    return httpUrl.replace('https://', 'wss://');
  }
  if (httpUrl.startsWith('http://')) {
    return httpUrl.replace('http://', 'ws://');
  }
  return httpUrl;
}

bootstrapApplication(AppComponent, {
  providers: [
    provideZoneChangeDetection({ eventCoalescing: true }),
    provideRouter(routes),
    provideIonicAngular({ mode: 'md' }),
    {
      provide: GRAPHQL_API_URL,
      useValue: getWorkerGraphqlUrl(),
    },
    {
      provide: GRAPHQL_WS_URL,
      useValue: getWorkerGraphqlWsUrl(),
    },
    {
      provide: GRAPHQL_EXTRA_HEADERS,
      useValue: {
        'x-tenant-id': getStoredSetting('johnnyone_tenant_id', '00000000-0000-0000-0000-000000000001'),
        'x-user-id': getStoredSetting('johnnyone_user_id', '00000000-0000-0000-0000-000000000002'),
      },
    },
  ],
});
