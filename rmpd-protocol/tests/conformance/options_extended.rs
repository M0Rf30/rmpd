//! Extended options conformance tests.
//! Tests mixrampdb and mixrampdelay set and status reflection.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn mixrampdb_set() {
    let (_server, mut client) = setup().await;
    let resp = client.command("mixrampdb -10").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn mixrampdb_reflected_in_status() {
    let (_server, mut client) = setup().await;
    client.command("mixrampdb -10").await;

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "mixrampdb"),
        Some("-10"),
        "mixrampdb should be reflected in status"
    );
}

#[tokio::test]
async fn mixrampdelay_set() {
    let (_server, mut client) = setup().await;
    let resp = client.command("mixrampdelay 2").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn mixrampdelay_reflected_in_status() {
    let (_server, mut client) = setup().await;
    client.command("mixrampdelay 2").await;

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "mixrampdelay"),
        Some("2"),
        "mixrampdelay should be reflected in status"
    );
}
