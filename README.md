# JohnnyOne

> "Number 5 is alive!" -- Short Circuit (1986)

Personal AI agent platform. Your desktop is the always-on compute node (Tauri + Ionic/Angular), your phone is the remote control (Ionic/Angular + Capacitor), and a Cloudflare Worker acts as a thin relay in between. Multi-provider CLI support with Claude Code, Codex, Cline, and Ollama for fully offline operation.

## Architecture

All AI processing happens on the desktop. The worker is a thin relay layer.

```
  +----------+           +-------------------+           +-----------------+
  |  Mobile  | <-------> |   CF Worker       | <-------> | Desktop Agent   |
  |  App     |  GraphQL  |   (Relay Hub)     |    WSS    | (Tauri)         |
  |          |  + WS     |                   |           |                 |
  +----------+           | +---------------+ |           | +-------------+ |
                         | | D1 (nodes)    | |           | | SQLite DB   | |
  +----------+           | | ChatRelayDO   | |           | | CLI Runner  | |
  |  Desktop | <-------> | | (WebSocket    | |           | | Claude Code | |
  |  Web UI  |  Angular  | |  relay)       | |           | | Codex       | |
  +----------+           | +---------------+ |           | | Cline       | |
                         +-------------------+           | | Ollama      | |
                                                         | +-------------+ |
                                                         +-----------------+
```

## Project Structure

```
johnnyone/
  desktop/              Ionic/Angular desktop frontend + Tauri backend
    src/                  Angular app (chat, sessions, tools, settings pages)
    src-tauri/            Rust backend (WS agent, CLI runners, RPC handlers, SQLite)
  mobile/               Ionic/Angular + Capacitor mobile app (Android)
    src/                  Angular app (chat, sessions, settings pages)
  worker/               Cloudflare Worker -- GraphQL API + WebSocket relay
    d1/                   D1 migrations and seed data
    resolvers/            GraphQL resolvers (ai/, channels/)
    schema/               GraphQL schema definitions
  ui/                   Shared UI component library
    src/components/       Reusable Angular components (8 components)
    src/services/         GraphQL client, JohnnyAPI, AI chat state
    src/models/           TypeScript models and interfaces
  shared/               Cross-platform shared code
    src/types/            WebSocket protocol types, tool definitions
    src/utils/            ID generation, date formatting, helpers
  package.json          Root monorepo config (Nx 20, Angular 19)
  nx.json               Nx workspace configuration
  lokal.yaml            Lokal deployment configuration
  tsconfig.base.json    Shared TypeScript config
```

## Tech Stack

| Layer         | Technology                              |
|---------------|-----------------------------------------|
| Frontend      | Ionic 8 / Angular 19                    |
| Desktop       | Tauri 2 (Rust)                          |
| Mobile        | Capacitor 6 (Android)                   |
| Backend       | Cloudflare Workers                      |
| Database      | D1 (SQLite at the edge)                 |
| Real-time     | Durable Objects + WebSockets            |
| LLM Providers | Claude Code, Codex, Cline, Ollama (CLI)  |
| Monorepo      | Nx 20                                   |
| GraphQL       | Custom resolver layer + graphql-ws      |

## Getting Started

### Prerequisites

- Node.js 20+
- Rust / Cargo (for Tauri desktop builds)
- Wrangler CLI (bundled via devDependencies)
- Lokal CLI (for worker simulation and D1 migrations)

### Install

```bash
npm install
```

### Development

```bash
# Start the desktop Tauri app (includes frontend dev server)
npm run start:desktop

# (Optional) Start desktop frontend only (port 4200)
npm run start:desktop:web

# Start the worker API (port 7714)
npm run start:worker

# Apply D1 database migrations (local)
npm run migrate:worker

# Start the mobile Angular dev server (port 4201)
npm run start:mobile
```

### Build

```bash
npm run build:desktop    # Build desktop Angular frontend
npm run build:desktop:tauri  # Build native desktop app (Tauri)
npm run build:mobile     # Build mobile Angular app
npm run build:worker     # Build Cloudflare Worker
```

