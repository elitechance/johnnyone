#!/usr/bin/env node
/**
 * Minimal Node.js partner client (Task 02).
 * Run:
 *   JOHNNYONE_BASE_URL=https://johnnyone-dev.ethan-353.workers.dev \
 *   JOHNNYONE_EMAIL=... JOHNNYONE_PW=... JOHNNYONE_TENANT=... \
 *   node docs/examples/partner-client.mjs
 *
 * Or supply a pre-issued token:
 *   JOHNNYONE_BASE_URL=... JOHNNYONE_TOKEN=... \
 *   JOHNNYONE_PROVIDER=ollama JOHNNYONE_WORKDIR=/tmp \
 *   node docs/examples/partner-client.mjs
 *
 * Requires: npm i ws   (uses global fetch on Node 18+)
 *
 * Flow: login (or token) → createAiSession → WSS (Bearer) → subscribe → send command → read updated screen.
 * Errors: use WSS upgrade HTTP status (401/503 via error/unexpected-response), terminal_command_ack.error (e.g. 'forbidden_session'), GraphQL errors[].message.
 * Envelopes match chat-relay-do.ts RelayEnvelope exactly.
 */

import WebSocket from 'ws';

const BASE = process.env.JOHNNYONE_BASE_URL || 'https://johnnyone-dev.ethan-353.workers.dev';
const TOKEN = process.env.JOHNNYONE_TOKEN;
const EMAIL = process.env.JOHNNYONE_EMAIL || 'partner@example.com';
const PW = process.env.JOHNNYONE_PW;
const TENANT = process.env.JOHNNYONE_TENANT || 'your-tenant';
const PROVIDER = process.env.JOHNNYONE_PROVIDER || 'ollama';
const WORKDIR = process.env.JOHNNYONE_WORKDIR || '/tmp/partner-demo';

const gql = async (query, variables, token, tenantId) => {
  const res = await fetch(`${BASE}/graphql`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      ...(tenantId ? { 'x-tenant-id': tenantId } : {}),
      ...(token ? { authorization: `Bearer ${token}` } : {})
    },
    body: JSON.stringify({ query, variables })
  });
  if (!res.ok) throw new Error(`GraphQL ${res.status}`);
  const j = await res.json();
  if (j.errors?.length) {
    const msg = j.errors[0].message;
    if (msg.includes('Desktop not connected') || msg.includes('503')) {
      throw new Error('503 no online desktop node');
    }
    throw new Error(msg);
  }
  return j;
};

