//! Tests for MPD output control commands over TCP.

use crate::tcp_harness::*;

#[tokio::test]
async fn outputs_lists_default() {
    let (_server, mut client) = setup().await;
    let resp = client.command("outputs").await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "outputid").is_some(),
        "should have an output"
    );
    assert!(get_field(&resp, "outputname").is_some());
    assert!(get_field(&resp, "outputenabled").is_some());
}

#[tokio::test]
async fn disableoutput_and_enableoutput() {
    let (_server, mut client) = setup().await;

    let resp = client.command("disableoutput 0").await;
    assert_ok(&resp);

    // Check it's disabled
    let resp = client.command("outputs").await;
    assert_eq!(get_field(&resp, "outputenabled"), Some("0"));

    let resp = client.command("enableoutput 0").await;
    assert_ok(&resp);

    // Check it's enabled again
    let resp = client.command("outputs").await;
    assert_eq!(get_field(&resp, "outputenabled"), Some("1"));
}

#[tokio::test]
async fn toggleoutput() {
    let (_server, mut client) = setup().await;

    // Initially enabled (1), toggle should disable (0)
    let resp = client.command("toggleoutput 0").await;
    assert_ok(&resp);

    let resp = client.command("outputs").await;
    assert_eq!(get_field(&resp, "outputenabled"), Some("0"));

    // Toggle again to re-enable
    let resp = client.command("toggleoutput 0").await;
    assert_ok(&resp);

    let resp = client.command("outputs").await;
    assert_eq!(get_field(&resp, "outputenabled"), Some("1"));
}

#[tokio::test]
async fn outputset_attribute() {
    let (_server, mut client) = setup().await;
    let resp = client
        .command("outputset 0 allowed_formats \"44100:16:2\"")
        .await;
    assert_ok(&resp);
}

#[tokio::test]
async fn output_nonexistent_id() {
    let (_server, mut client) = setup().await;
    let resp = client.command("enableoutput 999").await;
    assert!(resp.starts_with("ACK "), "nonexistent output: {resp}");
}

#[tokio::test]
async fn outputset_illegal_attribute_name_rejected() {
    let (_server, mut client) = setup().await;
    let resp = client.command("outputset 0 \"bad name!\" value").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");
}

#[tokio::test]
async fn outputs_no_blank_line_between_entries() {
    use rmpd_protocol::state::{AppState, OutputInfo};

    let state = AppState::new();
    {
        let mut outputs = state.outputs.write().await;
        outputs.push(OutputInfo {
            id: 1,
            name: "Second Output".to_string(),
            plugin: "cpal".to_string(),
            enabled: true,
            partition: Some("default".to_string()),
            config: None,
            attributes: Default::default(),
        });
    }

    let server = MpdTestServer::start_with_state(state).await;
    let mut client = MpdTestClient::connect(server.port()).await;
    let resp = client.command("outputs").await;
    assert_ok(&resp);
    assert!(
        !resp.contains("\n\n"),
        "outputs response must not have blank lines between entries: {resp:?}"
    );
}