## Key Features

- **Relay architecture** -- Worker is a thin relay; all AI processing, sessions, and messages live on the desktop
- **Multi-provider CLI** -- Claude Code, Codex, Cline, and Ollama CLI runners with streaming output
- **RPC-over-relay** -- Mobile queries desktop SQLite (sessions, messages) via Worker → ChatRelayDO → WebSocket → Desktop RPC
- **Real-time streaming** -- Token-by-token response streaming through ChatRelayDO WebSocket relay
- **Offline mode** -- Ollama CLI integration means the agent works without internet
- **Mobile remote control** -- List sessions, view messages, send chat, and delete sessions from your phone via swipe-to-delete
- **Desktop-local storage** -- Sessions and messages stored in local SQLite with `ON DELETE CASCADE` for clean session removal
- **Channel adapters** -- Telegram, Discord, WhatsApp adapter stubs (channel resolvers)

## Data Flow

### Chat (relay)

1. Mobile sends a message via GraphQL `sendRelayChatMessage` mutation
2. Worker finds an online desktop node in D1 and forwards the request to `ChatRelayDO`
3. ChatRelayDO relays the message over WebSocket to the desktop agent
4. Desktop spawns the configured CLI provider (Claude Code, Codex, Cline, or Ollama) as a subprocess
5. CLI output is streamed line-by-line, parsed into chunks, and sent back through the WebSocket as `chat_delta` frames
6. Worker relays deltas to the mobile client's WebSocket connection in real time
7. On completion, desktop saves user + assistant messages to local SQLite

### RPC queries (read/delete)

1. Mobile calls a GraphQL query or mutation (e.g. `listAiSessions`, `deleteAiSession`)
2. Worker resolver finds an online node and POSTs to `ChatRelayDO` at `/relay-rpc`
3. ChatRelayDO forwards the RPC request over WebSocket to the desktop agent
4. Desktop executes the query against local SQLite and returns the result
5. Worker resolver returns the data to the mobile client via GraphQL

## Ports

| Service           | Port |
|-------------------|------|
| Worker API        | 7714 |
| Desktop frontend  | 4200 |
| Mobile frontend   | 4201 |

## GraphQL API

The worker exposes a GraphQL API with:

- **4 queries** -- `listDesktopNodes`, `listAiSessions`, `getAiSession`, `listAiMessages` (sessions/messages resolve via RPC to desktop SQLite)
- **4 mutations** -- `registerDesktopNode`, `updateDesktopNodeStatus`, `sendRelayChatMessage`, `deleteAiSession`
- **3 subscriptions** -- `onDesktopNodeStatus`, `onRelayChatDelta` (streaming), `onRelayChatMessage`

Schema: [`worker/schema/johnnyone-ai.graphql`](worker/schema/johnnyone-ai.graphql)

### Desktop RPC Methods

The desktop agent handles these RPC methods over the WebSocket relay:

| Method | Params | Description |
|--------|--------|-------------|
| `list_sessions` | `status?` | List all sessions from local SQLite |
| `get_session` | `id` | Get a single session by ID |
| `list_messages` | `sessionId`, `limit?`, `offset?` | List messages for a session |
| `delete_session` | `id` | Kill active process + delete session (cascades to messages) |

## Roadmap

| Phase  | Description                                         | Status      |
|--------|-----------------------------------------------------|-------------|
| 0      | Nx monorepo scaffold, project structure             | Done        |
| 1A     | Foundation + basic AI chat                          | Done        |
| 1B     | Relay architecture + ChatRelayDO streaming          | Done        |
| 1C     | Multi-provider CLI runners (Claude Code, Codex, Cline, Ollama) | Done |
| 1D     | Desktop node registration + heartbeat               | Done        |
| 1E     | Mobile RPC (list/view/delete sessions & messages)   | Done        |
| 2      | Channel adapters (Telegram, Discord, WhatsApp)      | Planned     |
| 3      | Browser automation, cron scheduling, voice input    | Planned     |

## License

Private / Proprietary. All rights reserved.
