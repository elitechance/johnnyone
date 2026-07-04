import { Injectable, inject, signal, computed } from '@angular/core';
import { Subscription } from 'rxjs';
import {
  JohnnyApiService,
  RelayChatDeltaMsg,
  RelayChatMessageMsg,
} from './johnny-api.service';
import { AiSession } from '../models/ai-session.model';
import { AiMessage } from '../models/ai-message.model';

@Injectable({ providedIn: 'root' })
export class AiChatService {
  private readonly api = inject(JohnnyApiService);

  private deltaSubscription: Subscription | null = null;
  private messageSubscription: Subscription | null = null;
  private streamingContent = '';

  /** The currently active session. */
  readonly currentSession = signal<AiSession | null>(null);

  /** All messages in the current session. */
  readonly messages = signal<AiMessage[]>([]);

  /** Whether the assistant is currently streaming a response. */
  readonly isStreaming = signal<boolean>(false);

  /** The id of the message currently being streamed. */
  readonly streamingMessageId = signal<string | null>(null);

  /** Computed message count for convenience. */
  readonly messageCount = computed(() => this.messages().length);

  /**
   * Load a session by id and fetch its messages.
   * Subscribes to real-time deltas and new messages.
   */
  loadSession(id: string): void {
    this.teardownSubscriptions();
    this.messages.set([]);
    this.isStreaming.set(false);
    this.streamingMessageId.set(null);
    this.streamingContent = '';

    this.api.getSession(id).subscribe({
      next: (session) => {
        this.currentSession.set(session);
        this.fetchMessages(session.id);
        // Deltas are subscribed per-send (keyed by relayId); at load we only watch for
        // completed messages on this session.
        this.subscribeToMessages(session.id);
      },
      error: (err) => {
        console.error('[AiChatService] Failed to load session:', err);
      },
    });
  }

  /**
   * Send a user message in the current session.
   * Optimistically adds the message and starts streaming state.
   */
  sendMessage(content: string): void {
    const session = this.currentSession();
    if (!session) {
      console.error('[AiChatService] No active session');
      return;
    }

    const optimisticMessage: AiMessage = {
      id: `temp-${Date.now()}`,
      sessionId: session.id,
      role: 'user',
      content,
      inputTokens: 0,
      outputTokens: 0,
      costCents: 0,
      createdAt: new Date().toISOString(),
    };

    this.messages.update((msgs) => [...msgs, optimisticMessage]);
    this.isStreaming.set(true);
    this.streamingContent = '';

    this.api.sendRelayMessage({ sessionId: session.id, content }).subscribe({
      next: ({ relayId }) => {
        // The assistant's reply streams back keyed by THIS relayId (the worker has no
        // session-level delta subscription — deltas are per-request).
        this.subscribeToDeltas(relayId);
      },
      error: (err) => {
        console.error('[AiChatService] Failed to send message:', err);
        // Remove the optimistic message on failure
        this.messages.update((msgs) => msgs.filter((m) => m.id !== optimisticMessage.id));
        this.isStreaming.set(false);
      },
    });
  }

  /**
   * Subscribe to streaming deltas for ONE relay response (keyed by relayId).
   */
  subscribeToDeltas(relayId: string): void {
    if (!relayId) return;
    this.deltaSubscription?.unsubscribe();
    this.streamingContent = '';
    this.deltaSubscription = this.api.onRelayChatDelta(relayId).subscribe({
      next: (delta) => this.handleDelta(delta),
      error: (err) => {
        console.error('[AiChatService] Delta subscription error:', err);
      },
    });
  }

  /**
   * Subscribe to completed messages for a session (relay chat, via ChatRelayDO).
   */
  subscribeToMessages(sessionId?: string): void {
    const id = sessionId ?? this.currentSession()?.id;
    if (!id) return;

    this.messageSubscription?.unsubscribe();
    this.messageSubscription = this.api.onRelayChatMessage(id).subscribe({
      next: (message) => this.handleNewMessage(message),
      error: (err) => {
        console.error('[AiChatService] Message subscription error:', err);
      },
    });
  }

  /**
   * Clean up all subscriptions. Call when the service consumer is destroyed.
   */
  teardownSubscriptions(): void {
    this.deltaSubscription?.unsubscribe();
    this.deltaSubscription = null;
    this.messageSubscription?.unsubscribe();
    this.messageSubscription = null;
  }

  // ── Private Helpers ───────────────────────────────────────────────────

  private fetchMessages(sessionId: string): void {
    this.api.listMessages(sessionId).subscribe({
      next: (messages) => {
        this.messages.set(messages);
      },
      error: (err) => {
        console.error('[AiChatService] Failed to fetch messages:', err);
      },
    });
  }

  /** Stable id for the streamed/finalized assistant message of one relay response. */
  private assistantMessageId(relayId: string): string {
    return `relay-${relayId}-assistant`;
  }

  private handleDelta(delta: RelayChatDeltaMsg): void {
    // Skip control chunks (system/result/error) — only assistant text accumulates. The final
    // `onRelayChatMessage` reconciles the clean content, so minor noise self-corrects.
    const kind = (delta.chunkType || '').toLowerCase();
    if (kind === 'system' || kind === 'result' || kind === 'error') {
      if (delta.isFinal) this.finishStreaming();
      return;
    }
    this.streamingContent += delta.delta ?? '';
    const id = this.assistantMessageId(delta.relayId);
    this.streamingMessageId.set(id);
    this.isStreaming.set(true);

    this.messages.update((msgs) => {
      const existingIdx = msgs.findIndex((m) => m.id === id);
      if (existingIdx >= 0) {
        const updated = [...msgs];
        updated[existingIdx] = { ...updated[existingIdx], content: this.streamingContent };
        return updated;
      }
      return [
        ...msgs,
        {
          id,
          sessionId: delta.sessionId,
          role: 'assistant' as const,
          content: this.streamingContent,
          inputTokens: 0,
          outputTokens: 0,
          costCents: 0,
          createdAt: new Date().toISOString(),
        },
      ];
    });

    if (delta.isFinal) this.finishStreaming();
  }

  private handleNewMessage(message: RelayChatMessageMsg): void {
    // The user message is already shown optimistically — only reconcile assistant/system messages
    // (matched to their streamed placeholder by relayId) with the clean final content.
    if (message.role === 'user') return;
    const id = this.assistantMessageId(message.relayId);
    this.messages.update((msgs) => {
      const existingIdx = msgs.findIndex((m) => m.id === id);
      const finalized: AiMessage = {
        id,
        sessionId: message.sessionId,
        role: 'assistant',
        content: message.content,
        inputTokens: 0,
        outputTokens: 0,
        costCents: 0,
        createdAt: new Date().toISOString(),
      };
      if (existingIdx >= 0) {
        const updated = [...msgs];
        updated[existingIdx] = { ...updated[existingIdx], content: message.content };
        return updated;
      }
      return [...msgs, finalized];
    });
    this.finishStreaming();
  }

  private finishStreaming(): void {
    this.isStreaming.set(false);
    this.streamingMessageId.set(null);
    this.streamingContent = '';
  }
}
