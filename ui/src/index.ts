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
  GRAPHQL_WS_URL,
  GRAPHQL_EXTRA_HEADERS,
} from './services/graphql-client';
export { JohnnyApiService } from './services/johnny-api.service';
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
