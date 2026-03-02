import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { map } from 'rxjs/operators';
import { GraphQLClient } from './graphql-client';
import { AiSession, CreateAiSessionInput } from '../models/ai-session.model';
import { AiMessage, AiMessageDelta, SendAgentMessageInput } from '../models/ai-message.model';
import { ToolDefinition, ToolExecution } from '../models/tool.model';
import { ProviderConfig, AiUsageSummary } from '../models/provider.model';
import { DesktopNode } from '../models/desktop-node.model';

@Injectable({ providedIn: 'root' })
export class JohnnyApiService {
  private readonly gql = inject(GraphQLClient);

  // ── Sessions ──────────────────────────────────────────────────────────

  listSessions(): Observable<AiSession[]> {
    return this.gql
      .query<{ aiSessions: AiSession[] }>(
        `query ListSessions {
          aiSessions {
            id userId title provider model status
            systemPrompt totalInputTokens totalOutputTokens totalCost
            createdAt updatedAt
          }
        }`
      )
      .pipe(map((data) => data.aiSessions));
  }

  getSession(id: string): Observable<AiSession> {
    return this.gql
      .query<{ aiSession: AiSession }>(
        `query GetSession($id: ID!) {
          aiSession(id: $id) {
            id userId title provider model status
            systemPrompt totalInputTokens totalOutputTokens totalCost
            createdAt updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.aiSession));
  }

  createSession(input: CreateAiSessionInput): Observable<AiSession> {
    return this.gql
      .mutate<{ createAiSession: AiSession }>(
        `mutation CreateSession($input: CreateAiSessionInput!) {
          createAiSession(input: $input) {
            id userId title provider model status
            systemPrompt totalInputTokens totalOutputTokens totalCost
            createdAt updatedAt
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createAiSession));
  }

  archiveSession(id: string): Observable<AiSession> {
    return this.gql
      .mutate<{ archiveAiSession: AiSession }>(
        `mutation ArchiveSession($id: ID!) {
          archiveAiSession(id: $id) {
            id status updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.archiveAiSession));
  }

  updateSessionTitle(id: string, title: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionTitle: AiSession }>(
        `mutation UpdateSessionTitle($id: ID!, $title: String!) {
          updateAiSessionTitle(id: $id, title: $title) {
            id title updatedAt
          }
        }`,
        { id, title }
      )
      .pipe(map((data) => data.updateAiSessionTitle));
  }

  // ── Messages ──────────────────────────────────────────────────────────

  listMessages(sessionId: string, limit?: number, offset?: number): Observable<AiMessage[]> {
    return this.gql
      .query<{ aiMessages: AiMessage[] }>(
        `query ListMessages($sessionId: ID!, $limit: Int, $offset: Int) {
          aiMessages(sessionId: $sessionId, limit: $limit, offset: $offset) {
            id sessionId role content toolCalls { id name input }
            toolCallId sourceChannel finishReason
            inputTokens outputTokens createdAt
          }
        }`,
        { sessionId, limit, offset }
      )
      .pipe(map((data) => data.aiMessages));
  }

  sendMessage(input: SendAgentMessageInput): Observable<AiMessage> {
    return this.gql
      .mutate<{ sendAgentMessage: AiMessage }>(
        `mutation SendMessage($input: SendAgentMessageInput!) {
          sendAgentMessage(input: $input) {
            id sessionId role content toolCalls { id name input }
            toolCallId sourceChannel finishReason
            inputTokens outputTokens createdAt
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.sendAgentMessage));
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
            id sessionId role content toolCalls { id name input }
            toolCallId sourceChannel finishReason
            inputTokens outputTokens createdAt
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.aiMessageCreated));
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
        `mutation UpsertProviderConfig($input: ProviderConfigInput!) {
          upsertProviderConfig(input: $input) {
            id provider model apiKeyRef baseUrl isDefault settings
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.upsertProviderConfig));
  }

  deleteProviderConfig(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteProviderConfig: boolean }>(
        `mutation DeleteProviderConfig($id: ID!) {
          deleteProviderConfig(id: $id)
        }`,
        { id }
      )
      .pipe(map((data) => data.deleteProviderConfig));
  }

  // ── Desktop Nodes ─────────────────────────────────────────────────────

  listDesktopNodes(): Observable<DesktopNode[]> {
    return this.gql
      .query<{ desktopNodes: DesktopNode[] }>(
        `query ListDesktopNodes {
          desktopNodes {
            id hostname os arch version status
            capabilities lastHeartbeatAt createdAt
          }
        }`
      )
      .pipe(map((data) => data.desktopNodes));
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
}
