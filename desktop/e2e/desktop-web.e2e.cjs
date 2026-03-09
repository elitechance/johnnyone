const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const { URL } = require('node:url');

const puppeteer = require('puppeteer');
const { WebSocketServer } = require('ws');

const DIST_DIR = path.resolve(__dirname, '../../dist/desktop/browser');

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function setCorsHeaders(res, origin) {
  res.setHeader('Access-Control-Allow-Origin', origin);
  res.setHeader('Access-Control-Allow-Credentials', 'true');
  res.setHeader('Access-Control-Allow-Headers', 'content-type, x-tenant-id, x-user-id');
}

function jsonResponse(res, origin, body) {
  setCorsHeaders(res, origin);
  res.writeHead(200, {
    'Content-Type': 'application/json; charset=utf-8',
  });
  res.end(JSON.stringify(body));
}

function createState() {
  const now = new Date().toISOString();

  return {
    session: {
      id: 'session-streaming-001',
      title: 'Streaming Session',
      provider: 'claude_code',
      model: 'sonnet',
      workingDirectory: '/workspace/demo',
      status: 'active',
      totalInputTokens: 0,
      totalOutputTokens: 0,
      totalCostCents: 0,
      createdAt: now,
      updatedAt: now,
    },
    messages: [],
    settings: {
      last_working_directory: '/workspace/demo',
    },
    detectedTools: [
      { provider: 'claude_code', command: 'claude', found: true, path: '/usr/local/bin/claude' },
      { provider: 'codex', command: 'codex', found: true, path: '/usr/local/bin/codex' },
      { provider: 'ollama', command: 'ollama', found: true, path: '/usr/local/bin/ollama' },
    ],
    providerConfigs: [
      {
        id: 'provider-claude-code',
        provider: 'claude_code',
        cliPath: '/usr/local/bin/claude',
        apiKey: '',
        defaultModel: 'sonnet',
        settings: '{}',
        isAvailable: true,
        updatedAt: now,
      },
    ],
    sockets: new Map(),
  };
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

function createMockGraphqlServer(state) {
  const server = http.createServer(async (req, res) => {
    const origin = req.headers.origin || 'http://127.0.0.1';

    if (req.method === 'OPTIONS') {
      setCorsHeaders(res, origin);
      res.writeHead(204, {
      });
      res.end();
      return;
    }

    if (req.url !== '/graphql' || req.method !== 'POST') {
      res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('Not found');
      return;
    }

    const body = await new Promise((resolve, reject) => {
      let data = '';
      req.on('data', (chunk) => {
        data += chunk.toString();
      });
      req.on('end', () => resolve(data));
      req.on('error', reject);
    });

    const request = JSON.parse(body || '{}');
    const { query = '', variables = {} } = request;

    if (query.includes('listAiSessions')) {
      jsonResponse(res, origin, { data: { listAiSessions: [state.session] } });
      return;
    }

    if (query.includes('getAiSession')) {
      jsonResponse(res, origin, { data: { getAiSession: state.session } });
      return;
    }

    if (query.includes('listAiMessages')) {
      jsonResponse(res, origin, {
        data: {
          listAiMessages: state.messages.filter((message) => message.sessionId === variables.sessionId),
        },
      });
      return;
    }

    if (query.includes('detectCliTools')) {
      jsonResponse(res, origin, { data: { detectCliTools: state.detectedTools } });
      return;
    }

    if (query.includes('listProviderConfigs')) {
      jsonResponse(res, origin, { data: { listProviderConfigs: state.providerConfigs } });
      return;
    }

    if (query.includes('getSetting')) {
      jsonResponse(res, origin, {
        data: {
          getSetting: state.settings[variables.key] ?? '',
        },
      });
      return;
    }

    if (query.includes('setSetting')) {
      state.settings[variables.key] = variables.value;
      jsonResponse(res, origin, { data: { setSetting: true } });
      return;
    }

    if (query.includes('updateAiSessionProvider')) {
      state.session.provider = variables.provider;
      state.session.updatedAt = new Date().toISOString();
      jsonResponse(res, origin, { data: { updateAiSessionProvider: state.session } });
      return;
    }

    if (query.includes('updateAiSessionWorkingDirectory')) {
      state.session.workingDirectory = variables.workingDirectory;
      state.settings.last_working_directory = variables.workingDirectory;
      state.session.updatedAt = new Date().toISOString();
      jsonResponse(res, origin, { data: { updateAiSessionWorkingDirectory: state.session } });
      return;
    }

    if (query.includes('updateAiSessionTitle')) {
      state.session.title = variables.title;
      state.session.updatedAt = new Date().toISOString();
      jsonResponse(res, origin, { data: { updateAiSessionTitle: state.session } });
      return;
    }

    if (query.includes('createAiSession')) {
      state.session = {
        ...state.session,
        id: `session-${Date.now()}`,
        title: variables.input?.title || 'New Session',
        provider: variables.input?.provider || state.session.provider,
        model: variables.input?.model || state.session.model,
        workingDirectory: variables.input?.workingDirectory || state.session.workingDirectory,
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
      };
      state.messages = [];
      jsonResponse(res, origin, { data: { createAiSession: state.session } });
      return;
    }

    if (query.includes('sendAiChatMessage')) {
      const userMessage = {
        id: `user-${Date.now()}`,
        sessionId: variables.input.sessionId,
        role: 'user',
        content: variables.input.content,
        toolCalls: null,
        finishReason: null,
        inputTokens: 0,
        outputTokens: 0,
        costCents: 0,
        createdAt: new Date().toISOString(),
      };

      const assistantMessage = {
        id: `assistant-${Date.now()}`,
        sessionId: variables.input.sessionId,
        role: 'assistant',
        content: 'Hello from stream',
        toolCalls: null,
        finishReason: 'stop',
        inputTokens: 12,
        outputTokens: 24,
        costCents: 3,
        createdAt: new Date().toISOString(),
      };

      state.messages = state.messages.filter((message) => message.sessionId !== variables.input.sessionId);
      state.messages.push(userMessage);

      setTimeout(() => publishSubscription(state, 'onAiChatDelta', variables.input.sessionId, {
        sessionId: variables.input.sessionId,
        messageId: assistantMessage.id,
        delta: 'Hello',
        chunkType: 'text',
        isFinal: false,
      }), 120);

      setTimeout(() => publishSubscription(state, 'onAiChatDelta', variables.input.sessionId, {
        sessionId: variables.input.sessionId,
        messageId: assistantMessage.id,
        delta: ' from',
        chunkType: 'text',
        isFinal: false,
      }), 360);

      setTimeout(() => publishSubscription(state, 'onAiChatDelta', variables.input.sessionId, {
        sessionId: variables.input.sessionId,
        messageId: assistantMessage.id,
        delta: ' stream',
        chunkType: 'text',
        isFinal: true,
      }), 640);

      setTimeout(() => {
        state.messages.push(assistantMessage);
        state.session.totalInputTokens = 12;
        state.session.totalOutputTokens = 24;
        state.session.totalCostCents = 3;
        state.session.updatedAt = new Date().toISOString();

        publishSubscription(state, 'onAiChatComplete', variables.input.sessionId, {
          sessionId: variables.input.sessionId,
          messageId: assistantMessage.id,
        });
      }, 760);

      setTimeout(() => {
        jsonResponse(res, origin, {
          data: {
            sendAiChatMessage: {
              userMessage,
              assistantMessage,
            },
          },
        });
      }, 860);
      return;
    }

    if (query.includes('stopAiGeneration')) {
      jsonResponse(res, origin, { data: { stopAiGeneration: true } });
      return;
    }

    setCorsHeaders(res, origin);
    res.writeHead(400, { 'Content-Type': 'application/json; charset=utf-8' });
    res.end(JSON.stringify({ errors: [{ message: `Unhandled query: ${query}` }] }));
  });

  const wsServer = new WebSocketServer({
    noServer: true,
    handleProtocols: (protocols) => (
      protocols.has('graphql-transport-ws') ? 'graphql-transport-ws' : false
    ),
  });

  wsServer.on('connection', (socket) => {
    state.sockets.set(socket, new Map());

    socket.on('message', (raw) => {
      const message = JSON.parse(raw.toString());

      if (message.type === 'connection_init') {
        const headers = message.payload?.headers || {};
        if (!headers['x-tenant-id'] || !headers['x-user-id']) {
          socket.close(4401, 'Missing auth headers');
          return;
        }

        socket.send(JSON.stringify({ type: 'connection_ack' }));
        return;
      }

      if (message.type === 'subscribe') {
        const subscriptions = state.sockets.get(socket);
        if (!subscriptions) return;

        const fieldName = message.payload.query.includes('onAiChatComplete')
          ? 'onAiChatComplete'
          : 'onAiChatDelta';

        subscriptions.set(message.id, {
          fieldName,
          sessionId: message.payload.variables.sessionId,
        });
        return;
      }

      if (message.type === 'complete') {
        state.sockets.get(socket)?.delete(message.id);
      }
    });

    socket.on('close', () => {
      state.sockets.delete(socket);
    });
  });

  server.on('upgrade', (req, socket, head) => {
    if (req.url !== '/graphql') {
      socket.destroy();
      return;
    }

    wsServer.handleUpgrade(req, socket, head, (ws) => {
      wsServer.emit('connection', ws, req);
    });
  });

  server.on('close', () => {
    wsServer.close();
  });

  return { server, wsServer };
}

function publishSubscription(state, fieldName, sessionId, payload) {
  for (const [socket, subscriptions] of state.sockets.entries()) {
    for (const [id, subscription] of subscriptions.entries()) {
      if (subscription.fieldName !== fieldName || subscription.sessionId !== sessionId) {
        continue;
      }

      if (socket.readyState !== 1) {
        continue;
      }

      socket.send(JSON.stringify({
        id,
        type: 'next',
        payload: {
          data: {
            [fieldName]: payload,
          },
        },
      }));
    }
  }
}

async function typeIntoComposer(page, value) {
  const textarea = await page.waitForSelector('pierce/ion-textarea[data-testid="message-input"] textarea', {
    timeout: 5000,
  });

  await textarea.click();
  await textarea.type(value);
}

async function run() {
  assert(fs.existsSync(path.join(DIST_DIR, 'index.html')), `Desktop build output not found at ${DIST_DIR}. Run npm run build:desktop:web first.`);

  const state = createState();
  const staticServer = createStaticServer();
  const { server: graphqlServer } = createMockGraphqlServer(state);

  await new Promise((resolve) => staticServer.listen(0, '127.0.0.1', resolve));
  await new Promise((resolve) => graphqlServer.listen(0, '127.0.0.1', resolve));

  const appPort = staticServer.address().port;
  const graphqlPort = graphqlServer.address().port;
  const appUrl = `http://127.0.0.1:${appPort}`;
  const workerUrl = `http://127.0.0.1:${graphqlPort}`;
  console.log(`desktop-web.e2e: app=${appUrl} worker=${workerUrl}`);

  let browser = null;

  try {
    browser = await puppeteer.launch({
      headless: true,
      args: ['--no-sandbox'],
    });

    const page = await browser.newPage();
    await page.evaluateOnNewDocument((edgeUrl) => {
      window.localStorage.setItem('johnnyone_worker_url', edgeUrl);
      window.localStorage.setItem('johnnyone_tenant_id', '00000000-0000-0000-0000-000000000001');
      window.localStorage.setItem('johnnyone_user_id', '00000000-0000-0000-0000-000000000002');
    }, workerUrl);
    page.on('console', (message) => {
      if (message.type() === 'error') {
        console.error(`[browser:${message.type()}] ${message.text()}`);
      }
    });

    console.log('desktop-web.e2e: open chat');
    await page.goto(`${appUrl}/chat`, { waitUntil: 'domcontentloaded' });

    console.log('desktop-web.e2e: wait for session title');
    await page.waitForFunction(
      () => document.querySelector('.chat-title')?.textContent?.includes('Streaming Session'),
      { timeout: 5000 },
    );

    console.log('desktop-web.e2e: send message');
    await typeIntoComposer(page, 'Write a streaming reply');
    await page.keyboard.press('Enter');

    console.log('desktop-web.e2e: wait for user bubble');
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('[data-testid="message-bubble"][data-message-role="user"]'))
        .some((element) => element.textContent?.includes('Write a streaming reply')),
      { timeout: 5000 },
    );

    console.log('desktop-web.e2e: wait for streamed partial');
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('[data-testid="message-bubble"][data-message-role="assistant"]'))
        .some((element) => {
          const text = element.textContent || '';
          return text.includes('Hello') && !text.includes('Hello from stream');
      }),
      { timeout: 5000 },
    );

    console.log('desktop-web.e2e: wait for streamed final');
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('[data-testid="message-bubble"][data-message-role="assistant"]'))
        .some((element) => element.textContent?.includes('Hello from stream')),
      { timeout: 5000 },
    );

    console.log('desktop-web.e2e: open settings');
    await page.goto(`${appUrl}/settings`, { waitUntil: 'domcontentloaded' });

    console.log('desktop-web.e2e: wait for settings page');
    await page.waitForFunction(
      () => document.body.textContent?.includes('Desktop Client Connection'),
      { timeout: 5000 },
    );

    console.log('desktop-web.e2e: wait for provider runtime');
    await page.waitForFunction(
      () => {
        const bodyText = document.body.textContent || '';
        return bodyText.includes('Claude Code') && bodyText.includes('ready');
      },
      { timeout: 5000 },
    );

    console.log('desktop-web.e2e: passed');
  } finally {
    if (browser) {
      console.log('desktop-web.e2e: close browser');
      await browser.close();
    }

    console.log('desktop-web.e2e: close servers');
    await new Promise((resolve, reject) => staticServer.close((error) => (error ? reject(error) : resolve())));
    await new Promise((resolve, reject) => graphqlServer.close((error) => (error ? reject(error) : resolve())));
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
