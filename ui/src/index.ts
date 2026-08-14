// ── Models ───────────────────────────────────────────────────────────────────
export { AiSession, CreateAiSessionInput } from './models/ai-session.model';
export {
  AiMessage,
  AiMessageDelta,
  SendAgentMessageInput,
} from './models/ai-message.model';
export { ToolDefinition, ToolExecution, ToolCall } from './models/tool.model';
export {
  ProviderConfig,
  LlmModel,
  AiUsageSummary,
} from './models/provider.model';
export { DesktopNode, ChannelBinding } from './models/desktop-node.model';

// ── Services ─────────────────────────────────────────────────────────────────
export {
  GraphQLClient,
  GraphQLRequestError,
  GraphQLResponse,
  GraphQLError,
  GRAPHQL_API_URL,
  HOST_GRAPHQL_API_URL,
  GRAPHQL_WS_URL,
  GRAPHQL_EXTRA_HEADERS,
} from './services/graphql-client';
export {
  JohnnyApiService,
} from './services/johnny-api.service';
export type {
  CreateAiSessionInput as JohnnyCreateAiSessionInput,
  AiChatRunResult,
  AiChatComplete,
  DetectedCliTool,
  ApiKey,
  CreateApiKeyInput,
  CreateApiKeyResult,
  AgentPlan,
  AgentPlanPhase,
  AgentPlanTask,
  AgentPlanEvent,
  AgentPlanRun,
  CreateAgentPlanInput,
  CreateBriefingInput,
  AcceptBriefInput,
  HostFileContent,
  HostFileEntry,
  FileEntry,
  DirListing,
  FileContent,
  FileOpResult,
  UploadChunkResult,
  TmuxSession,
  PlannerPromptSettings,
  ChatAttachment,
  GitActionResult,
  GitFilesView,
  GitDiffFile,
  GitDiffView,
  WorkspaceFileDiff,
  WorkspaceValidation,
} from './services/johnny-api.service';
export { AiChatService } from './services/ai-chat.service';
export type {
  ChatBackend,
  ChatMessage,
  ChatSession,
  ChatDeltaEvent,
} from './services/chat-backend.interface';

// ── Components ───────────────────────────────────────────────────────────────
export { ChatWindowComponent } from './components/chat-window/chat-window.component';
export { MessageBubbleComponent } from './components/message-bubble/message-bubble.component';
export { MessageComposerComponent } from './components/message-composer/message-composer.component';
export { ToolExecutionCardComponent } from './components/tool-execution-card/tool-execution-card.component';
export { SessionListComponent } from './components/session-list/session-list.component';
export { ProviderSelectorComponent } from './components/provider-selector/provider-selector.component';
export { NodeStatusComponent } from './components/node-status/node-status.component';
export { StreamingTextComponent } from './components/streaming-text/streaming-text.component';
export { TerminalScreenComponent } from './components/terminal-screen/terminal-screen.component';
export { MarkdownViewComponent } from './components/markdown-view/markdown-view.component';
export { TranscriptViewComponent } from './components/transcript-view/transcript-view.component';
export { eventsToBlocks } from './components/transcript-view/transcript-blocks';
export type {
  TranscriptBlock,
  TranscriptBlockKind,
} from './components/transcript-view/transcript-blocks';
export { DiffViewComponent } from './components/diff-view/diff-view.component';
export {
  parseUnifiedDiff,
  langForPath,
  fileTotals,
} from './components/diff-view/diff-parse';
export type {
  DiffLine,
  DiffLineKind,
} from './components/diff-view/diff-parse';
export { StatusPillComponent } from './components/status-pill/status-pill.component';
export { LifecycleBarComponent } from './components/lifecycle-bar/lifecycle-bar.component';
export {
  statusMeta,
  healthMeta,
  stageIndex,
  stageFill,
  stageDescription,
  planningRoundOf,
  phaseNnOf,
  STATUS_META,
  HEALTH_META,
  LIFECYCLE_STAGES,
} from './lib/lifecycle-status';
export type { StatusMeta, LifecycleStage } from './lib/lifecycle-status';
export type { TerminalScreen } from './models/terminal.model';
export type { StreamEvent } from './models/stream-event.model';

// ── Render core ──────────────────────────────────────────────────────────────
export {
  parseMarkdown,
  hydrateMermaid,
  escapeHtml,
  highlightCode,
} from './lib/markdown-render';
