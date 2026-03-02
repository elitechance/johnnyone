use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tokio::sync::Mutex;

/// Maximum number of messages to queue while offline.
const MAX_QUEUE_SIZE: usize = 1000;

/// Maximum age of a queued message before it's discarded (in seconds).
const MAX_MESSAGE_AGE_SECS: i64 = 3600; // 1 hour

/// A message queued for delivery when the connection is restored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// Unique ID for this queued message.
    pub id: String,
    /// The serialized message payload (JSON string).
    pub payload: String,
    /// When the message was queued.
    pub queued_at: DateTime<Utc>,
    /// Number of delivery attempts.
    pub attempts: u32,
    /// Priority (lower = higher priority). Tool results have priority 0.
    pub priority: u32,
}

/// Offline message queue for buffering messages when the WebSocket is disconnected.
///
/// Messages are queued with a timestamp and priority, and are delivered
/// in priority order when the connection is restored. Messages older than
/// `MAX_MESSAGE_AGE_SECS` are automatically discarded during delivery.
pub struct OfflineQueue {
    queue: Mutex<VecDeque<QueuedMessage>>,
}

impl OfflineQueue {
    /// Create a new empty offline queue.
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Enqueue a message for later delivery.
    ///
    /// Returns `Ok(())` if queued successfully, or `Err` if the queue is full.
    pub async fn enqueue(&self, payload: String, priority: u32) -> Result<(), String> {
        let mut queue = self.queue.lock().await;

        if queue.len() >= MAX_QUEUE_SIZE {
            // Try to make room by removing expired messages
            self.remove_expired_inner(&mut queue);

            if queue.len() >= MAX_QUEUE_SIZE {
                return Err(format!(
                    "Offline queue is full ({} messages). Message discarded.",
                    MAX_QUEUE_SIZE
                ));
            }
        }

        let message = QueuedMessage {
            id: uuid::Uuid::new_v4().to_string(),
            payload,
            queued_at: Utc::now(),
            attempts: 0,
            priority,
        };

        tracing::debug!(
            id = %message.id,
            priority = priority,
            queue_size = queue.len() + 1,
            "Message queued for offline delivery"
        );

        queue.push_back(message);
        Ok(())
    }

    /// Drain all queued messages in priority order for delivery.
    ///
    /// Expired messages are automatically filtered out. Messages are returned
    /// sorted by priority (ascending), then by queue time (ascending).
    pub async fn drain(&self) -> Vec<QueuedMessage> {
        let mut queue = self.queue.lock().await;
        let now = Utc::now();

        // Drain all messages
        let mut messages: Vec<QueuedMessage> = queue.drain(..).collect();

        // Filter out expired messages
        let before_count = messages.len();
        messages.retain(|msg| {
            let age = now.signed_duration_since(msg.queued_at).num_seconds();
            age < MAX_MESSAGE_AGE_SECS
        });

        let expired_count = before_count - messages.len();
        if expired_count > 0 {
            tracing::info!(
                expired = expired_count,
                remaining = messages.len(),
                "Discarded expired queued messages"
            );
        }

        // Sort by priority, then by queued_at
        messages.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.queued_at.cmp(&b.queued_at))
        });

        // Increment attempt counters
        for msg in &mut messages {
            msg.attempts += 1;
        }

        tracing::info!(
            count = messages.len(),
            "Draining offline queue for delivery"
        );

        messages
    }

    /// Re-enqueue messages that failed to deliver.
    ///
    /// Messages that have exceeded `max_attempts` are discarded.
    pub async fn requeue(&self, messages: Vec<QueuedMessage>, max_attempts: u32) {
        let mut queue = self.queue.lock().await;

        for msg in messages {
            if msg.attempts >= max_attempts {
                tracing::warn!(
                    id = %msg.id,
                    attempts = msg.attempts,
                    "Discarding message after max delivery attempts"
                );
                continue;
            }

            if queue.len() < MAX_QUEUE_SIZE {
                queue.push_back(msg);
            }
        }
    }

    /// Get the number of messages currently in the queue.
    pub async fn len(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Check if the queue is empty.
    pub async fn is_empty(&self) -> bool {
        self.queue.lock().await.is_empty()
    }

    /// Clear all messages from the queue.
    pub async fn clear(&self) {
        let mut queue = self.queue.lock().await;
        let count = queue.len();
        queue.clear();
        tracing::info!(cleared = count, "Offline queue cleared");
    }

    /// Remove expired messages from the queue (internal, requires held lock).
    fn remove_expired_inner(&self, queue: &mut VecDeque<QueuedMessage>) {
        let now = Utc::now();
        let before = queue.len();
        queue.retain(|msg| {
            let age = now.signed_duration_since(msg.queued_at).num_seconds();
            age < MAX_MESSAGE_AGE_SECS
        });
        let removed = before - queue.len();
        if removed > 0 {
            tracing::debug!(removed = removed, "Removed expired messages from queue");
        }
    }
}

impl Default for OfflineQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_enqueue_and_drain() {
        let queue = OfflineQueue::new();

        queue.enqueue("msg1".to_string(), 1).await.unwrap();
        queue.enqueue("msg2".to_string(), 0).await.unwrap();
        queue.enqueue("msg3".to_string(), 1).await.unwrap();

        assert_eq!(queue.len().await, 3);

        let messages = queue.drain().await;
        assert_eq!(messages.len(), 3);

        // Priority 0 should come first
        assert_eq!(messages[0].payload, "msg2");
        assert_eq!(messages[0].priority, 0);

        // Queue should be empty after drain
        assert!(queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_requeue_with_max_attempts() {
        let queue = OfflineQueue::new();

        let messages = vec![
            QueuedMessage {
                id: "1".to_string(),
                payload: "msg1".to_string(),
                queued_at: Utc::now(),
                attempts: 3,
                priority: 0,
            },
            QueuedMessage {
                id: "2".to_string(),
                payload: "msg2".to_string(),
                queued_at: Utc::now(),
                attempts: 1,
                priority: 0,
            },
        ];

        queue.requeue(messages, 3).await;

        // Only msg2 should be requeued (msg1 has 3 attempts >= max 3)
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn test_clear() {
        let queue = OfflineQueue::new();

        queue.enqueue("msg1".to_string(), 0).await.unwrap();
        queue.enqueue("msg2".to_string(), 0).await.unwrap();

        assert_eq!(queue.len().await, 2);

        queue.clear().await;
        assert!(queue.is_empty().await);
    }
}
