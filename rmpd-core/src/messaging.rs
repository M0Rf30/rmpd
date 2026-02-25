//! Publish-subscribe messaging system
//!
//! A generic message broker that allows clients to subscribe to named channels
//! and send/receive messages. Originally designed for MPD protocol but can be
//! used for any pub-sub messaging needs.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Maximum messages to store per channel
const MAX_MESSAGES_PER_CHANNEL: usize = 100;

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

#[derive(Debug)]
struct MessageBrokerInner {
    /// Messages queued for each channel
    channels: HashMap<String, VecDeque<Message>>,
    /// Number of active subscribers per channel
    subscriber_counts: HashMap<String, usize>,
}

impl MessageBroker {
    /// Create a new message broker
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MessageBrokerInner {
                channels: HashMap::new(),
                subscriber_counts: HashMap::new(),
            })),
        }
    }

    /// Send a message to a channel. Returns false if nobody is subscribed.
    /// Send a message to a channel. Returns false if nobody is subscribed.
    pub async fn send_message(&self, channel: String, text: String) -> bool {
        let mut inner = self.inner.write().await;
        // Check if anyone is subscribed
        let count = inner.subscriber_counts.get(&channel).copied().unwrap_or(0);
        if count == 0 {
            return false;
        }
        let message = Message {
            channel: channel.clone(),
            text,
        };

        let queue = inner.channels.entry(channel).or_insert_with(VecDeque::new);
        queue.push_back(message);

        // Limit queue size
        if queue.len() > MAX_MESSAGES_PER_CHANNEL {
            queue.pop_front();
        }
        true
    }

    /// Register a subscription to a channel.
    pub async fn register_subscriber(&self, channel: &str) {
        let mut inner = self.inner.write().await;
        *inner.subscriber_counts.entry(channel.to_string()).or_insert(0) += 1;
    }

    /// Unregister a subscription from a channel.
    pub async fn unregister_subscriber(&self, channel: &str) {
        let mut inner = self.inner.write().await;
        if let Some(count) = inner.subscriber_counts.get_mut(channel) {
            if *count > 0 {
                *count -= 1;
            }
            if *count == 0 {
                inner.subscriber_counts.remove(channel);
            }
        }
    }

    /// Get all messages from channels the client is subscribed to
    pub async fn read_messages(&self, subscribed_channels: &[String]) -> Vec<Message> {
        let mut inner = self.inner.write().await;
        let mut messages = Vec::new();

        for channel in subscribed_channels {
            if let Some(queue) = inner.channels.get_mut(channel) {
                // Drain all messages from this channel
                messages.extend(queue.drain(..));
            }
        }

        messages
    }

    /// Get list of all active channels (channels with messages or subscribers)
    pub async fn list_channels(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        // Include channels with messages OR active subscribers
        let mut channels: std::collections::HashSet<String> = inner
            .channels
            .iter()
            .filter(|(_, queue)| !queue.is_empty())
            .map(|(name, _)| name.clone())
            .collect();
        for name in inner.subscriber_counts.keys() {
            channels.insert(name.clone());
        }
        let mut result: Vec<String> = channels.into_iter().collect();
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

        broker.register_subscriber("test").await;
        broker
            .send_message("test".to_string(), "hello".to_string())
            .await;

        let messages = broker.read_messages(&["test".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].channel, "test");
        assert_eq!(messages[0].text, "hello");
    }

    #[tokio::test]
    async fn test_multiple_channels() {
        let broker = MessageBroker::new();

        broker.register_subscriber("channel1").await;
        broker.register_subscriber("channel2").await;
        broker
            .send_message("channel1".to_string(), "msg1".to_string())
            .await;
        broker
            .send_message("channel2".to_string(), "msg2".to_string())
            .await;

        let messages = broker.read_messages(&["channel1".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "msg1");

        let messages = broker.read_messages(&["channel2".to_string()]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "msg2");
    }

    #[tokio::test]
    async fn test_messages_are_consumed() {
        let broker = MessageBroker::new();

        broker.register_subscriber("test").await;
        broker
            .send_message("test".to_string(), "hello".to_string())
            .await;

        // First read gets the message
        let messages = broker.read_messages(&["test".to_string()]).await;
        assert_eq!(messages.len(), 1);

        // Second read gets nothing (messages consumed)
        let messages = broker.read_messages(&["test".to_string()]).await;
        assert_eq!(messages.len(), 0);
    }

    #[tokio::test]
    async fn test_list_channels() {
        let broker = MessageBroker::new();

        broker.register_subscriber("channel1").await;
        broker.register_subscriber("channel2").await;
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

    #[tokio::test]
    async fn test_max_messages_limit() {
        let broker = MessageBroker::new();

        broker.register_subscriber("test").await;
        // Send more than MAX_MESSAGES_PER_CHANNEL
        for i in 0..150 {
            broker
                .send_message("test".to_string(), format!("msg{}", i))
                .await;
        }

        let messages = broker.read_messages(&["test".to_string()]).await;
        // Should only keep the last MAX_MESSAGES_PER_CHANNEL messages
        assert_eq!(messages.len(), MAX_MESSAGES_PER_CHANNEL);
        // First message should be msg50 (last 100 messages)
        assert_eq!(messages[0].text, "msg50");
}
}