(async () => {
  let token = TOKEN;
  if (!token) {
    if (!PW) throw new Error('Provide JOHNNYONE_TOKEN or JOHNNYONE_PW');
    const { data: { login } } = await gql(
      'mutation($i:LoginInput!){login(input:$i){accessToken refreshToken}}',
      { i: { email: EMAIL, password: PW, tenantId: TENANT } },
      null,
      TENANT
    );
    token = login.accessToken;
    console.error('Logged in, got accessToken');
  }

  // create session
  const { data: { createAiSession: s } } = await gql(
    'mutation($i:CreateAiSessionInput!){createAiSession(input:$i){id}}',
    { i: { provider: PROVIDER, workingDirectory: WORKDIR } },
    token
  );
  const sessionId = s.id;
  console.error('Session created:', sessionId);

  // WSS
  const wsUrl = BASE.replace(/^https/, 'wss') + '/api/relay/ws';
  const ws = new WebSocket(wsUrl, { headers: { authorization: `Bearer ${token}` } });

  const timeout = setTimeout(() => { console.error('timeout'); ws.close(); process.exit(1); }, 15000);

  let firstScreen = null;
  let lastScreen = null;
  let cmdSentAt = 0;
  ws.on('open', () => {
    // subscribe for future screens (use data.control)
    ws.send(JSON.stringify({ type: 'terminal_command', data: { requestId: 'sub-' + Date.now(), sessionId, data: '', control: 'visual_subscribe' } }));
    // pull current screen (subscribe streams FUTURE only)
    ws.send(JSON.stringify({ type: 'terminal_command', data: { requestId: 'ref-' + Date.now(), sessionId, data: '', control: 'visual_refresh' } }));
  });

  ws.on('message', (buf) => {
    const msg = JSON.parse(buf.toString());
    if (msg.type === 'terminal_screen') {
      console.log('SCREEN:', JSON.stringify(msg.data));
      lastScreen = msg.data;
      if (!firstScreen) {
        firstScreen = msg.data;
        console.error('received first screen');
        // now send command to prove updated screen
        cmdSentAt = Date.now();
        ws.send(JSON.stringify({ type: 'terminal_command', data: { requestId: 'cmd-' + Date.now(), sessionId, data: 'echo partner-demo\r' } }));
        // schedule a delayed visual_refresh to pull a post-render screen (capture loop is ~2s active + render)
        setTimeout(() => {
          ws.send(JSON.stringify({ type: 'terminal_command', data: { requestId: 'ref-post-' + Date.now(), sessionId, data: '', control: 'visual_refresh' } }));
          console.error('sent post-command visual_refresh to force current rendered screen');
        }, 2200);
      } else if (cmdSentAt > 0 && (Date.now() - cmdSentAt > 1500)) {
        // updated screen after command + delay for output
        clearTimeout(timeout);
        ws.close();
        console.error('received updated screen after command');
        const firstContent = (firstScreen.content || '').replace(/\n/g, '');
        const newContent = (msg.data.content || '').replace(/\n/g, '');
        const visibleDelta = newContent !== firstContent;
        const hasCommandText = (msg.data.content || '').includes('partner-demo') || (msg.data.content || '').includes('echo partner-demo');
        if (visibleDelta && (hasCommandText || newContent.length > firstContent.length + 2)) {
          console.error('SUCCESS: updated screen differs from first with visible command/output (input round-trip observed)');
        } else if (visibleDelta) {
          console.error('INFO: updated screen differs (cursor/metadata only)');
        } else {
          console.error('INFO: received screen after command (input accepted)');
        }
        process.exit(0);
      }
    } else if (msg.type === 'terminal_command_ack') {
      if (msg.error) {
        if (msg.error === 'forbidden_session') {
          console.error('ERROR: forbidden_session (session not owned by this tenant/user)');
        } else {
          console.error('ERROR: ' + msg.error);
        }
        process.exit(1);
      }
      console.error('command ack');
    }
  });

  ws.on('error', (e) => {
    const msg = e.message || String(e);
    if (msg.includes('401') || msg.includes('Unauthorized')) {
      console.error('ERROR: 401 unauthorized on WSS upgrade (invalid/missing token)');
    } else if (msg.includes('503') || msg.includes('Service Unavailable') || msg.includes('no online') || msg.includes('Desktop not connected')) {
      console.error('ERROR: 503 no online desktop node');
    } else {
      console.error('WS error:', msg);
    }
    process.exit(1);
  });
  ws.on('close', (c, r) => {
    if (c === 1008 || c === 4001) console.error('ERROR: 401 unauthorized on WSS upgrade (invalid/missing token)');
    else if (c === 1013 || String(r).includes('503')) console.error('ERROR: 503 no online desktop node');
    else if (c !== 1000) console.error('WS closed', c, r.toString());
  });
  ws.on('unexpected-response', (req, res) => {
    const status = res.statusCode;
    if (status === 401) console.error('ERROR: 401 unauthorized on WSS upgrade (invalid/missing token)');
    else if (status === 503) console.error('ERROR: 503 no online desktop node');
    else console.error('WS unexpected response:', status);
    process.exit(1);
  });
})().catch(e => {
  if (e.message && e.message.includes('401')) console.error('ERROR: 401 on GraphQL (auth failed)');
  else if (e.message && (e.message.includes('503') || e.message.includes('no online desktop node'))) console.error('ERROR: 503 no online desktop node');
  else console.error('FAIL:', e.message);
  process.exit(1);
});
