import { Injectable, inject } from '@angular/core';
import { SessionReport } from '../models/stream-event.model';
import { Observable } from 'rxjs';
import { map } from 'rxjs/operators';
import { GraphQLClient } from './graphql-client';
import { AiSession } from '../models/ai-session.model';
import { AiMessage, AiMessageDelta } from '../models/ai-message.model';
import { ToolDefinition, ToolExecution } from '../models/tool.model';
import { ProviderConfig, AiUsageSummary } from '../models/provider.model';
import { DesktopNode } from '../models/desktop-node.model';
import { TerminalScreen } from '../models/terminal.model';

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  workingDirectory?: string;
  /** When set, attach to this existing external tmux session instead of spawning a shell. */
  tmuxSessionName?: string;
}

/** An external tmux session a terminal can attach to. */
export interface TmuxSession {
  name: string;
  attached: boolean;
  windows: number;
}

export interface AiChatRunResult {
  userMessage: AiMessage;
  assistantMessage: AiMessage;
}

export interface AiChatComplete {
  sessionId: string;
  messageId: string;
}

export interface DetectedCliTool {
  provider: string;
  command: string;
  found: boolean;
  path?: string;
}

// ── M2M API keys (partner/developer credentials) ──────────────────────────
// Mirrors worker/schema/johnnyone-api-keys.graphql. The full secret (`jk_...`)
// is returned exactly once on create and is never re-readable.
export interface ApiKey {
  id: string;
  name: string;
  keyPrefix: string;
  scopes: string[];
  lastUsedAt?: string | null;
  expiresAt?: string | null;
  revokedAt?: string | null;
  createdAt: string;
}

export interface CreateApiKeyInput {
  name: string;
  scopes: string[];
  expiresAt?: string;
}

export interface CreateApiKeyResult {
  apiKey: ApiKey;
  secret: string;
}

export interface CreateAgentPlanInput {
  runType?: 'planning' | 'development';
  title?: string;
  workspacePath: string;
  planPath: string;
  workerProvider: string;
  reviewerProvider: string;
  brief?: string;
  appScope?: string;
  docsScope?: string;
  referencePaths?: string;
  /** When workerProvider is 'shell', commands run in the spawned shell on first launch. */
  workerSetupCommands?: string;
  /** When reviewerProvider is 'shell', commands run in each spawned reviewer shell. */
  reviewerSetupCommands?: string;
  /** Per-stage agent binding: which agent BUILDS the development stage and which VALIDATES it.
   * Omit both to inherit the planning agent (behaviour before per-stage providers existed). */
  devWorkerProvider?: string;
  devReviewerProvider?: string;
}

/** Create an Initiative in briefing (overhaul P4, D1). `brief` is the raw ask. */
export interface CreateBriefingInput {
  title?: string;
  workspacePath: string;
  brief: string;
  workerProvider: string;
  reviewerProvider: string;
  model?: string;
  /** Per-stage agent binding: which agent BUILDS the development stage and which VALIDATES it.
   * Omit both to inherit the planning agent (behaviour before per-stage providers existed). */
  devWorkerProvider?: string;
  devReviewerProvider?: string;
  executorConfig?: string;
}

/** Accept a briefing brief (overhaul P4, D1/D4). `finalBrief` optionally overrides the stored draft. */
export interface AcceptBriefInput {
  initiativeId: string;
  finalBrief?: string;
}

export interface AgentPlan {
  id: string;
  runType: 'planning' | 'development' | string;
  title: string;
  workspacePath: string;
  planPath: string;
  status: string;
  workerSessionId?: string;
  reviewerSessionId?: string;
  workerProvider: string;
  reviewerProvider: string;
  /** Non-null on a planning row that binds a different agent for the development stage. */
  devWorkerProvider?: string | null;
  devReviewerProvider?: string | null;
  currentPhaseId?: string;
  currentPhaseIndex: number;
  error?: string;
  brief?: string;
  appScope?: string;
  docsScope?: string;
  referencePaths?: string;
  /** Non-empty when an amendment cycle is in flight; cleared when T2 PASSes. */
  amendBrief?: string;
  /** Configurable validation lenses as a JSON string (overhaul P7, D1). Null/empty → the desktop
   * resolves the default product/qa/lead template. Nullable for deploy-skew safety. */
  validationConfig?: string | null;
  phaseRunMode: 'continue' | 'single' | string;
  /** Groups a planning stage-run + a development stage-run into one Initiative. */
  initiativeId: string;
  /** Lifecycle stage. */
  initiativeStatus: 'briefing' | 'planning' | 'development' | 'review' | 'done' | string;
  /** Condition axis, independent of stage. */
  health: 'in-progress' | 'needs-attention' | 'blocked' | string;
  /** Non-empty only on a briefing Initiative: its kind='user' chat session (overhaul P4, D3). */
  briefingSessionId?: string;
  /** Local-small executor JSON. Null/empty = commercial. */
  executorConfig?: string | null;
  /** Host consecutive non-pass planning rounds for this initiative. */
  consecutiveNonPassPlanningRounds?: number | null;
  /** The coordinator's own test gate at PLAN level (migration 023). Nullable for deploy skew; null
   *  or empty = no plan-level gate. Prefer `AgentPlanPhase.effectiveTestCommands` for display. */
  testCommands?: string[] | null;
  createdAt: string;
  updatedAt: string;
}

export interface AgentPlanPhase {
  id: string;
  planId: string;
  phaseId: string;
  phaseTitle: string;
  phaseIndex: number;
  status: string;
  workerStartedAt?: string;
  workerIdleAt?: string;
  reviewerStartedAt?: string;
  reviewerIdleAt?: string;
  gateVerdict: string;
  clarificationAttempts: number;
  summary?: string;
  findingsJson: string;
  /** This phase's OWN gate (migration 025). Null/empty = inherits the plan's. */
  testCommands?: string[] | null;
  /** The gate that ACTUALLY applies: this phase's commands when it declares any, else the plan's.
   *  Resolved by the host's `effective_test_commands` — the single place that precedence lives, so
   *  no client re-implements the fallback. Non-null; `[]` is the honest "no gate at either level". */
  effectiveTestCommands?: string[];
  createdAt: string;
  updatedAt: string;
}

