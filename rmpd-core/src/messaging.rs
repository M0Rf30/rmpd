//! Publish-subscribe messaging system
//!
//! A generic message broker that allows clients to subscribe to named channels
//! and send/receive messages. Originally designed for MPD protocol but can be
//! used for any pub-sub messaging needs.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Maximum messages queued per subscriber per channel.
const MAX_MESSAGES_PER_CHANNEL: usize = 100;

/// Maximum distinct channels tracked at once, bounding total broker memory
/// (channels x subscribers x queued messages).
const MAX_CHANNELS: usize = 256;

/// Maximum message payload length in bytes.
const MAX_MESSAGE_BYTES: usize = 4096;

/// Opaque per-connection subscriber identity, supplied by the caller (one
/// per client connection) so each subscriber gets its own message queue.
pub type SubscriberId = u64;

/// A message in a channel
#[derive(Debug, Clone)]
pub struct Message {
    pub channel: String,
    pub text: String,
}

/// Message broker managing channels and message delivery
#[derive(Debug, Clone)]
pub struct MessageBroker {
    inner: Arc<RwLock<MessageBrokerInner>>,
}

/// Per-channel state: every subscriber has its own queue, so each one is
/// guaranteed to see every message sent while it is subscribed (mirrors
/// MPD's `Client::PushMessage`/`ConsumeMessages`).
#[derive(Debug, Default)]
struct ChannelState {
    queues: HashMap<SubscriberId, VecDeque<Message>>,
}

#[derive(Debug, Default)]
struct MessageBrokerInner {
    channels: HashMap<String, ChannelState>,
}

impl MessageBroker {
    /// Create a new message broker
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MessageBrokerInner::default())),
        }
    }

    /// Send a message to a channel, delivering a copy into every current
    /// subscriber's own queue. Returns false if nobody is subscribed or the
    /// message exceeds `MAX_MESSAGE_BYTES`.
    pub async fn send_message(&self, channel: String, text: String) -> bool {
        if text.len() > MAX_MESSAGE_BYTES {
            return false;
        }
        let mut inner = self.inner.write().await;
        let Some(state) = inner.channels.get_mut(&channel) else {
            return false;
        };
        if state.queues.is_empty() {
            return false;
        }
        for queue in state.queues.values_mut() {
            queue.push_back(Message {
                channel: channel.clone(),
                text: text.clone(),
            });
            if queue.len() > MAX_MESSAGES_PER_CHANNEL {
                queue.pop_front();
            }
        }
        true
    }

    /// Register `subscriber`'s subscription to `channel`. Returns false (and
    /// does not subscribe) when the broker already tracks `MAX_CHANNELS`
    /// distinct channels and `channel` is a new one, bounding total memory.
    pub async fn register_subscriber(&self, channel: &str, subscriber: SubscriberId) -> bool {
        let mut inner = self.inner.write().await;
        if !inner.channels.contains_key(channel) && inner.channels.len() >= MAX_CHANNELS {
            return false;
        }
        inner
            .channels
            .entry(channel.to_string())
            .or_default()
            .queues
            .entry(subscriber)
            .or_default();
        true
    }

    /// Unregister `subscriber`'s subscription to `channel`, dropping its
    /// queue. Once a channel has no subscribers left it is removed entirely,
    /// so messages never persist for a channel nobody is listening to.
    pub async fn unregister_subscriber(&self, channel: &str, subscriber: SubscriberId) {
        let mut inner = self.inner.write().await;
        if let Some(state) = inner.channels.get_mut(channel) {
            state.queues.remove(&subscriber);
            if state.queues.is_empty() {
                inner.channels.remove(channel);
            }
        }
    }

    /// Get and consume `subscriber`'s own queued messages from
    /// `subscribed_channels`. Other subscribers' queues are untouched.
    pub async fn read_messages(
        &self,
        subscriber: SubscriberId,
        subscribed_channels: &[String],
    ) -> Vec<Message> {
        let mut inner = self.inner.write().await;
        let mut messages = Vec::new();

        for channel in subscribed_channels {
            if let Some(state) = inner.channels.get_mut(channel)
                && let Some(queue) = state.queues.get_mut(&subscriber)
            {
                messages.extend(queue.drain(..));
            }
        }

        messages
    }

    /// Get list of all active channels (channels with at least one subscriber)
    pub async fn list_channels(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        let mut result: Vec<String> = inner.channels.keys().cloned().collect();
        result.sort();
        result
    }
}

impl Default for MessageBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_send_and_read_message() {
        let broker = MessageBroker::new();

