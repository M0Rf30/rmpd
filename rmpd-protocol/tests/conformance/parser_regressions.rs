//! Regression tests for parser edge cases at the TCP protocol boundary.

use crate::tcp_harness::*;

#[tokio::test]
async fn parser_regression_binarylimit_accepts_quoted_argument() {
    let (_server, mut client) = setup().await;
    let resp = client.command("binarylimit \"8192\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn parser_regression_protocol_accepts_quoted_subcommand_and_feature() {
    let (_server, mut client) = setup().await;

    let resp = client.command("protocol \"available\"").await;
    assert_ok(&resp);

    let resp = client.command("protocol \"enable\" \"binary\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn parser_regression_malformed_quoted_command_returns_ack_and_keeps_stream_synced() {
    let (_server, mut client) = setup().await;

    client.send_raw("add \"unterminated\nping\n").await;

    let malformed = client.read_response().await;
    assert!(
        malformed.starts_with("ACK [") && malformed.contains("{add}"),
        "unterminated quoted string should return ACK for add: {malformed}"
    );

    let ping = client.read_response().await;
    assert_eq!(ping, "OK\n", "follow-up ping should stay in sync: {ping}");
}

#[tokio::test]
async fn parser_regression_unknown_protocol_subcommand_returns_arg_error() {
    let (_server, mut client) = setup().await;

    let resp = client.command("protocol definitely_not_real").await;
    assert!(
        resp.starts_with("ACK [2@0] {protocol} Unknown sub command\n"),
        "unknown protocol subcommand should be an argument error: {resp}"
    );
}

#[tokio::test]
async fn parser_regression_command_list_ok_parse_error_preserves_prior_list_ok_and_index() {
    let (_server, mut client) = setup().await;

    client
        .send_raw("command_list_ok_begin\nping\nadd \"unterminated\ncommand_list_end\n")
        .await;

    let resp = client.read_response().await;
    assert!(
        resp.starts_with("list_OK\nACK [2@1] {add}"),
        "batch parse error should keep prior list_OK and report index 1: {resp}"
    );
}