export interface AgentPlanTask {
  id: string;
  planId: string;
  phaseId: string;
  taskId: string;
  taskTitle: string;
  taskIndex: number;
  promptPath: string;
  statusPath?: string;
  decisionsPath?: string;
  status: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentPlanEvent {
  id: string;
  planId: string;
  phaseId?: string;
  phaseIndex?: number;
  phaseTitle?: string;
  eventType: string;
  actor: string;
  category: string;
  summary: string;
  statusBefore?: string;
  statusAfter?: string;
  reason?: string;
  verdict?: string;
  taskId?: string;
  clarificationAttempt?: number;
  payloadJson: string;
  createdAt: string;
}

export interface AgentPlanRun {
  plan: AgentPlan;
  phases: AgentPlanPhase[];
  tasks: AgentPlanTask[];
  events: AgentPlanEvent[];
}

export interface HostFileEntry {
  path: string;
  name: string;
  kind: string;
  status?: string;
  size?: number;
}

export interface HostFileContent {
  path: string;
  name: string;
  kind: string;
  contentType: string;
  encoding: string;
  content: string;
  size: number;
}

// File manager (overhaul P2, files_root-rooted). Mirrors the Phase-02 GraphQL types and the host
// serde (host_files.rs) — camelCase, separate from the plan-scoped HostFile* interfaces above.
export interface FileEntry {
  name: string;
  path: string; // relative to files_root
  kind: 'file' | 'dir' | string;
  size?: number;
  modified?: number; // unix millis
}
export interface DirListing {
  path: string;
  root: string;
  entries: FileEntry[];
}
/** A streaming delta of a relay chat response (worker `RelayChatDelta`). */
export interface RelayChatDeltaMsg {
  relayId: string;
  sessionId: string;
  delta: string;
  chunkType: string;
  isFinal: boolean;
}
/** A completed relay chat message (worker `RelayChatMessage`). */
export interface RelayChatMessageMsg {
  relayId: string;
  sessionId: string;
  role: string;
  content: string;
}
export interface FileContent {
  path: string;
  name: string;
  contentType: string;
  encoding: 'utf8' | 'base64' | string;
  content: string;
  size: number;
}
export interface FileOpResult {
  path: string;
  ok: boolean;
}
export interface UploadChunkResult {
  path: string;
  received: number;
  complete: boolean;
}

export interface WorkspaceFileDiff {
  path: string;
  diff: string;
}

/** One changed file in a whole-tree diff (overhaul P7, D7). Mirrors the Rust `GitDiffFile`. */
export interface GitDiffFile {
  path: string;
  oldPath?: string | null;
  additions: number;
  deletions: number;
  binary: boolean;
  diff: string;
}

/** Working-tree diff of a repo (overhaul P7, D7/D10/D11). Mirrors the Rust `GitDiffView`; a non-repo
 * or clean path yields `clean:true`, `files:[]`, `repoRoot:null` (benign empty state). */
export interface GitDiffView {
  repoRoot?: string | null;
  branch?: string | null;
  clean: boolean;
  files: GitDiffFile[];
}

export interface GitFilesView {
  path: string;
  repoRoot?: string;
  branch?: string;
  clean: boolean;
  entries: HostFileEntry[];
}

export interface GitActionResult {
  success: boolean;
  output: string;
}

export interface WorkspaceValidation {
  valid: boolean;
  workspacePath: string;
  planPath: string;
  title?: string;
  phaseCount: number;
  taskCount: number;
  error?: string;
}

export interface PlannerSmallModePrompts {
  planner: string;
  reviewer: string;
  leafWrapper: string;
  amendPlanner: string;
}

export interface PromptLibraryEntry {
  id: string;
  key: string;
  name: string;
  role: string;
  scope: string;
  version: string;
  usedCount: number;
  customised: boolean;
  readOnly: boolean;
  engineReads: boolean;
}

export interface PlannerPromptSettings {
  schema?: string;
  development: {
    worker: string;
    reviewer: string;
    workerNudge?: string | null;
  };
  planning: {
    planner: string;
    reviewer: string;
    amendPlanner?: string | null;
    amendReviewer?: string | null;
  };
  smallMode?: PlannerSmallModePrompts;
}

export interface ChatAttachment {
  id: string;
  sessionId?: string;
  originalName: string;
  contentType: string;
  size: number;
  status: string;
  uploadedAt: string;
  localPath?: string;
}

/** One run of a phase's (or a plan's) effective gate — `runAgentPlanTestGate`. */
export interface TestGateRun {
  /** Whether a gate was DECLARED at all, at either level. `exitCode === 0` alone only means
   *  "nothing failed": a phase with no gate also exits 0. Ask this first. */
  gateDeclared: boolean;
  /** The score, and the only one. 0 = pass. Negative values are reserved outcomes with no real
   *  exit code (-1 signal, -2 gate unreadable, -3 timeout) — all of them failures. */
  exitCode: number;
  /** ADVISORY AND NULLABLE, and null is NOT an error (a lint or a build reports no count). Never
   *  branch on it; render it through `gateCollectedLabel` so null never shows as 0. */
  collected?: number | null;
  command?: string | null;
  commandsRun: number;
  output: string;
  truncated: boolean;
}

// --- The workflow arena (desktop migration 026) ---
//
// Every RULE behind these objects lives on the host (`budgets_are_comparable`, `promotion_block`,
// `workflow_search_state`). The client reads the host's answers — `WorkflowPref.comparable`,
// `WorkflowSearchState.halted` — and never recomputes them.

/** The SEARCHED ARTIFACT: the four prompt parts a phase-type is executed with. */
export interface Workflow {
  id: string;
  repo: string;
  phaseType: string;
  /** Model family. Part of a champion's IDENTITY, not its environment. */
  family: string;
  /** Monotonic WITHIN (repo, phaseType, family), from 1. */
  version: number;
  parentId?: string | null;
  role: string;
  instruction: string;
  contract: string;
  /** JSON string: steps, parallel groups, returns. Served as stored. */
  hop: string;
  fanOut: number;
  isChampion: boolean;
  createdAt: string;
}

/** One execution of one workflow. */
export interface WorkflowRun {
  id: string;
  workflowId: string;
  planId?: string | null;
  seed: number;
  /** THE SCORE. 0 = the gate passed. NULL = the gate never ran, which is NOT a pass. */
  gateExit?: number | null;
  /** Advisory and nullable — see `TestGateRun.collected`. */
  gateCollected?: number | null;
  /** JSON object, model -> seats. */
  seatsHeld: string;
  /** JSON object: fleet contention at start. Comparability evidence. */
  fleetBusy: string;
  startedAt: string;
  endedAt?: string | null;
}

/** One outcome: this run beat that run. `comparable` is the host's, not an argument. */
export interface WorkflowPref {
  id: string;
  winnerRunId: string;
  loserRunId: string;
  /** False when the two runs held different seat budgets. Still recorded, but CANNOT promote. */
  comparable: boolean;
  reason?: string | null;
  createdAt: string;
}

/** The search's state for one (repo, phaseType, family) — the halt as DATA. */
export interface WorkflowSearchState {
  repo: string;
  phaseType: string;
  family: string;
  consecutiveRejections: number;
  /** The host REFUSES `startWorkflowRun` while this is true. The client explains it, never
   *  recounts it. */
  halted: boolean;
  strikeLimit: number;
  /** Newest first — the one thing a halted search owes a human. */
  recentRejections: string[];
  championWorkflowId?: string | null;
}

@Injectable({ providedIn: 'root' })
export class JohnnyApiService {
  private readonly gql = inject(GraphQLClient);
  /**
   * The ONE `AiSession` selection set. It was duplicated seven times inline and had drifted:
   * `attachedTmux` was present in the list/get/create copies but MISSING from the archive/title/
   * workingDirectory/provider mutations, so those mutations returned a session whose `attachedTmux`
   * was `undefined`. Nothing depended on it (every caller re-fetches), but the latent bug was real:
   * patch a local list from one of those results instead of re-fetching and `isShellSession`
   * (`web/.../shells-page-logic.ts`) drops the row — attached shells silently vanish from `/shells`
   * until a reload. Selected once here so it cannot drift again.
   */
  private readonly sessionFields = `
    id title provider model workingDirectory status
    totalInputTokens totalOutputTokens totalCostCents
    createdAt updatedAt attachedTmux
  `;
  private readonly agentPlanRunFields = `
    plan {
      id runType title workspacePath planPath status workerSessionId reviewerSessionId
      workerProvider reviewerProvider currentPhaseId currentPhaseIndex error
      brief appScope docsScope referencePaths amendBrief validationConfig executorConfig phaseRunMode
      initiativeId initiativeStatus health briefingSessionId
      consecutiveNonPassPlanningRounds testCommands
      createdAt updatedAt
    }
    phases {
      id planId phaseId phaseTitle phaseIndex status workerStartedAt workerIdleAt
      reviewerStartedAt reviewerIdleAt gateVerdict clarificationAttempts summary
      findingsJson testCommands effectiveTestCommands createdAt updatedAt
    }
    tasks {
      id planId phaseId taskId taskTitle taskIndex promptPath statusPath
      decisionsPath status createdAt updatedAt
    }
    events {
      id planId phaseId phaseIndex phaseTitle eventType actor category summary
      statusBefore statusAfter reason verdict taskId clarificationAttempt payloadJson createdAt
    }
  `;

