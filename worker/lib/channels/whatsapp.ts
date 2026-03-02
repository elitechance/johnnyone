import type { NormalizedMessage, NormalizedAttachment } from './normalize';

/**
 * WhatsApp Cloud API webhook payload types (subset).
 * Based on the Meta WhatsApp Business Platform Cloud API.
 */
interface WhatsAppWebhookPayload {
  object: string;
  entry: WhatsAppEntry[];
}

interface WhatsAppEntry {
  id: string;
  changes: WhatsAppChange[];
}

interface WhatsAppChange {
  value: {
    messaging_product: string;
    metadata: {
      display_phone_number: string;
      phone_number_id: string;
    };
    contacts?: Array<{
      profile: {
        name: string;
      };
      wa_id: string;
    }>;
    messages?: WhatsAppMessage[];
    statuses?: Array<{
      id: string;
      status: string;
      timestamp: string;
    }>;
  };
  field: string;
}

interface WhatsAppMessage {
  from: string;
  id: string;
  timestamp: string;
  type: 'text' | 'image' | 'audio' | 'video' | 'document' | 'sticker' | 'reaction' | 'location';
  text?: {
    body: string;
  };
  image?: WhatsAppMedia;
  audio?: WhatsAppMedia;
  video?: WhatsAppMedia;
  document?: WhatsAppMedia & {
    filename?: string;
  };
  sticker?: WhatsAppMedia;
  reaction?: {
    message_id: string;
    emoji: string;
  };
  location?: {
    latitude: number;
    longitude: number;
    name?: string;
    address?: string;
  };
  context?: {
    from: string;
    id: string;
  };
}

interface WhatsAppMedia {
  id: string;
  mime_type?: string;
  sha256?: string;
  caption?: string;
}

/**
 * Normalize a WhatsApp Cloud API webhook payload into a NormalizedMessage.
 */
export function normalizeWhatsAppMessage(payload: string): NormalizedMessage {
  const data: WhatsAppWebhookPayload = JSON.parse(payload);

  if (!data.entry || data.entry.length === 0) {
    throw new Error('Empty WhatsApp webhook payload');
  }

  const entry = data.entry[0];
  const change = entry.changes?.[0];

  if (!change || !change.value.messages || change.value.messages.length === 0) {
    throw new Error('No messages in WhatsApp webhook payload');
  }

  const message = change.value.messages[0];
  const contact = change.value.contacts?.[0];
  const phoneNumberId = change.value.metadata.phone_number_id;

  let text = '';
  const attachments: NormalizedAttachment[] = [];

  switch (message.type) {
    case 'text':
      text = message.text?.body ?? '';
      break;

    case 'image':
      text = message.image?.caption ?? '[Image]';
      if (message.image) {
        attachments.push({
          type: 'image',
          url: `whatsapp:media:${message.image.id}`,
          mimeType: message.image.mime_type,
        });
      }
      break;

    case 'audio':
      text = '[Audio message]';
      if (message.audio) {
        attachments.push({
          type: 'audio',
          url: `whatsapp:media:${message.audio.id}`,
          mimeType: message.audio.mime_type,
        });
      }
      break;

    case 'video':
      text = message.video?.caption ?? '[Video]';
      if (message.video) {
        attachments.push({
          type: 'video',
          url: `whatsapp:media:${message.video.id}`,
          mimeType: message.video.mime_type,
        });
      }
      break;

    case 'document':
      text = message.document?.caption ?? `[Document: ${message.document?.filename ?? 'file'}]`;
      if (message.document) {
        attachments.push({
          type: 'file',
          url: `whatsapp:media:${message.document.id}`,
          filename: message.document.filename,
          mimeType: message.document.mime_type,
        });
      }
      break;

    case 'sticker':
      text = '[Sticker]';
      if (message.sticker) {
        attachments.push({
          type: 'sticker',
          url: `whatsapp:media:${message.sticker.id}`,
          mimeType: message.sticker.mime_type,
        });
      }
      break;

    case 'reaction':
      text = message.reaction?.emoji ?? '';
      break;

    case 'location':
      text = message.location
        ? `[Location: ${message.location.name ?? ''} ${message.location.address ?? `${message.location.latitude},${message.location.longitude}`}]`
        : '[Location]';
      break;

    default:
      text = `[Unsupported message type: ${message.type}]`;
  }

  return {
    text,
    channelIdentifier: `${phoneNumberId}:${message.from}`,
    senderId: message.from,
    senderName: contact?.profile.name,
    timestamp: new Date(parseInt(message.timestamp) * 1000).toISOString(),
    attachments: attachments.length > 0 ? attachments : undefined,
    externalMessageId: message.id,
    rawPayload: data,
  };
}
