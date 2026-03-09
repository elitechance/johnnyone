const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { URL } = require('node:url');

const puppeteer = require('puppeteer');

const REPO_DIR = path.resolve(__dirname, '../..');
const DIST_DIR = path.join(REPO_DIR, 'dist/desktop/browser');
const HOST_CWD = path.join(REPO_DIR, 'desktop/src-tauri');
const HOST_PORT = 7788;
const AUTH_HEADERS = {
  'x-tenant-id': '00000000-0000-0000-0000-000000000001',
  'x-user-id': '00000000-0000-0000-0000-000000000002',
};

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function getMimeType(filePath) {
  if (filePath.endsWith('.html')) return 'text/html; charset=utf-8';
  if (filePath.endsWith('.js')) return 'application/javascript; charset=utf-8';
  if (filePath.endsWith('.css')) return 'text/css; charset=utf-8';
  if (filePath.endsWith('.json')) return 'application/json; charset=utf-8';
  if (filePath.endsWith('.svg')) return 'image/svg+xml';
  if (filePath.endsWith('.png')) return 'image/png';
  if (filePath.endsWith('.woff2')) return 'font/woff2';
  return 'application/octet-stream';
}

function createStaticServer() {
  return http.createServer((req, res) => {
    const requestUrl = new URL(req.url || '/', 'http://127.0.0.1');
    const relativePath = requestUrl.pathname === '/'
      ? '/index.html'
      : requestUrl.pathname;
    const candidatePath = path.resolve(DIST_DIR, `.${relativePath}`);
    const safePath = candidatePath.startsWith(DIST_DIR) ? candidatePath : path.join(DIST_DIR, 'index.html');
    const filePath = fs.existsSync(safePath) && fs.statSync(safePath).isFile()
      ? safePath
      : path.join(DIST_DIR, 'index.html');

    try {
      const content = fs.readFileSync(filePath);
      res.writeHead(200, { 'Content-Type': getMimeType(filePath) });
      res.end(content);
    } catch (error) {
      res.writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end(`Failed to serve ${filePath}: ${error.message}`);
    }
  });
}

function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : null;
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }
        resolve(port);
      });
    });
  });
}

function canListenOnPort(port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once('error', () => resolve(false));
    server.listen(port, '127.0.0.1', () => {
      server.close(() => resolve(true));
    });
  });
}

function startProcess(label, command, args, options) {
  const logs = [];
  const processHandle = spawn(command, args, {
    cwd: options.cwd,
    env: options.env,
    detached: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  const appendLog = (source, chunk) => {
    const lines = chunk
      .toString()
      .split(/\r?\n/)
      .map((line) => line.trimEnd())
      .filter(Boolean);

    for (const line of lines) {
      logs.push(`[${source}] ${line}`);
      if (logs.length > 200) {
        logs.shift();
      }
    }
  };

  processHandle.stdout.on('data', (chunk) => appendLog('stdout', chunk));
  processHandle.stderr.on('data', (chunk) => appendLog('stderr', chunk));

  const exited = new Promise((resolve) => {
    processHandle.once('exit', (code, signal) => {
      resolve({ code, signal });
    });
  });

  return {
    label,
    process: processHandle,
    logs,
    exited,
  };
}

function formatProcessLogs(handle) {
  if (!handle.logs.length) {
    return '  <no process output captured>';
  }

  return handle.logs
    .slice(-40)
    .map((line) => `  ${line}`)
    .join('\n');
}

async function stopProcess(handle) {
  if (!handle || handle.process.exitCode !== null || handle.process.signalCode !== null) {
    return;
  }

  try {
    process.kill(-handle.process.pid, 'SIGTERM');
  } catch {
    handle.process.kill('SIGTERM');
  }

  const gracefulExit = await Promise.race([
    handle.exited.then(() => true),
    new Promise((resolve) => setTimeout(() => resolve(false), 5000)),
  ]);

  if (gracefulExit) {
    return;
  }

  try {
    process.kill(-handle.process.pid, 'SIGKILL');
  } catch {
    handle.process.kill('SIGKILL');
  }
  await handle.exited;
}

async function postGraphql(graphqlUrl, query, variables, headers = {}) {
  const response = await fetch(graphqlUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      ...headers,
    },
    body: JSON.stringify({ query, variables }),
  });

  const payload = await response.text();

  if (!response.ok) {
    throw new Error(`GraphQL request failed (${response.status}): ${payload}`);
  }

  const json = JSON.parse(payload);
  if (json.errors?.length) {
    throw new Error(json.errors.map((error) => error.message || 'Unknown GraphQL error').join('; '));
  }

  return json.data;
}