  /**
   * The ONE `Workflow` selection set, hoisted for the same reason `sessionFields` is: the list, the
   * detail page and every mutation that returns a Workflow must ask for the same fields, or a
   * screen patched from one result silently loses a slot.
   */
  private readonly workflowFields = `
    id repo phaseType family version parentId
    role instruction contract hop fanOut isChampion createdAt
  `;
  /** The ONE `WorkflowRun` selection set. `gateCollected` is in it on purpose: a UI that never
   *  asked for the count cannot tell "not reported" from "the field was not requested". */
  private readonly workflowRunFields = `
    id workflowId planId seed gateExit gateCollected seatsHeld fleetBusy startedAt endedAt
  `;
  private readonly workflowPrefFields = `
    id winnerRunId loserRunId comparable reason createdAt
  `;
  private readonly workflowSearchStateFields = `
    repo phaseType family consecutiveRejections halted strikeLimit recentRejections
    championWorkflowId
  `;

  // ── Sessions ──────────────────────────────────────────────────────────

  listSessions(status?: string): Observable<AiSession[]> {
    const query = `query ListSessions($status: String) {
      listAiSessions(status: $status) { ${this.sessionFields} }
    }`;

    return this.gql
      .queryPreferLocalHost<{ listAiSessions: AiSession[] }>(query, query, { status })
      .pipe(map((data) => data.listAiSessions));
  }

  /** A session's persisted reports, oldest->newest — what the transcript hydrates from. */
  listSessionReports(sessionId: string, limit = 100): Observable<SessionReport[]> {
    const query = `query ListSessionReports($sessionId: ID!, $limit: Int) {
      listSessionReports(sessionId: $sessionId, limit: $limit) {
        id sessionId kind role summary markdown createdAt
      }
    }`;
    return this.gql
      .queryPreferLocalHost<{ listSessionReports: SessionReport[] }>(query, query, {
        sessionId,
        limit,
      })
      .pipe(map((data) => data.listSessionReports));
  }

  /** External tmux sessions a new terminal can attach to (excludes johnnyone_<id> panes). */
  listTmuxSessions(): Observable<TmuxSession[]> {
    const query = `query ListTmuxSessions {
      listTmuxSessions { name attached windows }
    }`;
    return this.gql
      .queryPreferLocalHost<{ listTmuxSessions: TmuxSession[] }>(query, query, {})
      .pipe(map((data) => data.listTmuxSessions));
  }

  getSession(id: string): Observable<AiSession> {
    const query = `query GetSession($id: ID!) {
      getAiSession(id: $id) { ${this.sessionFields} }
    }`;

    return this.gql
      .queryPreferLocalHost<{ getAiSession: AiSession }>(query, query, { id })
      .pipe(map((data) => data.getAiSession));
  }

  deleteSession(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteAiSession: boolean }>(
        `mutation DeleteAiSession($id: ID!) {
          deleteAiSession(id: $id)
        }`,
        { id }
      )
      .pipe(map((data) => data.deleteAiSession));
  }

