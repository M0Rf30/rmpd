//! Verifies `subscribe`/`unsubscribe` notify idle `subscription` clients and
//! `sendmessage` notifies idle `message` clients, matching MPD's
//! `EmitIdle(IDLE_SUBSCRIPTION)` / `IdleAdd(IDLE_MESSAGE)`.

use rmpd_core::event::{Event, Subsystem};
use rmpd_protocol::commands::messaging;
use rmpd_protocol::connection::ConnectionState;
use rmpd_protocol::state::AppState;

#[test]
fn subscription_and_message_events_map_to_subsystems() {
    assert!(
        Event::SubscriptionChanged
            .subsystems()
            .contains(&Subsystem::Subscription),
        "Event::SubscriptionChanged must map to Subsystem::Subscription"
    );
    assert!(
        Event::MessageReceived
            .subsystems()
            .contains(&Subsystem::Message),
        "Event::MessageReceived must map to Subsystem::Message"
    );
}

#[tokio::test]
async fn subscribe_emits_subscription_notification() {
    let state = AppState::new();
    let mut conn = ConnectionState::new();
    let mut rx = state.event_bus.subscribe();

    let resp = messaging::handle_subscribe_command(&state, &mut conn, "testchan").await;
    assert!(resp.contains("OK"), "got: {resp}");

    assert!(
        rx.try_recv()
            .is_ok_and(|ev| matches!(ev, Event::SubscriptionChanged)),
        "subscribe must emit Event::SubscriptionChanged"
    );
}

#[tokio::test]
async fn unsubscribe_emits_subscription_notification() {
    let state = AppState::new();
    let mut conn = ConnectionState::new();
    messaging::handle_subscribe_command(&state, &mut conn, "testchan").await;

    let mut rx = state.event_bus.subscribe();
    let resp = messaging::handle_unsubscribe_command(&state, &mut conn, "testchan").await;
    assert!(resp.contains("OK"), "got: {resp}");

    assert!(
        rx.try_recv()
            .is_ok_and(|ev| matches!(ev, Event::SubscriptionChanged)),
        "unsubscribe must emit Event::SubscriptionChanged"
    );
}

#[tokio::test]
async fn sendmessage_emits_message_notification_only_when_delivered() {
    let state = AppState::new();
    let mut conn = ConnectionState::new();
    messaging::handle_subscribe_command(&state, &mut conn, "testchan").await;

    let mut rx = state.event_bus.subscribe();
    let resp = messaging::handle_sendmessage_command(&state, "testchan", "hi").await;
    assert!(resp.contains("OK"), "got: {resp}");
    assert!(
        rx.try_recv()
            .is_ok_and(|ev| matches!(ev, Event::MessageReceived)),
        "sendmessage to a subscribed channel must emit Event::MessageReceived"
    );

    let resp = messaging::handle_sendmessage_command(&state, "nobody-here", "hi").await;
    assert!(resp.starts_with("ACK"), "got: {resp}");
    assert!(
        rx.try_recv().is_err(),
        "sendmessage with no subscribers must not emit a notification"
    );
}