async function waitForGraphql(label, graphqlUrl, query, variables, headers, handle, timeoutMs = 90000) {
  const startedAt = Date.now();
  let lastError = null;

  while (Date.now() - startedAt < timeoutMs) {
    if (handle.process.exitCode !== null || handle.process.signalCode !== null) {
      throw new Error(
        `${label} exited before becoming ready.\n${formatProcessLogs(handle)}`,
      );
    }

    try {
      return await postGraphql(graphqlUrl, query, variables, headers);
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
  }

  throw new Error(
    `${label} did not become ready in time: ${lastError?.message || 'unknown error'}\n${formatProcessLogs(handle)}`,
  );
}

async function typeIntoComposer(page, value) {
  const textarea = await page.waitForSelector('pierce/ion-textarea[data-testid="message-input"] textarea', {
    timeout: 10000,
  });

  await textarea.click();
  await textarea.type(value);
}

async function run() {
  assert(
    fs.existsSync(path.join(DIST_DIR, 'index.html')),
    `Desktop build output not found at ${DIST_DIR}. Run npm run build:desktop:web first.`,
  );

  const workerPort = await getFreePort();
  const appPort = await getFreePort();
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'johnnyone-e2e-'));
  const dbPath = path.join(tempDir, 'johnnyone-e2e.db');
  const hostPortAvailable = await canListenOnPort(HOST_PORT);
  assert(hostPortAvailable, `Host port ${HOST_PORT} is already in use. Stop the existing process and rerun the e2e.`);

  const hostGraphqlUrl = `http://127.0.0.1:${HOST_PORT}/graphql`;
  const workerGraphqlUrl = `http://127.0.0.1:${workerPort}/graphql`;
  const workerBaseUrl = `http://127.0.0.1:${workerPort}`;

  const staticServer = createStaticServer();
  let browser = null;
  let host = null;
  let worker = null;

  try {
    host = startProcess('host', 'cargo', ['run', '--bin', 'johnnyone-host'], {
      cwd: HOST_CWD,
      env: {
        ...process.env,
        JOHNNYONE_DB_PATH: dbPath,
        JOHNNYONE_HOST_ADDR: `127.0.0.1:${HOST_PORT}`,
        JOHNNYONE_HOST_SIMULATOR: '1',
      },
    });

    console.log(`desktop-web.e2e: wait for host ${hostGraphqlUrl}`);
    await waitForGraphql(
      'host',
      hostGraphqlUrl,
      'query Health { health }',
      undefined,
      {},
      host,
      90000,
    );

    worker = startProcess(
      'worker',
      'npx',
      ['lokal', 'cf', 'worker', 'sim', '--project', 'johnnyone', '--worker-dir', 'worker', '--port', String(workerPort)],
      {
        cwd: REPO_DIR,
        env: process.env,
      },
    );

    console.log(`desktop-web.e2e: wait for worker ${workerGraphqlUrl}`);
    await waitForGraphql(
      'worker',
      workerGraphqlUrl,
      'query ListAiSessions { listAiSessions { id } }',
      undefined,
      AUTH_HEADERS,
      worker,
      90000,
    );

    await new Promise((resolve) => staticServer.listen(appPort, '127.0.0.1', resolve));
    const appUrl = `http://127.0.0.1:${appPort}`;

    console.log(`desktop-web.e2e: app=${appUrl} worker=${workerBaseUrl} host=${hostGraphqlUrl}`);

    browser = await puppeteer.launch({
      headless: true,
      args: ['--no-sandbox'],
    });

    const page = await browser.newPage();
    await page.evaluateOnNewDocument((edgeUrl, authHeaders) => {
      window.localStorage.setItem('johnnyone_worker_url', edgeUrl);
      window.localStorage.setItem('johnnyone_tenant_id', authHeaders['x-tenant-id']);
      window.localStorage.setItem('johnnyone_user_id', authHeaders['x-user-id']);
    }, workerBaseUrl, AUTH_HEADERS);
    page.on('console', (message) => {
      if (message.type() === 'error') {
        console.error(`[browser:${message.type()}] ${message.text()}`);
      }
    });
    page.on('pageerror', (error) => {
      console.error(`[browser:pageerror] ${error.message}`);
    });

    console.log('desktop-web.e2e: open chat');
    await page.goto(`${appUrl}/chat`, { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('[data-testid="messages-container"]', { timeout: 15000 });

    console.log('desktop-web.e2e: send message');
    await typeIntoComposer(page, 'Write a streaming reply');
    await page.keyboard.press('Enter');

    console.log('desktop-web.e2e: wait for user bubble');
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('[data-testid="message-bubble"][data-message-role="user"]'))
        .some((element) => element.textContent?.includes('Write a streaming reply')),
      { timeout: 15000 },
    );

    console.log('desktop-web.e2e: wait for streamed partial');
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('[data-testid="message-bubble"][data-message-role="assistant"]'))
        .some((element) => {
          const text = element.textContent || '';
          return text.includes('Simulated') && !text.includes('Write a streaming reply');
        }),
      { timeout: 15000 },
    );

    console.log('desktop-web.e2e: wait for streamed final');
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('[data-testid="message-bubble"][data-message-role="assistant"]'))
        .some((element) => {
          const text = element.textContent || '';
          return text.includes('Simulated') && text.includes('Write a streaming reply');
        }),
      { timeout: 15000 },
    );

    console.log('desktop-web.e2e: open settings');
    await page.goto(`${appUrl}/settings`, { waitUntil: 'domcontentloaded' });

    console.log('desktop-web.e2e: wait for settings page');
    await page.waitForFunction(
      () => document.body.textContent?.includes('Desktop Client Connection'),
      { timeout: 15000 },
    );

    console.log('desktop-web.e2e: wait for provider runtime');
    await page.waitForFunction(
      () => {
        const bodyText = document.body.textContent || '';
        return bodyText.includes('Claude Code') && bodyText.includes('ready');
      },
      { timeout: 15000 },
    );

    console.log('desktop-web.e2e: passed');
  } finally {
    if (browser) {
      console.log('desktop-web.e2e: close browser');
      await browser.close();
    }

    console.log('desktop-web.e2e: close servers');
    await new Promise((resolve, reject) => staticServer.close((error) => (error ? reject(error) : resolve())));
    await stopProcess(worker);
    await stopProcess(host);
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
