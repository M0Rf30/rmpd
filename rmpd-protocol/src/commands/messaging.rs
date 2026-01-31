//! Client-to-client messaging commands
//!
//! MPD supports a publish-subscribe messaging system for clients to communicate.
//! This module handles channel subscription and message passing commands.

use super::{AppState, ResponseBuilder};
use crate::connection::ConnectionState;

/// Subscribe to a message channel
///
/// Clients can subscribe to named channels to receive messages.
pub async fn handle_subscribe_command(conn_state: &mut ConnectionState, channel: &str) -> String {
    conn_state.subscribe(channel.to_string());
    ResponseBuilder::new().ok()
}

/// Unsubscribe from a message channel
///
/// Removes the subscription to a channel.
pub async fn handle_unsubscribe_command(conn_state: &mut ConnectionState, channel: &str) -> String {
    conn_state.unsubscribe(channel);
    ResponseBuilder::new().ok()
}

/// List all available message channels
///
/// Returns channels that currently have messages or subscribers.
pub async fn handle_channels_command(state: &AppState) -> String {
    let channels = state.message_broker.list_channels().await;
    let mut resp = ResponseBuilder::new();

    for channel in channels {
        resp.field("channel", channel);
    }

    resp.ok()
}

/// Read messages from subscribed channels
///
/// Returns all messages from channels this client is subscribed to,
/// and removes them from the queue.
pub async fn handle_readmessages_command(
    state: &AppState,
    conn_state: &ConnectionState,
) -> String {
    let messages = state
        .message_broker
        .read_messages(conn_state.subscribed_channels())
        .await;

    let mut resp = ResponseBuilder::new();

    for message in messages {
        resp.field("channel", message.channel);
        resp.field("message", message.text);
    }

    resp.ok()
}

/// Send a message to a channel
///
/// Broadcasts a message to a channel. All subscribed clients will receive it
/// when they call readmessages.
pub async fn handle_sendmessage_command(state: &AppState, channel: &str, message: &str) -> String {
    state
        .message_broker
        .send_message(channel.to_string(), message.to_string())
        .await;
    ResponseBuilder::new().ok()
}
