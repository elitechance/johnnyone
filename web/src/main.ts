import { bootstrapApplication } from '@angular/platform-browser';
import { GRAPHQL_API_URL, GRAPHQL_WS_URL } from '@johnnyone/ui';
import { appConfig } from './app/app.config';
import { AppComponent } from './app/app.component';

/**
 * Env-detected worker URL.
 *
 * Order of precedence:
 *  1. localStorage `johnnyone_worker_url` (forward-compat for a future settings UX)
 *  2. Local dev fallback `http://127.0.0.1:7714` when host is localhost/127.0.0.1
 *  3. Deployed worker URL when host ends in `.pages.dev`
 *  4. Same origin otherwise (single-domain deploys)
 */
function getStoredSetting(key: string, fallback: string): string {
  const stored =
    typeof window !== 'undefined' && window.localStorage
      ? window.localStorage.getItem(key)
      : null;
  return (stored && stored.trim()) || fallback;
}

function getWorkerBaseUrl(): string {
  const localFallback = 'http://127.0.0.1:7714';
  let fallback = localFallback;

  if (
    typeof window !== 'undefined' &&
    !['localhost', '127.0.0.1'].includes(window.location.hostname)
  ) {
    fallback = window.location.hostname.endsWith('.pages.dev')
      ? 'https://johnnyone-dev-hub.ethan-353.workers.dev'
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
  ...appConfig,
  providers: [
    ...(appConfig.providers ?? []),
    { provide: GRAPHQL_API_URL, useValue: getWorkerGraphqlUrl() },
    { provide: GRAPHQL_WS_URL, useValue: getWorkerGraphqlWsUrl() },
  ],
}).catch((err) => console.error(err));
