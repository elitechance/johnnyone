/**
 * Relay → GraphQL subscription bridge (live UI bugs 2, item 1).
 *
 * The web's plan chat listens through graphql-ws subscriptions:
 *  - `onRelayChatDelta(relayId)`   → pubsub topic `relay-chat-delta:<relayId>`
 *  - `onRelayChatMessage(sessionId)` → pubsub topic `relay-chat-message:<userId>:<sessionId>`
 *
 * Those topics live in lokal's generated `PubSubDO` (binding `PUBSUB_DO`, one DO per "room" = the
 * part of the topic after its last `:`). The desktop's `chat_delta` / `chat_message` envelopes reach
 * the ChatRelayDO over its WebSocket, which only re-broadcast them to raw relay sockets. Nothing
 * published them to the pubsub topics, so the subscriptions were acked and then never fed: the
 * reply showed only after the store-poll fallback. This module is that missing publish.
 */

/** The pubsub room for a topic: the same rule as lokal's generated `extractRoom`. */
export function pubsubRoom(topic: string): string {
  const idx = topic.lastIndexOf(':');
  return idx >= 0 ? topic.substring(idx + 1) : topic;
}

export interface RelayPublication {
  topic: string;
  data: Record<string, unknown>;
}

/**
 * The subscription publications for one desktop→client envelope. Pure.
 * - `chat_delta` → the delta topic of its relayId;
 * - `chat_message` → the message topic of the desktop's OWNER (`ownerUserId`, the socket's
 *   authenticated user — never a client-supplied id) and the session.
 * Anything else, or an envelope missing its keys, publishes nothing.
 */
export function relayPublications(
  envelope: { type?: string; relayId?: string; sessionId?: string; data?: unknown },
  ownerUserId: string | null | undefined,
): RelayPublication[] {
  const data = (envelope.data && typeof envelope.data === 'object' ? envelope.data : {}) as Record<string, unknown>;
  const relayId = String(data['relayId'] ?? envelope.relayId ?? '');
  const sessionId = String(data['sessionId'] ?? envelope.sessionId ?? '');
  if (envelope.type === 'chat_delta') {
    if (!relayId) return [];
    return [
      {
        topic: `relay-chat-delta:${relayId}`,
        data: {
          relayId,
          sessionId,
          delta: String(data['delta'] ?? ''),
          chunkType: String(data['chunkType'] ?? ''),
          isFinal: Boolean(data['isFinal']),
        },
      },
    ];
  }
  if (envelope.type === 'chat_message') {
    if (!sessionId || !ownerUserId) return [];
    return [
      {
        topic: `relay-chat-message:${ownerUserId}:${sessionId}`,
        data: {
          relayId,
          sessionId,
          role: String(data['role'] ?? 'assistant'),
          content: String(data['content'] ?? ''),
        },
      },
    ];
  }
  return [];
}

/** Publish one message to lokal's PubSubDO (the same POST the generated `pubsub.publish` makes). */
export async function publishToPubSub(env: Record<string, unknown>, pub: RelayPublication): Promise<void> {
  const ns = env['PUBSUB_DO'] as DurableObjectNamespace | undefined;
  if (!ns) return;
  const stub = ns.get(ns.idFromName(pubsubRoom(pub.topic)));
  await stub.fetch(
    new Request('http://do/publish', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ topic: pub.topic, data: pub.data }),
    }),
  );
}