        broker.register_subscriber("test", 1).await;
        broker
            .send_message("test".to_string(), "hello".to_string())
            .await;

        let messages = broker.read_messages(1, &["test".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].channel, "test");
        assert_eq!(messages[0].text, "hello");
    }

    #[tokio::test]
    async fn test_multiple_channels() {
        let broker = MessageBroker::new();

        broker.register_subscriber("channel1", 1).await;
        broker.register_subscriber("channel2", 1).await;
        broker
            .send_message("channel1".to_string(), "msg1".to_string())
            .await;
        broker
            .send_message("channel2".to_string(), "msg2".to_string())
            .await;

        let messages = broker.read_messages(1, &["channel1".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "msg1");

        let messages = broker.read_messages(1, &["channel2".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "msg2");
    }

    #[tokio::test]
    async fn test_messages_are_consumed() {
        let broker = MessageBroker::new();

        broker.register_subscriber("test", 1).await;
        broker
            .send_message("test".to_string(), "hello".to_string())
            .await;

        // First read gets the message
        let messages = broker.read_messages(1, &["test".to_string()]).await;
        assert_eq!(messages.len(), 1);

        // Second read gets nothing (messages consumed)
        let messages = broker.read_messages(1, &["test".to_string()]).await;
        assert_eq!(messages.len(), 0);
    }

    /// CORE-02: two independent subscribers on the same channel must each
    /// see every message; one reading must not drain the other's queue.
    #[tokio::test]
    async fn test_two_subscribers_both_see_every_message() {
        let broker = MessageBroker::new();

        broker.register_subscriber("test", 1).await;
        broker.register_subscriber("test", 2).await;
        broker
            .send_message("test".to_string(), "hello".to_string())
            .await;

        // Subscriber 1 reads and consumes only its own queue.
        let messages = broker.read_messages(1, &["test".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "hello");

        // Subscriber 2 still sees the message, untouched by subscriber 1's read.
        let messages = broker.read_messages(2, &["test".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "hello");

        // Both queues are now empty.
        assert!(
            broker
                .read_messages(1, &["test".to_string()])
                .await
                .is_empty()
        );
        assert!(
            broker
                .read_messages(2, &["test".to_string()])
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_list_channels() {
        let broker = MessageBroker::new();

        broker.register_subscriber("channel1", 1).await;
        broker.register_subscriber("channel2", 1).await;
        broker
            .send_message("channel1".to_string(), "msg1".to_string())
            .await;
        broker
            .send_message("channel2".to_string(), "msg2".to_string())
            .await;

        let channels = broker.list_channels().await;
        assert_eq!(channels.len(), 2);
        assert!(channels.contains(&"channel1".to_string()));
        assert!(channels.contains(&"channel2".to_string()));
    }

    /// SEC-15: once the last subscriber leaves, the channel (and any
    /// unread messages) must be dropped rather than persisting forever.
    #[tokio::test]
    async fn test_channel_dropped_after_last_unsubscribe() {
        let broker = MessageBroker::new();

        broker.register_subscriber("test", 1).await;
        broker
            .send_message("test".to_string(), "hello".to_string())
            .await;
        broker.unregister_subscriber("test", 1).await;

        assert!(broker.list_channels().await.is_empty());
        // Sending now finds no subscribers.
        assert!(
            !broker
                .send_message("test".to_string(), "hello again".to_string())
                .await
        );
    }

    #[tokio::test]
    async fn test_max_messages_limit() {
        let broker = MessageBroker::new();

        broker.register_subscriber("test", 1).await;
        // Send more than MAX_MESSAGES_PER_CHANNEL
        for i in 0..150 {
            broker
                .send_message("test".to_string(), format!("msg{}", i))
                .await;
        }

        let messages = broker.read_messages(1, &["test".to_string()]).await;
        // Should only keep the last MAX_MESSAGES_PER_CHANNEL messages
        assert_eq!(messages.len(), MAX_MESSAGES_PER_CHANNEL);
        // First message should be msg50 (last 100 messages)
        assert_eq!(messages[0].text, "msg50");
    }

    #[tokio::test]
    async fn test_oversized_message_rejected() {
        let broker = MessageBroker::new();
        broker.register_subscriber("test", 1).await;
        let huge = "x".repeat(MAX_MESSAGE_BYTES + 1);
        assert!(!broker.send_message("test".to_string(), huge).await);
        assert!(
            broker
                .read_messages(1, &["test".to_string()])
                .await
                .is_empty()
        );
    }
}
