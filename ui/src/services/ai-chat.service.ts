import { Injectable, inject, signal, computed } from '@angular/core';
import { Subscription } from 'rxjs';
import { JohnnyApiService } from './johnny-api.service';
import { AiSession, CreateAiSessionInput } from '../models/ai-session.model';
import { AiMessage, AiMessageDelta } from '../models/ai-message.model';

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
        this.subscribeToDeltas(session.id);
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
      toolCalls: [],
      createdAt: new Date().toISOString(),
    };

    this.messages.update((msgs) => [...msgs, optimisticMessage]);
    this.isStreaming.set(true);
    this.streamingContent = '';

    this.api.sendMessage({ sessionId: session.id, content }).subscribe({
      next: (message) => {
        // Replace the optimistic message with the real one
        this.messages.update((msgs) =>
          msgs.map((m) => (m.id === optimisticMessage.id ? message : m))
        );
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
   * Subscribe to streaming deltas for a session.
   */
  subscribeToDeltas(sessionId?: string): void {
    const id = sessionId ?? this.currentSession()?.id;
    if (!id) return;

    this.deltaSubscription?.unsubscribe();
    this.deltaSubscription = this.api.onMessageDelta(id).subscribe({
      next: (delta: AiMessageDelta) => {
        this.handleDelta(delta);
      },
      error: (err) => {
        console.error('[AiChatService] Delta subscription error:', err);
      },
    });
  }

  /**
   * Subscribe to new complete messages for a session.
   */
  subscribeToMessages(sessionId?: string): void {
    const id = sessionId ?? this.currentSession()?.id;
    if (!id) return;

    this.messageSubscription?.unsubscribe();
    this.messageSubscription = this.api.onNewMessage(id).subscribe({
      next: (message: AiMessage) => {
        this.handleNewMessage(message);
      },
      error: (err) => {
        console.error('[AiChatService] Message subscription error:', err);
      },
    });
  }

  /**
   * Create a new session and switch to it.
   */
  createNewSession(input?: CreateAiSessionInput): void {
    this.api.createSession(input ?? {}).subscribe({
      next: (session) => {
        this.loadSession(session.id);
      },
      error: (err) => {
        console.error('[AiChatService] Failed to create session:', err);
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

  private handleDelta(delta: AiMessageDelta): void {
    this.streamingContent += delta.delta;
    this.streamingMessageId.set(delta.messageId);
    this.isStreaming.set(true);

    // Update or insert the streaming assistant message
    this.messages.update((msgs) => {
      const existingIdx = msgs.findIndex((m) => m.id === delta.messageId);
      if (existingIdx >= 0) {
        const updated = [...msgs];
        updated[existingIdx] = {
          ...updated[existingIdx],
          content: this.streamingContent,
        };
        return updated;
      } else {
        return [
          ...msgs,
          {
            id: delta.messageId,
            sessionId: delta.sessionId,
            role: 'assistant' as const,
            content: this.streamingContent,
            toolCalls: [],
            createdAt: new Date().toISOString(),
          },
        ];
      }
    });

    if (delta.finishReason) {
      this.isStreaming.set(false);
      this.streamingMessageId.set(null);
      this.streamingContent = '';
    }
  }

  private handleNewMessage(message: AiMessage): void {
    this.messages.update((msgs) => {
      const existingIdx = msgs.findIndex((m) => m.id === message.id);
      if (existingIdx >= 0) {
        const updated = [...msgs];
        updated[existingIdx] = message;
        return updated;
      }
      return [...msgs, message];
    });

    if (message.role === 'assistant' && message.finishReason) {
      this.isStreaming.set(false);
      this.streamingMessageId.set(null);
      this.streamingContent = '';
    }
  }
}