  createSession(input: CreateAiSessionInput): Observable<AiSession> {
    return this.gql
      .mutate<{ createAiSession: AiSession }>(
        `mutation CreateAiSession($input: CreateAiSessionInput!) {
          createAiSession(input: $input) {
            ${this.sessionFields}
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createAiSession));
  }

  updateSessionTitle(id: string, title: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionTitle: AiSession }>(
        `mutation UpdateAiSessionTitle($id: ID!, $title: String!) {
          updateAiSessionTitle(id: $id, title: $title) {
            ${this.sessionFields}
          }
        }`,
        { id, title }
      )
      .pipe(map((data) => data.updateAiSessionTitle));
  }

  updateSessionWorkingDirectory(id: string, workingDirectory: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionWorkingDirectory: AiSession }>(
        `mutation UpdateAiSessionWorkingDirectory($id: ID!, $workingDirectory: String!) {
          updateAiSessionWorkingDirectory(id: $id, workingDirectory: $workingDirectory) {
            ${this.sessionFields}
          }
        }`,
        { id, workingDirectory }
      )
      .pipe(map((data) => data.updateAiSessionWorkingDirectory));
  }

  updateSessionProvider(id: string, provider: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionProvider: AiSession }>(
        `mutation UpdateAiSessionProvider($id: ID!, $provider: String!) {
          updateAiSessionProvider(id: $id, provider: $provider) {
            ${this.sessionFields}
          }
        }`,
        { id, provider }
      )
      .pipe(map((data) => data.updateAiSessionProvider));
  }

  archiveSession(id: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionArchived: AiSession }>(
        `mutation UpdateAiSessionArchived($id: ID!) {
          updateAiSessionArchived(id: $id) {
            ${this.sessionFields}
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.updateAiSessionArchived));
  }

  // ── Messages ──────────────────────────────────────────────────────────

  listMessages(sessionId: string, limit?: number, offset?: number): Observable<AiMessage[]> {
    const query = `query ListMessages($sessionId: ID!, $limit: Int, $offset: Int) {
      listAiMessages(sessionId: $sessionId, limit: $limit, offset: $offset) {
        id sessionId role content toolCalls
        finishReason inputTokens outputTokens costCents createdAt
      }
    }`;

    return this.gql
      .queryPreferLocalHost<{ listAiMessages: AiMessage[] }>(query, query, { sessionId, limit, offset })
      .pipe(map((data) => data.listAiMessages));
  }

  sendRelayMessage(input: { sessionId: string; content: string; provider?: string; model?: string; workingDirectory?: string }): Observable<{ success: boolean; relayId: string; desktopNodeId: string }> {
    return this.gql
      .mutate<{ sendRelayChatMessage: { success: boolean; relayId: string; desktopNodeId: string } }>(
        `mutation SendRelayChatMessage($input: RelayChatMessageInput!) {
          sendRelayChatMessage(input: $input) {
            success relayId desktopNodeId
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.sendRelayChatMessage));
  }

  createChatAttachment(input: { sessionId?: string; originalName: string; contentType: string; dataBase64: string }): Observable<ChatAttachment> {
    return this.gql
      .mutate<{ createChatAttachment: ChatAttachment }>(
        `mutation CreateChatAttachment($input: ChatAttachmentInput!) {
          createChatAttachment(input: $input) {
            id sessionId originalName contentType size status uploadedAt localPath
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createChatAttachment));
  }

  // sendAiChatMessage (forward path) removed by Phase 2 task 05 of multi-user-saas plan.
  // Use sendRelayChatMessage (above) — it routes through ChatRelayDO to the user's
  // registered host, which is multi-tenant safe.

  stopAiGeneration(sessionId: string): Observable<boolean> {
    return this.gql
      .mutate<{ cancelAiGeneration: boolean }>(
        `mutation CancelAiGeneration($sessionId: ID!) {
          cancelAiGeneration(sessionId: $sessionId)
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.cancelAiGeneration));
  }

  // ── Message Subscriptions ─────────────────────────────────────────────

  onMessageDelta(sessionId: string): Observable<AiMessageDelta> {
    return this.gql
      .subscribe<{ aiMessageDelta: AiMessageDelta }>(
        `subscription OnMessageDelta($sessionId: ID!) {
          aiMessageDelta(sessionId: $sessionId) {
            sessionId messageId delta finishReason
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.aiMessageDelta));
  }

  onNewMessage(sessionId: string): Observable<AiMessage> {
    return this.gql
      .subscribe<{ aiMessageCreated: AiMessage }>(
        `subscription OnNewMessage($sessionId: ID!) {
          aiMessageCreated(sessionId: $sessionId) {
            id sessionId role content toolCalls
            finishReason inputTokens outputTokens costCents createdAt
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.aiMessageCreated));
  }

  // onAiChatDelta / onAiChatComplete (forward subscriptions) removed by Phase 2 task 05.
  // Use onRelayChatDelta + onRelayChatMessage (relay-WS via ChatRelayDO) for
  // streaming + completion.

  /** Live streaming deltas for ONE relay chat response, keyed by the `relayId` returned from
   *  `sendRelayMessage`. (The worker has no session-level delta subscription — deltas are per-request.) */
  onRelayChatDelta(relayId: string): Observable<RelayChatDeltaMsg> {
    return this.gql
      .subscribe<{ onRelayChatDelta: RelayChatDeltaMsg }>(
        `subscription OnRelayChatDelta($relayId: String!) {
          onRelayChatDelta(relayId: $relayId) {
            relayId sessionId delta chunkType isFinal
          }
        }`,
        { relayId }
      )
      .pipe(map((data) => data.onRelayChatDelta));
  }

  /** Completed relay chat messages for a session (user + assistant), as they are persisted. */
  onRelayChatMessage(sessionId: string): Observable<RelayChatMessageMsg> {
    return this.gql
      .subscribe<{ onRelayChatMessage: RelayChatMessageMsg }>(
        `subscription OnRelayChatMessage($sessionId: String!) {
          onRelayChatMessage(sessionId: $sessionId) {
            relayId sessionId role content
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.onRelayChatMessage));
  }

  // ── Tools ─────────────────────────────────────────────────────────────

  listToolDefinitions(category?: string): Observable<ToolDefinition[]> {
    return this.gql
      .query<{ toolDefinitions: ToolDefinition[] }>(
        `query ListToolDefinitions($category: String) {
          toolDefinitions(category: $category) {
            id slug name description category
            parametersSchema executor requiresApproval isEnabled
          }
        }`,
        { category }
      )
      .pipe(map((data) => data.toolDefinitions));
  }

  approveToolExecution(id: string): Observable<ToolExecution> {
    return this.gql
      .mutate<{ approveToolExecution: ToolExecution }>(
        `mutation ApproveToolExecution($id: ID!) {
          approveToolExecution(id: $id) {
            id status updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.approveToolExecution));
  }

  rejectToolExecution(id: string): Observable<ToolExecution> {
    return this.gql
      .mutate<{ rejectToolExecution: ToolExecution }>(
        `mutation RejectToolExecution($id: ID!) {
          rejectToolExecution(id: $id) {
            id status updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.rejectToolExecution));
  }

  cancelToolExecution(id: string): Observable<ToolExecution> {
    return this.gql
      .mutate<{ cancelToolExecution: ToolExecution }>(
        `mutation CancelToolExecution($id: ID!) {
          cancelToolExecution(id: $id) {
            id status updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.cancelToolExecution));
  }

  // ── Providers ─────────────────────────────────────────────────────────

  upsertProviderConfig(input: Partial<ProviderConfig>): Observable<ProviderConfig> {
    return this.gql
      .mutate<{ upsertProviderConfig: ProviderConfig }>(
        `mutation UpsertProviderConfig($input: UpsertProviderConfigInput!) {
          upsertProviderConfig(input: $input) {
            id provider cliPath apiKey defaultModel settings isAvailable updatedAt
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.upsertProviderConfig));
  }

  deleteProviderConfig(provider: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteProviderConfig: boolean }>(
        `mutation DeleteProviderConfig($provider: String!) {
          deleteProviderConfig(provider: $provider)
        }`,
        { provider }
      )
      .pipe(map((data) => data.deleteProviderConfig));
  }

  // ── Desktop Nodes ─────────────────────────────────────────────────────

  listDesktopNodes(): Observable<DesktopNode[]> {
    return this.gql
      .query<{ listDesktopNodes: DesktopNode[] }>(
        `query ListDesktopNodes {
          listDesktopNodes {
            id hostname os arch version status
            capabilities lastHeartbeatAt createdAt
          }
        }`
      )
      .pipe(map((data) => data.listDesktopNodes));
  }

  listProviderConfigs(): Observable<ProviderConfig[]> {
    return this.gql
      .query<{ listProviderConfigs: ProviderConfig[] }>(
        `query ListProviderConfigs {
          listProviderConfigs {
            id provider cliPath apiKey defaultModel settings isAvailable updatedAt
          }
        }`
      )
      .pipe(map((data) => data.listProviderConfigs));
  }

  detectCliTools(): Observable<DetectedCliTool[]> {
    const workerQuery = `query ListDetectedCliTools {
      listDetectedCliTools { provider command found path }
    }`;
    const hostQuery = `query DetectCliTools {
      detectCliTools { provider command found path }
    }`;

    return this.gql
      .queryPreferLocalHost<{
        listDetectedCliTools?: DetectedCliTool[];
        detectCliTools?: DetectedCliTool[];
      }>(workerQuery, hostQuery)
      .pipe(map((data) => data.detectCliTools ?? data.listDetectedCliTools ?? []));
  }

  getSetting(key: string): Observable<string> {
    const query = `query GetSetting($key: String!) {
      getSetting(key: $key)
    }`;

    return this.gql
      .queryPreferLocalHost<{ getSetting: string }>(query, query, { key })
      .pipe(map((data) => data.getSetting));
  }

  setSetting(key: string, value: string): Observable<boolean> {
    return this.gql
      .mutate<{ updateSetting: boolean }>(
        `mutation UpdateSetting($key: String!, $value: String!) {
          updateSetting(key: $key, value: $value)
        }`,
        { key, value }
      )
      .pipe(map((data) => data.updateSetting));
  }

  listPromptLibrary(): Observable<PromptLibraryEntry[]> {
    return this.gql
      .query<{ listPromptLibrary: PromptLibraryEntry[] }>(
        `query ListPromptLibrary {
          listPromptLibrary {
            id
            key
            name
            role
            scope
            version
            usedCount
            customised
            readOnly
            engineReads
          }
        }`
      )
      .pipe(map((data) => data.listPromptLibrary));
  }

  getPlannerPromptSettings(): Observable<PlannerPromptSettings> {
    return this.gql
      .query<{ getPlannerPromptSettings: PlannerPromptSettings }>(
        `query GetPlannerPromptSettings {
          getPlannerPromptSettings {
            schema
            development { worker reviewer workerNudge }
            planning { planner reviewer amendPlanner amendReviewer }
            smallMode { planner reviewer leafWrapper amendPlanner }
          }
        }`
      )
      .pipe(map((data) => data.getPlannerPromptSettings));
  }

  updatePlannerPromptSettings(input: PlannerPromptSettings): Observable<PlannerPromptSettings> {
    const payload = {
      development: input.development,
      planning: input.planning,
      ...(input.smallMode ? { smallMode: input.smallMode } : {}),
    };
    return this.gql
      .mutate<{ updatePlannerPromptSettings: PlannerPromptSettings }>(
        `mutation UpdatePlannerPromptSettings($input: PlannerPromptSettingsInput!) {
          updatePlannerPromptSettings(input: $input) {
            schema
            development { worker reviewer workerNudge }
            planning { planner reviewer amendPlanner amendReviewer }
            smallMode { planner reviewer leafWrapper amendPlanner }
          }
        }`,
        { input: payload }
      )
      .pipe(map((data) => data.updatePlannerPromptSettings));
  }

  // ── Agent Planner ────────────────────────────────────────────────────

  listAgentPlans(status?: string, runType?: 'planning' | 'development', onlyExisting = false): Observable<AgentPlan[]> {
    return this.gql
      .query<{ listAgentPlans: AgentPlan[] }>(
        `query ListAgentPlans($status: String, $runType: String, $onlyExisting: Boolean) {
          listAgentPlans(status: $status, runType: $runType, onlyExisting: $onlyExisting) {
            id runType title workspacePath planPath status workerSessionId reviewerSessionId
            workerProvider reviewerProvider currentPhaseId currentPhaseIndex error
            brief appScope docsScope referencePaths
            initiativeId initiativeStatus health validationConfig executorConfig briefingSessionId
            consecutiveNonPassPlanningRounds
            createdAt updatedAt
          }
        }`,
        { status, runType, onlyExisting }
      )
      .pipe(map((data) => data.listAgentPlans));
  }

  getAgentPlan(id: string): Observable<AgentPlanRun> {
    return this.gql
      .query<{ getAgentPlan: AgentPlanRun }>(
        `query GetAgentPlan($id: ID!) {
          getAgentPlan(id: $id) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.getAgentPlan));
  }

  getPlanCheck(planId: string, phaseId?: string): Observable<string | null> {
    return this.gql
      .query<{ getPlanCheck: string | null }>(
        `query GetPlanCheck($planId: ID!, $phaseId: String) {
          getPlanCheck(planId: $planId, phaseId: $phaseId)
        }`,
        { planId, phaseId: phaseId ?? null }
      )
      .pipe(map((data) => data.getPlanCheck));
  }

  getTaskRun(planId: string, phaseId: string, taskId?: string): Observable<string | null> {
    return this.gql
      .query<{ getTaskRun: string | null }>(
        `query GetTaskRun($planId: ID!, $phaseId: String!, $taskId: String) {
          getTaskRun(planId: $planId, phaseId: $phaseId, taskId: $taskId)
        }`,
        { planId, phaseId, taskId: taskId ?? null }
      )
      .pipe(map((data) => data.getTaskRun));
  }

  getKlooDoctor(): Observable<string> {
    return this.gql
      .query<{ getKlooDoctor: string }>(`query GetKlooDoctor { getKlooDoctor }`)
      .pipe(map((data) => data.getKlooDoctor));
  }

  getKlooProbe(): Observable<string> {
    return this.gql
      .query<{ getKlooProbe: string }>(`query GetKlooProbe { getKlooProbe }`)
      .pipe(map((data) => data.getKlooProbe));
  }

  /** Full SDLC event timeline for one Initiative (planning-run + development-run events merged,
   *  oldest-first) — backs the console Events pane. */
  listInitiativeEvents(initiativeId: string): Observable<AgentPlanEvent[]> {
    return this.gql
      .query<{ listInitiativeEvents: AgentPlanEvent[] }>(
        `query ListInitiativeEvents($initiativeId: ID!) {
          listInitiativeEvents(initiativeId: $initiativeId) {
            id planId phaseId phaseIndex phaseTitle eventType actor category summary
            statusBefore statusAfter reason verdict taskId clarificationAttempt payloadJson createdAt
          }
        }`,
        { initiativeId }
      )
      .pipe(map((data) => data.listInitiativeEvents));
  }

  createAgentPlan(input: CreateAgentPlanInput): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ createAgentPlan: AgentPlanRun }>(
        `mutation CreateAgentPlan($input: CreateAgentPlanInput!) {
          createAgentPlan(input: $input) {
            ${this.agentPlanRunFields}
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createAgentPlan));
  }

  // ── Briefing loop (overhaul P4) ───────────────────────────────────────
  // Thin GraphQL clients over the Phase 03 relay mutations (each a desktopRpc pass-through to a
  // Phase 01/02 host method). Variable names match worker/schema/johnnyone-ai.graphql.

  /** Create an Initiative in briefing (host `create_briefing_run`). */
  createBriefingInitiative(input: CreateBriefingInput): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ createBriefingInitiative: AgentPlanRun }>(
        `mutation CreateBriefingInitiative($input: CreateBriefingInput!) {
          createBriefingInitiative(input: $input) {
            ${this.agentPlanRunFields}
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createBriefingInitiative));
  }

  /** Accept the brief → flips the same Initiative briefing→planning (host `accept_brief`). */
  acceptInitiativeBrief(input: AcceptBriefInput): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ acceptInitiativeBrief: AgentPlanRun }>(
        `mutation AcceptInitiativeBrief($input: AcceptBriefInput!) {
          acceptInitiativeBrief(input: $input) {
            ${this.agentPlanRunFields}
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.acceptInitiativeBrief));
  }

  /** Record a ▤ Reference host path on a briefing Initiative (host `add_initiative_reference_path`). */
  addInitiativeReferencePath(initiativeId: string, path: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ addInitiativeReferencePath: AgentPlanRun }>(
        `mutation AddInitiativeReferencePath($initiativeId: ID!, $path: String!) {
          addInitiativeReferencePath(initiativeId: $initiativeId, path: $path) {
            ${this.agentPlanRunFields}
          }
        }`,
        { initiativeId, path }
      )
      .pipe(map((data) => data.addInitiativeReferencePath));
  }

  /** Upload a briefing attachment chunk into <id>/attachments/ (host `initiative_upload_chunk`). */
  initiativeUploadChunk(
    initiativeId: string,
    path: string,
    chunkIndex: number,
    totalChunks: number,
    dataBase64: string,
    done: boolean
  ): Observable<UploadChunkResult> {
    return this.gql
      .mutate<{ initiativeUploadChunk: UploadChunkResult }>(
        `query InitiativeUploadChunk($initiativeId: ID!, $path: String!, $chunkIndex: Int!, $totalChunks: Int!, $dataBase64: String!, $done: Boolean!) {
          initiativeUploadChunk(initiativeId: $initiativeId, path: $path, chunkIndex: $chunkIndex, totalChunks: $totalChunks, dataBase64: $dataBase64, done: $done) {
            path received complete
          }
        }`,
        { initiativeId, path, chunkIndex, totalChunks, dataBase64, done }
      )
      .pipe(map((data) => data.initiativeUploadChunk));
  }

  startAgentPlan(id: string, phaseId?: string, phaseRunMode?: 'continue' | 'single'): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ startAgentPlan: AgentPlanRun }>(
        `mutation StartAgentPlan($id: ID!, $phaseId: String, $phaseRunMode: String) {
          startAgentPlan(id: $id, phaseId: $phaseId, phaseRunMode: $phaseRunMode) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, phaseId: phaseId || null, phaseRunMode: phaseRunMode || null }
      )
      .pipe(map((data) => data.startAgentPlan));
  }

  /** Unified run/resume: (re)start a development run at `phaseId` (omit ⇒ current/paused phase)
   * with an OPTIONAL comment/guidance recorded + injected in the same action. Blanks → null so
   * a missing/empty comment is a pure re-run/resume; a whitespace-only comment is rejected
   * server-side. Reuses the shared `agentPlanRunFields` selection set. */
  runInitiativeFromPhase(
    id: string,
    phaseId?: string,
    phaseRunMode?: 'continue' | 'single',
    comment?: string,
  ): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ runInitiativeFromPhase: AgentPlanRun }>(
        `mutation RunInitiativeFromPhase($id: ID!, $phaseId: String, $phaseRunMode: String, $comment: String) {
          runInitiativeFromPhase(id: $id, phaseId: $phaseId, phaseRunMode: $phaseRunMode, comment: $comment) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, phaseId: phaseId || null, phaseRunMode: phaseRunMode || null, comment: comment || null }
      )
      .pipe(map((data) => data.runInitiativeFromPhase));
  }

  refreshAgentPlanPhases(id: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanPhasesRefresh: AgentPlanRun }>(
        `mutation UpdateAgentPlanPhasesRefresh($id: ID!) {
          updateAgentPlanPhasesRefresh(id: $id) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.updateAgentPlanPhasesRefresh));
  }

  updateAgentPlanTitle(id: string, title: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanTitle: AgentPlanRun }>(
        `mutation UpdateAgentPlanTitle($id: ID!, $title: String!) {
          updateAgentPlanTitle(id: $id, title: $title) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, title }
      )
      .pipe(map((data) => data.updateAgentPlanTitle));
  }

  /** Set/clear the app-repo path (`app_scope`) the docs-commit agent uses. Works on
   * an in-flight run; an empty value clears it (docs commit then skips). */
  updateAgentPlanAppScope(id: string, appScope: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanAppScope: AgentPlanRun }>(
        `mutation UpdateAgentPlanAppScope($id: ID!, $appScope: String) {
          updateAgentPlanAppScope(id: $id, appScope: $appScope) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, appScope }
      )
      .pipe(map((data) => data.updateAgentPlanAppScope));
  }

  /** Persist the Initiative's configurable validation lenses as a JSON string (overhaul P7, D1).
   * Pass `null` (or an empty string) to clear it — the desktop then resolves the default
   * product/qa/lead template. The desktop validates the JSON (parse + provider) before writing. */
  updateAgentPlanValidationConfig(
    id: string,
    config: string | null,
  ): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanValidationConfig: AgentPlanRun }>(
        `mutation UpdateAgentPlanValidationConfig($id: ID!, $config: String) {
          updateAgentPlanValidationConfig(id: $id, config: $config) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, config }
      )
      .pipe(map((data) => data.updateAgentPlanValidationConfig));
  }

