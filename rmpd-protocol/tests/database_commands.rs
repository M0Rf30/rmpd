//! Integration tests for database commands

mod common;
#[path = "common/tcp_harness.rs"]
mod tcp_harness;

use common::TestClient;

#[test]
fn test_update_command() {
    // update should return a job ID
    let response = "updating_db: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "updating_db"), Some("1"));
}

#[test]
fn test_rescan_command() {
    // rescan should return a job ID
    let response = "updating_db: 1\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_list_command() {
    // list should return list of values for a tag
    let response = "Artist: The Beatles\nArtist: Pink Floyd\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listall_command() {
    // listall should return all files and directories
    let response = "directory: music/album\nfile: music/album/song.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listallinfo_command() {
    // listallinfo should return files with metadata
    let response =
        "directory: music/album\nfile: music/album/song.mp3\nTitle: Song\nArtist: Artist\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_lsinfo_command() {
    // lsinfo should return directory contents
    let response = "directory: subdir\nfile: song.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_search_command() {
    // search should return matching songs
    let response = "file: test.mp3\nTitle: Test Song\nArtist: Test Artist\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Title"), Some("Test Song"));
}

#[test]
fn test_find_command() {
    // find should return exact matches
    let response = "file: test.mp3\nTitle: Test Song\nArtist: Test Artist\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_searchadd_command() {
    // searchadd should add matching songs to queue
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_findadd_command() {
    // findadd should add exact matches to queue
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_count_command() {
    // count should return statistics
    let response = "songs: 42\nplaytime: 3600\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "songs"), Some("42"));
    assert_eq!(TestClient::get_field(response, "playtime"), Some("3600"));
}

#[test]
fn test_searchcount_command() {
    // searchcount is an alias for count
    let response = "songs: 10\nplaytime: 600\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listfiles_command() {
    // listfiles should return files in directory
    let response = "file: song1.mp3\nfile: song2.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_readcomments_command() {
    // readcomments should return file metadata
    let response = "comment: This is a comment\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(
        TestClient::get_field(response, "comment"),
        Some("This is a comment")
    );
}

#[test]
fn test_getfingerprint_command() {
    // getfingerprint currently returns an error (not implemented)
    let response = "ACK [50@0] {getfingerprint} chromaprint not available\n";
    assert!(TestClient::is_error(response));
}

#[test]
fn test_albumart_command() {
    // albumart should return binary data or error
    let response = "size: 12345\nbinary: 12345\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "size"), Some("12345"));
}

#[test]
fn test_readpicture_command() {
    // readpicture should return binary picture data or error
    let response = "size: 54321\ntype: image/jpeg\nbinary: 54321\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "type"), Some("image/jpeg"));
}

// ── ACK code audit: "not found" (DatabaseErrorCode::NOT_FOUND ->
// ACK_ERROR_NO_EXIST=50) vs genuine internal/IO failure (ACK_ERROR_SYS=52),
// mirroring MPD's CommandError.cxx exception mapping. These run against a
// real rmpd instance (tcp_harness) rather than canned strings, so they
// actually exercise the handlers.

#[tokio::test]
async fn lsinfo_nonexistent_directory_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("lsinfo nosuch").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn listall_nonexistent_directory_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("listall nosuch").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn listallinfo_nonexistent_directory_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("listallinfo nosuch").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn listfiles_nonexistent_directory_stays_sys_error() {
    // Unlike lsinfo/listall (a pure database-tree lookup), `listfiles` reads
    // the real filesystem: a missing directory is a genuine `opendir()`
    // failure, which MPD's own exception mapping treats as a
    // `std::system_error` -> ACK_ERROR_SYS (52), not NOT_FOUND. This is a
    // regression test pinning that this one stays 52.
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("listfiles nosuch").await;
    assert!(resp.starts_with("ACK [52@0]"), "got: {resp}");
}

#[tokio::test]
async fn update_nonexistent_path_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("update nosuch").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn update_malformed_path_is_arg_error() {
    // A path escaping the music directory (`..`) is a different MPD error
    // than a merely nonexistent one: `uri_safe_local()` rejects it before
    // any database lookup happens, giving ACK_ERROR_ARG (2), not NOT_FOUND.
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("update \"../etc\"").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");
}

#[tokio::test]
async fn rescan_nonexistent_path_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("rescan nosuch").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn rescan_malformed_path_is_arg_error() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("rescan \"../etc\"").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");
}

