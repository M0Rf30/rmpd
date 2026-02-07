//! Extended database command conformance tests.
//! Tests findadd, searchadd, searchcount, and rescan.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn findadd_adds_to_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    let status_before = client.command("status").await;
    let len_before: u32 = get_field(&status_before, "playlistlength")
        .unwrap()
        .parse()
        .unwrap();

    let resp = client.command("findadd Artist \"Test Artist\"").await;
    assert_ok(&resp);

    let status_after = client.command("status").await;
    let len_after: u32 = get_field(&status_after, "playlistlength")
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        len_after > len_before,
        "findadd should add songs to queue: before={len_before}, after={len_after}"
    );
}

#[tokio::test]
async fn findadd_no_match() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("findadd Artist \"Nobody\"").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "playlistlength"),
        Some("0"),
        "findadd with no matches should leave queue empty"
    );
}

#[tokio::test]
async fn searchadd_adds_to_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("searchadd any \"Track\"").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    let len: u32 = get_field(&status, "playlistlength")
        .unwrap()
        .parse()
        .unwrap();
    assert!(len > 0, "searchadd should add matching songs to queue");
}

#[tokio::test]
async fn searchadd_no_match() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("searchadd any \"zzzzz\"").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "playlistlength"),
        Some("0"),
        "searchadd with no matches should leave queue empty"
    );
}

#[tokio::test]
async fn searchcount_returns_counts() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client
        .command("searchcount Artist \"Test Artist\"")
        .await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "songs").is_some(),
        "searchcount must return songs field: {resp}"
    );
    assert!(
        get_field(&resp, "playtime").is_some(),
        "searchcount must return playtime field: {resp}"
    );
}

#[tokio::test]
async fn searchcount_no_match() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("searchcount Artist \"Nobody\"").await;
    assert_ok(&resp);
    assert_eq!(
        get_field(&resp, "songs"),
        Some("0"),
        "searchcount with no matches should return songs: 0"
    );
}

#[tokio::test]
async fn findadd_exact_match_any() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    // "Track 1" is an exact title — findadd any should match it
    let resp = client.command("findadd any \"Track 1\"").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    let len: u32 = get_field(&status, "playlistlength")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(len, 1, "findadd any exact should match exactly 1 song");
}

#[tokio::test]
async fn findadd_any_no_partial_match() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    // "Track" is a partial match — findadd (exact) should NOT match it
    let resp = client.command("findadd any \"Track\"").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "playlistlength"),
        Some("0"),
        "findadd any should be exact match, not partial"
    );
}

#[tokio::test]
async fn rescan_returns_updating_db() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("rescan").await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "updating_db").is_some(),
        "rescan should return updating_db field: {resp}"
    );
}
