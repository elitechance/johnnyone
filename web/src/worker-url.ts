const PROD_WORKER_URL = 'https://johnnyone.ethan-353.workers.dev';
const DEV_WORKER_URL = 'https://johnnyone-dev.ethan-353.workers.dev';
const LOCAL_WORKER_URL = 'http://127.0.0.1:7714';

const LEGACY_WORKER_URLS: Record<string, string> = {
  'https://johnnyone-hub.ethan-353.workers.dev': PROD_WORKER_URL,
  'https://johnnyone-dev-hub.ethan-353.workers.dev': DEV_WORKER_URL,
};

export function migrateStoredWorkerUrl(stored: string | null): string | null {
  const trimmed = stored?.trim();
  if (!trimmed) return null;
  return LEGACY_WORKER_URLS[trimmed] ?? trimmed;
}

/**
 * Env-detected worker URL.
 *
 * Order of precedence:
 *  1. localStorage `johnnyone_worker_url` (legacy hub names auto-migrated)
 *  2. Local dev fallback when host is localhost/127.0.0.1
 *  3. Deployed worker URL when host ends in `.pages.dev`
 *  4. Same origin otherwise (single-domain deploys)
 */
export function getWorkerBaseUrl(): string {
  if (typeof window === 'undefined') {
    return LOCAL_WORKER_URL;
  }

  const { hostname } = window.location;

  // Prod Pages must use the prod worker. A stale localStorage override (legacy
  // johnnyone-hub) has a different D1 — login fails with "Invalid email or password".
  if (hostname.endsWith('.pages.dev')) {
    const stored = window.localStorage.getItem('johnnyone_worker_url');
    const migrated = migrateStoredWorkerUrl(stored);
    if (migrated !== PROD_WORKER_URL) {
      window.localStorage.setItem('johnnyone_worker_url', PROD_WORKER_URL);
    }
    return PROD_WORKER_URL;
  }

  const stored = window.localStorage.getItem('johnnyone_worker_url');
  const migrated = migrateStoredWorkerUrl(stored);
  if (migrated && migrated !== stored?.trim()) {
    window.localStorage.setItem('johnnyone_worker_url', migrated);
  }
  if (migrated) {
    return migrated;
  }

  if (!['localhost', '127.0.0.1'].includes(hostname)) {
    return window.location.origin;
  }

  return LOCAL_WORKER_URL;
}

export function getWorkerGraphqlUrl(): string {
  return `${getWorkerBaseUrl().replace(/\/$/, '')}/graphql`;
}

export function getWorkerGraphqlWsUrl(): string {
  const httpUrl = getWorkerGraphqlUrl();
  if (httpUrl.startsWith('https://')) {
    return httpUrl.replace('https://', 'wss://');
  }
  if (httpUrl.startsWith('http://')) {
    return httpUrl.replace('http://', 'ws://');
  }
  return httpUrl;
}