#[tokio::test]
async fn albumart_missing_uri_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("albumart \"nosuch.flac\" 0").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn readpicture_missing_uri_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("readpicture \"nosuch.flac\" 0").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn readcomments_missing_uri_is_no_exist() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("readcomments \"nosuch.flac\"").await;
    assert!(resp.starts_with("ACK [50@0]"), "got: {resp}");
}

#[tokio::test]
async fn listfiles_absolute_path_is_malformed_uri() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("listfiles \"/etc\"").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");
}

#[tokio::test]
async fn albumart_absolute_path_is_malformed_uri() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("albumart \"/etc/x\" 0").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");
}

#[tokio::test]
async fn readpicture_traversal_is_malformed_uri() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("readpicture \"../../x\" 0").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");
}

#[tokio::test]
async fn readcomments_traversal_is_malformed_uri() {
    let (_server, mut client, _tmp) = tcp_harness::setup_with_db(1).await;
    let resp = client.command("readcomments \"../x\"").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");
}

#[tokio::test]
async fn albumart_respects_binarylimit() {
    // SEC-09: albumart/readpicture chunk sizes must honor the connection's
    // `binarylimit`, not a hard-coded 8192.
    let (_server, mut client, tmp) = tcp_harness::setup_with_db(1).await;
    let cover_path = tmp.path().join("music").join("cover.jpg");
    std::fs::write(&cover_path, vec![b'A'; 4096]).unwrap();

    let resp = client.command("binarylimit 1024").await;
    assert!(TestClient::is_ok(&resp), "got: {resp}");

    let resp = client.command("albumart \"song1.flac\" 0").await;
    assert_eq!(TestClient::get_field(&resp, "size"), Some("4096"));
    assert_eq!(TestClient::get_field(&resp, "binary"), Some("1024"));
}

/// Send one command over a raw socket and return the full binary-safe response
/// (the harness client reads UTF-8 lines, which a JPEG payload is not).
async fn raw_command(port: u16, cmd: &str) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Greeting.
    while !buf.ends_with(b"\n") {
        let n = s.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
    }
    buf.clear();
    s.write_all(format!("{cmd}\n").as_bytes()).await.unwrap();
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut chunk))
            .await
            .expect("read timed out")
            .unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.ends_with(b"\nOK\n") || buf == b"OK\n" || buf.starts_with(b"ACK ") {
            break;
        }
    }
    buf
}

#[tokio::test]
async fn readpicture_returns_embedded_art_for_scanned_song() {
    // Regression: the artwork cache was keyed by absolute path, which violates the
    // artwork table's foreign key on songs(path) (relative), so every lookup failed
    // and readpicture silently answered an empty OK.
    let (server, _client, tmp) = tcp_harness::setup_with_db(0).await;
    let music = tmp.path().join("music");
    let cover = tmp.path().join("cover.png");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x64:d=1",
        ])
        .args(["-frames:v", "1"])
        .arg(&cover)
        .status()
        .is_ok_and(|s| s.success())
        && std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=1", "-i"])
            .arg(&cover)
            .args(["-map", "0:a", "-map", "1", "-c:a", "flac", "-c:v", "png"])
            .args(["-disposition:v", "attached_pic"])
            .arg(music.join("art.flac"))
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("Skipping test: ffmpeg not available");
        return;
    }
    {
        let db =
            rmpd_library::Database::open(tmp.path().join("test.db").to_str().unwrap()).unwrap();
        db.add_song(&rmpd_core::test_utils::make_test_song("art.flac", 1))
            .unwrap();
    }

    for pass in ["extract", "cache hit"] {
        let resp = raw_command(server.port(), "readpicture \"art.flac\" 0").await;
        let head = String::from_utf8_lossy(&resp[..resp.len().min(64)]).into_owned();
        assert!(
            head.starts_with("size: "),
            "{pass}: expected picture, got: {head:?}"
        );
        assert!(head.contains("type: image/png"), "{pass}: got: {head:?}");
    }
}