  /**
   * Amend an approved planning run. Stashes the brief on the plan, switches
   * T1 into "edit mode", and re-runs the T1→T2 planning cycle. On T2 PASS
   * the amended plan state is committed to the plan's per-plan git repo.
   *
   * The GraphQL name is `updateAgentPlanAmend` (uses an `update-` mutation
   * prefix lokal's validator recognizes); we expose it as `amendAgentPlan`
   * here so callers read naturally.
   */
  amendAgentPlan(id: string, brief: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanAmend: AgentPlanRun }>(
        `mutation UpdateAgentPlanAmend($id: ID!, $brief: String!) {
          updateAgentPlanAmend(id: $id, brief: $brief) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, brief }
      )
      .pipe(map((data) => data.updateAgentPlanAmend));
  }

  stopAgentPlan(id: string): Observable<AgentPlanRun> {
    return this.agentPlanMutation('updateAgentPlanStopped', { id });
  }

  deleteAgentPlan(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteAgentPlan: boolean }>(
        `mutation DeleteAgentPlan($id: ID!) {
          deleteAgentPlan(id: $id)
        }`,
        { id }
      )
      .pipe(map((data) => data.deleteAgentPlan));
  }

  blockAgentPlan(id: string, reason: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanBlocked: AgentPlanRun }>(
        `mutation BlockAgentPlan($id: ID!, $reason: String!) {
          updateAgentPlanBlocked(id: $id, reason: $reason) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, reason }
      )
      .pipe(map((data) => data.updateAgentPlanBlocked));
  }

  manualPassAgentPhase(id: string, phaseId: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPhaseManualPass: AgentPlanRun }>(
        `mutation ManualPassAgentPhase($id: ID!, $phaseId: String!) {
          updateAgentPhaseManualPass(id: $id, phaseId: $phaseId) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, phaseId }
      )
      .pipe(map((data) => data.updateAgentPhaseManualPass));
  }

  sendAgentFeedbackToWorker(id: string): Observable<AgentPlanRun> {
    return this.agentPlanMutation('sendAgentFeedbackToWorker', { id });
  }

  rerunAgentReviewer(id: string): Observable<AgentPlanRun> {
    return this.agentPlanMutation('retryAgentReviewer', { id });
  }

  browseHostDirectory(path: string): Observable<HostFileEntry[]> {
    return this.gql
      .query<{ browseHostDirectory: HostFileEntry[] }>(
        `query BrowseHostDirectory($path: String!) {
          browseHostDirectory(path: $path) {
            path name kind status size
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.browseHostDirectory));
  }

  validateWorkspacePlan(workspacePath: string, planPath: string): Observable<WorkspaceValidation> {
    return this.gql
      .query<{ validateWorkspacePlan: WorkspaceValidation }>(
        `query ValidateWorkspacePlan($workspacePath: String!, $planPath: String!) {
          validateWorkspacePlan(workspacePath: $workspacePath, planPath: $planPath) {
            valid workspacePath planPath title phaseCount taskCount error
          }
        }`,
        { workspacePath, planPath }
      )
      .pipe(map((data) => data.validateWorkspacePlan));
  }

  listWorkspaceFiles(planId: string, mode: 'changed' | 'all', path?: string): Observable<HostFileEntry[]> {
    return this.gql
      .query<{ listWorkspaceFiles: HostFileEntry[] }>(
        `query ListWorkspaceFiles($planId: ID!, $mode: String!, $path: String) {
          listWorkspaceFiles(planId: $planId, mode: $mode, path: $path) {
            path name kind status size
          }
        }`,
        { planId, mode, path }
      )
      .pipe(map((data) => data.listWorkspaceFiles));
  }

  gitFileView(planId: string, path?: string): Observable<GitFilesView> {
    return this.gql
      .query<{ gitFileView: GitFilesView }>(
        `query GitFileView($planId: ID!, $path: String) {
          gitFileView(planId: $planId, path: $path) {
            path repoRoot branch clean
            entries { path name kind status size }
          }
        }`,
        { planId, path }
      )
      .pipe(map((data) => data.gitFileView));
  }

  runGitAction(planId: string, input: { path?: string; action: 'fetch' | 'pull' | 'push' | 'commit'; message?: string }): Observable<GitActionResult> {
    return this.gql
      .mutate<{ updateGitAction: GitActionResult }>(
        `mutation RunGitAction($planId: ID!, $path: String, $action: String!, $message: String) {
          updateGitAction(planId: $planId, path: $path, action: $action, message: $message) {
            success output
          }
        }`,
        { planId, path: input.path || null, action: input.action, message: input.message || null }
      )
      .pipe(map((data) => data.updateGitAction));
  }

  readHostFile(planId: string, path: string): Observable<HostFileContent> {
    return this.gql
      .query<{ readHostFile: HostFileContent }>(
        `query ReadHostFile($planId: ID!, $path: String!) {
          readHostFile(planId: $planId, path: $path) {
            path name kind contentType encoding content size
          }
        }`,
        { planId, path }
      )
      .pipe(map((data) => data.readHostFile));
  }

  getWorkspaceFileDiff(planId: string, path: string): Observable<WorkspaceFileDiff> {
    return this.gql
      .query<{ getWorkspaceFileDiff: WorkspaceFileDiff }>(
        `query GetWorkspaceFileDiff($planId: ID!, $path: String!) {
          getWorkspaceFileDiff(planId: $planId, path: $path) {
            path diff
          }
        }`,
        { planId, path }
      )
      .pipe(map((data) => data.getWorkspaceFileDiff));
  }

  /** Whole-tree working-tree diff of the repo at `path` (a session's workingDirectory) — overhaul
   * P7, D7/D10. Scope-gated (`files:read`) + path-guarded on the worker/host; a non-repo/clean path
   * resolves to `{ clean:true, files:[] }` (no error). One round-trip, per-file +/- counts + hunks. */
  gitDiff(path: string): Observable<GitDiffView> {
    return this.gql
      .query<{ gitDiff: GitDiffView }>(
        `query GitDiff($path: String!) {
          gitDiff(path: $path) {
            repoRoot branch clean
            files { path oldPath additions deletions binary diff }
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.gitDiff));
  }

  // ── File manager (overhaul P2) — relay-RPC via the Phase-02 worker GraphQL surface. ──────────
  // Auth (JWT Bearer + tenant/user headers) is attached centrally by graphql-client.ts; these
  // methods inherit it. `content`/`dataBase64` are forwarded, never logged.
  listDir(path: string): Observable<DirListing> {
    return this.gql
      .query<{ filesListDir: DirListing }>(
        `query FilesListDir($path: String!) {
          filesListDir(path: $path) {
            path root entries { name path kind size modified }
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesListDir));
  }

  readFile(path: string): Observable<FileContent> {
    return this.gql
      .query<{ filesRead: FileContent }>(
        `query FilesRead($path: String!) {
          filesRead(path: $path) {
            path name contentType encoding content size
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesRead));
  }

  writeFile(
    path: string,
    content: string,
    encoding: 'utf8' | 'base64' = 'utf8'
  ): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesWrite: FileOpResult }>(
        `query FilesWrite($path: String!, $content: String!, $encoding: String) {
          filesWrite(path: $path, content: $content, encoding: $encoding) {
            path ok
          }
        }`,
        { path, content, encoding }
      )
      .pipe(map((data) => data.filesWrite));
  }

  mkdir(path: string): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesMkdir: FileOpResult }>(
        `query FilesMkdir($path: String!) {
          filesMkdir(path: $path) {
            path ok
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesMkdir));
  }

  rename(from: string, to: string): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesRename: FileOpResult }>(
        `query FilesRename($from: String!, $to: String!) {
          filesRename(from: $from, to: $to) {
            path ok
          }
        }`,
        { from, to }
      )
      .pipe(map((data) => data.filesRename));
  }

  deleteFile(path: string): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesDelete: FileOpResult }>(
        `query FilesDelete($path: String!) {
          filesDelete(path: $path) {
            path ok
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesDelete));
  }

  uploadChunk(
    path: string,
    chunkIndex: number,
    totalChunks: number,
    dataBase64: string,
    done: boolean
  ): Observable<UploadChunkResult> {
    return this.gql
      .mutate<{ filesUploadChunk: UploadChunkResult }>(
        `query FilesUploadChunk($path: String!, $chunkIndex: Int!, $totalChunks: Int!, $dataBase64: String!, $done: Boolean!) {
          filesUploadChunk(path: $path, chunkIndex: $chunkIndex, totalChunks: $totalChunks, dataBase64: $dataBase64, done: $done) {
            path received complete
          }
        }`,
        { path, chunkIndex, totalChunks, dataBase64, done }
      )
      .pipe(map((data) => data.filesUploadChunk));
  }

  // Deterministic one-shot terminal capture (overhaul P2). The live stream still uses
  // relay-terminal.service.ts; this request/response read is for deterministic reads (tests/automation).
  captureTerminal(sessionId: string, historyLines?: number): Observable<TerminalScreen> {
    return this.gql
      .query<{ captureTerminal: TerminalScreen }>(
        `query CaptureTerminal($sessionId: ID!, $historyLines: Int) {
          captureTerminal(sessionId: $sessionId, historyLines: $historyLines) {
            sessionId tmuxSessionName paneId cursor content
            cursorX cursorY historyLines rows cols status
          }
        }`,
        { sessionId, historyLines }
      )
      .pipe(map((data) => data.captureTerminal));
  }

  registerDesktopNode(input: Partial<DesktopNode>): Observable<DesktopNode> {
    return this.gql
      .mutate<{ registerDesktopNode: DesktopNode }>(
        `mutation RegisterDesktopNode($input: RegisterDesktopNodeInput!) {
          registerDesktopNode(input: $input) {
            id hostname os arch version status
            capabilities lastHeartbeatAt createdAt
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.registerDesktopNode));
  }

  updateDesktopNodeStatus(id: string, status: DesktopNode['status']): Observable<DesktopNode> {
    return this.gql
      .mutate<{ updateDesktopNodeStatus: DesktopNode }>(
        `mutation UpdateDesktopNodeStatus($id: ID!, $status: String!) {
          updateDesktopNodeStatus(id: $id, status: $status) {
            id status lastHeartbeatAt
          }
        }`,
        { id, status }
      )
      .pipe(map((data) => data.updateDesktopNodeStatus));
  }

  // ── Usage ─────────────────────────────────────────────────────────────

  getUsageSummary(from?: string, to?: string): Observable<AiUsageSummary> {
    return this.gql
      .query<{ aiUsageSummary: AiUsageSummary }>(
        `query GetUsageSummary($from: String, $to: String) {
          aiUsageSummary(from: $from, to: $to) {
            totalInputTokens totalOutputTokens totalCost
            sessionCount messageCount
          }
        }`,
        { from, to }
      )
      .pipe(map((data) => data.aiUsageSummary));
  }

  // ── API keys (M2M) ────────────────────────────────────────────────────
  // JWT-only management surface against the worker. create returns the secret
  // exactly once; list/revoke return metadata only (no secret).

  private readonly apiKeyFields = `
    id name keyPrefix scopes lastUsedAt expiresAt revokedAt createdAt
  `;

  listApiKeys(): Observable<ApiKey[]> {
    return this.gql
      .query<{ listApiKeys: ApiKey[] }>(
        `query ListApiKeys { listApiKeys { ${this.apiKeyFields} } }`
      )
      .pipe(map((data) => data.listApiKeys));
  }

  createApiKey(input: CreateApiKeyInput): Observable<CreateApiKeyResult> {
    return this.gql
      .mutate<{ createApiKey: CreateApiKeyResult }>(
        `mutation CreateApiKey($input: CreateApiKeyInput!) {
          createApiKey(input: $input) {
            apiKey { ${this.apiKeyFields} }
            secret
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createApiKey));
  }

  revokeApiKey(id: string): Observable<ApiKey> {
    return this.gql
      .mutate<{ revokeApiKey: ApiKey }>(
        `mutation RevokeApiKey($id: ID!) {
          revokeApiKey(id: $id) { ${this.apiKeyFields} }
        }`,
        { id }
      )
      .pipe(map((data) => data.revokeApiKey));
  }

  deleteApiKey(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteApiKey: boolean }>(
        `mutation DeleteApiKey($id: ID!) {
          deleteApiKey(id: $id)
        }`,
        { id }
      )
      .pipe(map((data) => data.deleteApiKey));
  }

  private agentPlanMutation(
    field: 'startAgentPlan' | 'updateAgentPlanStopped' | 'sendAgentFeedbackToWorker' | 'retryAgentReviewer',
    variables: { id: string }
  ): Observable<AgentPlanRun> {
    return this.gql
      .mutate<Record<typeof field, AgentPlanRun>>(
        `mutation AgentPlanMutation($id: ID!) {
          ${field}(id: $id) {
            ${this.agentPlanRunFields}
          }
        }`,
        variables
      )
      .pipe(map((data) => data[field]));
  }

  // ── The test gate (migrations 023 / 025) ──────────────────────────────

  /** Set — or with `clear`, remove — the gate at one level. `phaseId` null writes the PLAN's. */
  updateAgentPlanTestCommands(
    id: string,
    phaseId: string | null,
    commands: string[],
    clear = false,
  ): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanTestCommands: AgentPlanRun }>(
        `mutation UpdateAgentPlanTestCommands($id: ID!, $phaseId: String, $commands: [String!]!, $clear: Boolean) {
          updateAgentPlanTestCommands(id: $id, phaseId: $phaseId, commands: $commands, clear: $clear) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, phaseId, commands, clear }
      )
      .pipe(map((data) => data.updateAgentPlanTestCommands));
  }

  /** Run a phase's (or the plan's) effective gate ONCE, synchronously, and report the outcome.
   *  Both numbers come back: the exit code (the score) and the advisory collected count. */
  runAgentPlanTestGate(id: string, phaseId?: string | null): Observable<TestGateRun> {
    return this.gql
      .mutate<{ runAgentPlanTestGate: TestGateRun }>(
        `mutation RunAgentPlanTestGate($id: ID!, $phaseId: String) {
          runAgentPlanTestGate(id: $id, phaseId: $phaseId) {
            gateDeclared exitCode collected command commandsRun output truncated
          }
        }`,
        { id, phaseId: phaseId ?? null }
      )
      .pipe(map((data) => data.runAgentPlanTestGate));
  }

  // ── The workflow arena (migration 026) ────────────────────────────────

  /** All three filters are optional and independent; absent means "any". */
  workflows(filter: { repo?: string; phaseType?: string; family?: string }): Observable<Workflow[]> {
    return this.gql
      .query<{ workflows: Workflow[] }>(
        `query Workflows($repo: String, $phaseType: String, $family: String) {
          workflows(repo: $repo, phaseType: $phaseType, family: $family) {
            ${this.workflowFields}
          }
        }`,
        { repo: filter.repo, phaseType: filter.phaseType, family: filter.family }
      )
      .pipe(map((data) => data.workflows));
  }

  workflow(id: string): Observable<Workflow> {
    return this.gql
      .query<{ workflow: Workflow }>(
        `query Workflow($id: ID!) {
          workflow(id: $id) { ${this.workflowFields} }
        }`,
        { id }
      )
      .pipe(map((data) => data.workflow));
  }

  workflowRuns(workflowId: string): Observable<WorkflowRun[]> {
    return this.gql
      .query<{ workflowRuns: WorkflowRun[] }>(
        `query WorkflowRuns($workflowId: ID!) {
          workflowRuns(workflowId: $workflowId) { ${this.workflowRunFields} }
        }`,
        { workflowId }
      )
      .pipe(map((data) => data.workflowRuns));
  }

  /** The halt, as data. Read it before offering to start a run — the host refuses while halted. */
  workflowSearchState(
    repo: string,
    phaseType: string,
    family: string,
  ): Observable<WorkflowSearchState> {
    return this.gql
      .query<{ workflowSearchState: WorkflowSearchState }>(
        `query WorkflowSearchState($repo: String!, $phaseType: String!, $family: String!) {
          workflowSearchState(repo: $repo, phaseType: $phaseType, family: $family) {
            ${this.workflowSearchStateFields}
          }
        }`,
        { repo, phaseType, family }
      )
      .pipe(map((data) => data.workflowSearchState));
  }

  createWorkflowVersion(input: {
    repo: string;
    phaseType: string;
    family: string;
    parentId?: string | null;
    role: string;
    instruction: string;
    contract: string;
    hop: string;
    fanOut: number;
  }): Observable<Workflow> {
    return this.gql
      .mutate<{ createWorkflowVersion: Workflow }>(
        `mutation CreateWorkflowVersion($repo: String!, $phaseType: String!, $family: String!, $parentId: String, $role: String!, $instruction: String!, $contract: String!, $hop: String!, $fanOut: Int!) {
          createWorkflowVersion(repo: $repo, phaseType: $phaseType, family: $family, parentId: $parentId, role: $role, instruction: $instruction, contract: $contract, hop: $hop, fanOut: $fanOut) {
            ${this.workflowFields}
          }
        }`,
        { ...input, parentId: input.parentId ?? null }
      )
      .pipe(map((data) => data.createWorkflowVersion));
  }

  /** REFUSED by the host while the family's search is halted — that is what the halt means. */
  startWorkflowRun(input: {
    workflowId: string;
    planId?: string | null;
    seed?: number;
    seatsHeld: string;
    fleetBusy: string;
  }): Observable<WorkflowRun> {
    return this.gql
      .mutate<{ startWorkflowRun: WorkflowRun }>(
        `mutation StartWorkflowRun($workflowId: ID!, $planId: String, $seed: Int, $seatsHeld: String!, $fleetBusy: String!) {
          startWorkflowRun(workflowId: $workflowId, planId: $planId, seed: $seed, seatsHeld: $seatsHeld, fleetBusy: $fleetBusy) {
            ${this.workflowRunFields}
          }
        }`,
        {
          workflowId: input.workflowId,
          planId: input.planId ?? null,
          seed: input.seed ?? 0,
          seatsHeld: input.seatsHeld,
          fleetBusy: input.fleetBusy,
        }
      )
      .pipe(map((data) => data.startWorkflowRun));
  }

  /** `gateExit` is non-null: a missing score must be an error, not a 0 that reads as a pass. */
  recordWorkflowRunGate(
    runId: string,
    gateExit: number,
    gateCollected?: number | null,
  ): Observable<WorkflowRun> {
    return this.gql
      .mutate<{ recordWorkflowRunGate: WorkflowRun }>(
        `mutation RecordWorkflowRunGate($runId: ID!, $gateExit: Int!, $gateCollected: Int) {
          recordWorkflowRunGate(runId: $runId, gateExit: $gateExit, gateCollected: $gateCollected) {
            ${this.workflowRunFields}
          }
        }`,
        { runId, gateExit, gateCollected: gateCollected ?? null }
      )
      .pipe(map((data) => data.recordWorkflowRunGate));
  }

  /** No `comparable` argument exists, deliberately: the host computes it from the two runs, so a
   *  client cannot assert a comparability it does not have. */
  recordWorkflowPref(
    winnerRunId: string,
    loserRunId: string,
    reason?: string | null,
  ): Observable<WorkflowPref> {
    return this.gql
      .mutate<{ recordWorkflowPref: WorkflowPref }>(
        `mutation RecordWorkflowPref($winnerRunId: ID!, $loserRunId: ID!, $reason: String) {
          recordWorkflowPref(winnerRunId: $winnerRunId, loserRunId: $loserRunId, reason: $reason) {
            ${this.workflowPrefFields}
          }
        }`,
        { winnerRunId, loserRunId, reason: reason ?? null }
      )
      .pipe(map((data) => data.recordWorkflowPref));
  }

  /** PROMOTE. `confirmed` is explicit and non-null because this is the arena's one irreversible
   *  step: a call that forgot its arguments must be a validation error, not a promotion. */
  updateWorkflowChampion(
    runId: string,
    prefId: string | null,
    confirmed: boolean,
  ): Observable<Workflow> {
    return this.gql
      .mutate<{ updateWorkflowChampion: Workflow }>(
        `mutation UpdateWorkflowChampion($runId: ID!, $prefId: String, $confirmed: Boolean!) {
          updateWorkflowChampion(runId: $runId, prefId: $prefId, confirmed: $confirmed) {
            ${this.workflowFields}
          }
        }`,
        { runId, prefId, confirmed }
      )
      .pipe(map((data) => data.updateWorkflowChampion));
  }

  /** REJECT, with a reason (the host also refuses a blank one). Returns the resulting search
   *  state, so the caller already holds the halt instead of having to ask again. */
  recordWorkflowRejection(runId: string, reason: string): Observable<WorkflowSearchState> {
    return this.gql
      .mutate<{ recordWorkflowRejection: WorkflowSearchState }>(
        `mutation RecordWorkflowRejection($runId: ID!, $reason: String!) {
          recordWorkflowRejection(runId: $runId, reason: $reason) {
            ${this.workflowSearchStateFields}
          }
        }`,
        { runId, reason }
      )
      .pipe(map((data) => data.recordWorkflowRejection));
  }
}
