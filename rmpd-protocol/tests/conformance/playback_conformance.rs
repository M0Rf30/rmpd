// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for MPD playback commands over TCP.
//! Note: actual audio playback is not expected in test environments.
//! These tests verify protocol-level responses (OK/ACK as appropriate).

use crate::tcp_harness::*;
use rmpd_core::config::OutputConfig;
use rmpd_core::test_utils::make_test_song;
use rmpd_protocol::state::AppState;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[tokio::test]
async fn play_empty_queue_errors() {
    let (_server, mut client) = setup().await;
    let resp = client.command("play 0").await;
    assert!(resp.starts_with("ACK "), "play on empty queue: {resp}");
}

#[tokio::test]
async fn stop_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("stop").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn pause_without_playing() {
    let (_server, mut client) = setup().await;
    // pause when stopped should still succeed (MPD returns OK)
    let resp = client.command("pause").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn next_empty_queue() {
    let (_server, mut client) = setup().await;
    let resp = client.command("next").await;
    // rmpd returns ACK when queue is empty - valid behavior
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn previous_empty_queue() {
    let (_server, mut client) = setup().await;
    let resp = client.command("previous").await;
    // rmpd returns ACK when queue is empty - valid behavior
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn currentsong_when_stopped() {
    let (_server, mut client) = setup().await;
    let resp = client.command("currentsong").await;
    assert_ok(&resp);
    // When stopped, currentsong should return OK with no song data
    assert_eq!(resp, "OK\n");
}

#[tokio::test]
async fn seekid_nonexistent_errors() {
    let (_server, mut client) = setup().await;
    let resp = client.command("seekid 9999 0").await;
    assert!(resp.starts_with("ACK "), "seekid non-existent: {resp}");
}

#[tokio::test]
async fn seekcur_when_stopped() {
    let (_server, mut client) = setup().await;
    // seekcur without playing should error or be no-op
    let resp = client.command("seekcur 0").await;
    // Implementation may return OK or ACK, both are valid
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn play_with_songs_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    // play should succeed (even if audio device isn't available)
    let resp = client.command("play 0").await;
    // May succeed or fail depending on audio backend, but should not crash
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn playid_with_songs_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let r = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&r, "Id").unwrap();

    let resp = client.command(&format!("playid {id}")).await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn seek_with_songs_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("seek 0 10").await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn play_out_of_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("play 999").await;
    assert!(resp.starts_with("ACK "), "play out of range: {resp}");
}

// ── Real playback through the paced `null` output ───────────────────────
//
// The tests below need a song to genuinely start, play, fail or finish, so
// they play real (silent) WAV files through rmpd's `null` output — paced in
// real time like MPD's null plugin — which needs no audio hardware.

/// Write `seconds` of 8 kHz mono 16-bit silence as a WAV file.
pub(crate) fn write_silent_wav(path: &std::path::Path, seconds: u32) {
    let sample_rate: u32 = 8000;
    let data_len = sample_rate * seconds * 2;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVEfmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    buf.resize(44 + data_len as usize, 0);
    std::fs::write(path, buf).unwrap();
}

/// What a test song's file in the music directory holds.
#[derive(Clone, Copy)]
pub(crate) enum Media {
    /// This many seconds of silence (a WAV file).
    Silence(u32),
    /// Bytes that are not audio at all: the decoder rejects the file.
    NotAudio,
    /// A named pipe nobody writes to: opening it for decoding blocks until a
    /// writer shows up, i.e. a song that is stuck "opening" (unix only).
    #[cfg(unix)]
    Fifo,
}

/// A server whose database holds one song per `(file name, seconds)` entry,
/// backed by a real file in the music directory: `seconds` of silence, or —
/// for `0` — a file that is not audio at all (the decoder rejects it). Every
/// song is added to the queue, in order. Audio goes to an output of
/// `output_type` (`"null"` plays, anything unknown fails to open).
pub(crate) async fn setup_playable_with_output(
    files: &[(&str, u32)],
    output_type: &str,
) -> (MpdTestServer, MpdTestClient, TempDir) {
    let media: Vec<(&str, Media)> = files
        .iter()
        .map(|&(name, seconds)| {
            let media = if seconds == 0 {
                Media::NotAudio
            } else {
                Media::Silence(seconds)
            };
            (name, media)
        })
        .collect();
    setup_media(&media, output_type).await
}

/// [`setup_playable_with_output`] with a choice of what each file holds.
pub(crate) async fn setup_media(
    files: &[(&str, Media)],
    output_type: &str,
) -> (MpdTestServer, MpdTestClient, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();
    let music_dir = tmp.path().join("music");
    std::fs::create_dir_all(&music_dir).unwrap();
    let playlist_dir = tmp.path().join("playlists");
    std::fs::create_dir_all(&playlist_dir).unwrap();

    {
        let db = rmpd_library::Database::open(&db_path_str).unwrap();
        for (i, (name, media)) in files.iter().enumerate() {
            let file = music_dir.join(name);
            match media {
                Media::NotAudio => std::fs::write(&file, b"this is not audio").unwrap(),
                Media::Silence(seconds) => write_silent_wav(&file, *seconds),
                #[cfg(unix)]
                Media::Fifo => assert!(
                    std::process::Command::new("mkfifo")
                        .arg(&file)
                        .status()
                        .expect("mkfifo")
                        .success()
                ),
            }
            db.add_song(&make_test_song(name, i as u32 + 1)).unwrap();
        }
    }

    let mut state = AppState::with_all_paths(
        db_path_str,
        music_dir.to_str().unwrap().to_string(),
        playlist_dir.to_str().unwrap().to_string(),
    );
    state.disable_actual_mount = true;
    state.engine.write().await.set_outputs(vec![OutputConfig {
        output_type: output_type.to_owned(),
        ..OutputConfig::cpal_default()
    }]);
    let (server, mut client) = setup_with_state(state).await;
    for (name, _) in files {
        assert_ok(&client.command(&format!("add \"{name}\"")).await);
    }
    (server, client, tmp)
}

/// [`setup_playable_with_output`] playing through the `null` output.
pub(crate) async fn setup_playable(
    files: &[(&str, u32)],
) -> (MpdTestServer, MpdTestClient, TempDir) {
    setup_playable_with_output(files, "null").await
}

/// Poll `status` until `done` accepts it (15 s cap) and return that status.
pub(crate) async fn wait_status(
    client: &mut MpdTestClient,
    what: &str,
    done: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = client.command("status").await;
        if done(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; last status:\n{status}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `stats` `playtime` as a number.
async fn playtime(client: &mut MpdTestClient) -> u64 {
    let stats = client.command("stats").await;
    get_field(&stats, "playtime")
        .unwrap_or_else(|| panic!("no playtime in: {stats}"))
        .parse()
        .unwrap()
}

/// Poll `stats` `playtime` until `done` accepts it (15 s cap).
async fn wait_playtime(client: &mut MpdTestClient, what: &str, done: impl Fn(u64) -> bool) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let t = playtime(client).await;
        if done(t) {
            return t;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; playtime is {t}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn stop_keeps_the_current_song_in_status_and_currentsong() {
    // MPD `playlist::Stop` leaves `current` alone: the stopped player still
    // names the song it stopped on, and the one after it.
    let (_server, mut client, _tmp) =
        setup_playable(&[("a.wav", 60), ("b.wav", 60), ("c.wav", 60)]).await;
    assert_ok(&client.command("play 1").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;

    assert_ok(&client.command("stop").await);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "state"), Some("stop"), "{status}");
    assert_eq!(get_field(&status, "song"), Some("1"), "{status}");
    assert_eq!(get_field(&status, "songid"), Some("2"), "{status}");
    assert_eq!(get_field(&status, "nextsong"), Some("2"), "{status}");
    assert_eq!(get_field(&status, "nextsongid"), Some("3"), "{status}");
    // …but a stopped player reports no playback position.
    assert!(get_field(&status, "time").is_none(), "{status}");
    assert!(get_field(&status, "elapsed").is_none(), "{status}");

    let current = client.command("currentsong").await;
    assert_eq!(get_field(&current, "file"), Some("b.wav"), "{current}");
    assert_eq!(get_field(&current, "Pos"), Some("1"), "{current}");
    assert_eq!(get_field(&current, "Id"), Some("2"), "{current}");
}

#[tokio::test]
async fn play_without_argument_restarts_the_song_that_was_stopped() {
    let (_server, mut client, _tmp) =
        setup_playable(&[("a.wav", 60), ("b.wav", 60), ("c.wav", 60)]).await;
    assert_ok(&client.command("play 2").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("stop").await);

    // MPD `PlayAny`: resume at `current`, not at the top of the queue.
    assert_ok(&client.command("play").await);
    let status = wait_status(&mut client, "playback to restart", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_eq!(get_field(&status, "song"), Some("2"), "{status}");
    assert_eq!(get_field(&status, "songid"), Some("3"), "{status}");
}

#[tokio::test]
async fn stopped_player_refuses_next_previous_and_seekcur() {
    // `playlist::PlayNext`/`PlayPrevious`/`SeekCurrent` all check the
    // `playing` flag — a remembered current song does not make a stopped
    // player "playing".
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60), ("b.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("stop").await);

    assert_eq!(
        client.command("next").await,
        "ACK [55@0] {next} Not playing\n"
    );
    assert_eq!(
        client.command("previous").await,
        "ACK [55@0] {previous} Not playing\n"
    );
    assert_eq!(
        client.command("seekcur 1").await,
        "ACK [55@0] {seekcur} Not playing\n"
    );
    assert_eq!(
        client.command("seekcur +1").await,
        "ACK [55@0] {seekcur} Not playing\n"
    );
    // The current song is untouched by the refusals.
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "song"), Some("0"), "{status}");
}

#[tokio::test]
async fn relative_queue_positions_use_the_stopped_current_song() {
    // `RequireCurrentPosition` only needs `current >= 0`, which a stop keeps.
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60), ("b.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("stop").await);

    assert_ok(
        &client
            .command("addid \"http://example.com/x.mp3\" +0")
            .await,
    );
    let info = client.command("playlistinfo 1").await;
    assert_eq!(
        get_field(&info, "file"),
        Some("http://example.com/x.mp3"),
        "inserted right after the stopped current song: {info}"
    );
}

#[tokio::test]
async fn queue_edits_keep_the_stopped_current_song_anchored() {
    let (_server, mut client, _tmp) =
        setup_playable(&[("a.wav", 60), ("b.wav", 60), ("c.wav", 60)]).await;
    assert_ok(&client.command("play 1").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("stop").await);

    // Removing a song before it moves its position, not its identity.
    assert_ok(&client.command("delete 0").await);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "song"), Some("0"), "{status}");
    assert_eq!(get_field(&status, "songid"), Some("2"), "{status}");
    assert_eq!(get_field(&status, "nextsong"), Some("1"), "{status}");

    // Deleting the current song itself while stopped forgets it
    // (`DeleteInternal`: "there's a current song but we're not playing").
    assert_ok(&client.command("deleteid 2").await);
    let status = client.command("status").await;
    assert!(get_field(&status, "song").is_none(), "{status}");
    assert!(get_field(&status, "songid").is_none(), "{status}");
    assert_eq!(client.command("currentsong").await, "OK\n");
}

#[tokio::test]
async fn rangeid_is_allowed_on_the_stopped_current_song() {
    // `SetSongIdRange` only refuses the current song while `playing`.
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    let denied = client.command("rangeid 1 0.0:10.0").await;
    assert!(
        denied.starts_with("ACK [4@0] {rangeid} Cannot edit the current song"),
        "{denied}"
    );
    assert_ok(&client.command("stop").await);
    assert_ok(&client.command("rangeid 1 0.0:10.0").await);
}

#[tokio::test]
async fn play_without_argument_while_paused_resumes_instead_of_restarting() {
    // `PlayAny`: "already playing: unpause playback, just in case it was
    // paused, and return" — the song is not started over.
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("seekcur 20").await);
    assert_ok(&client.command("pause 1").await);
    wait_status(&mut client, "pause", |s| {
        get_field(s, "state") == Some("pause")
    })
    .await;

    assert_ok(&client.command("play").await);
    let status = wait_status(&mut client, "resume", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    let elapsed: f64 = get_field(&status, "elapsed").unwrap().parse().unwrap();
    assert!(elapsed >= 20.0, "play must resume, not restart: {status}");
}

#[tokio::test]
async fn deleting_the_playing_song_carries_on_with_the_next_one() {
    // `DeleteInternal`: the playing song goes away -> "play the song after the
    // deleted one".
    let (_server, mut client, _tmp) =
        setup_playable(&[("a.wav", 60), ("b.wav", 60), ("c.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;

    assert_ok(&client.command("delete 0").await);
    let status = wait_status(&mut client, "the next song to play", |s| {
        get_field(s, "state") == Some("play") && get_field(s, "songid") == Some("2")
    })
    .await;
    assert_eq!(get_field(&status, "song"), Some("0"), "{status}");

    // Deleting the last song while it plays has nothing to fall back on: stop.
    assert_ok(&client.command("deleteid 3").await); // c.wav, not playing
    assert_ok(&client.command("deleteid 2").await); // b.wav, playing, last
    let status = wait_status(&mut client, "playback to stop", |s| {
        get_field(s, "state") == Some("stop")
    })
    .await;
    assert!(get_field(&status, "song").is_none(), "{status}");
}

#[tokio::test]
async fn deleting_the_paused_song_stops_and_selects_its_successor() {
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60), ("b.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("pause 1").await);
    wait_status(&mut client, "pause", |s| {
        get_field(s, "state") == Some("pause")
    })
    .await;

    assert_ok(&client.command("delete 0").await);
    let status = wait_status(&mut client, "stop", |s| {
        get_field(s, "state") == Some("stop")
    })
    .await;
    // `current` moved on to the song that took its place (b.wav, id 2).
    assert_eq!(get_field(&status, "song"), Some("0"), "{status}");
    assert_eq!(get_field(&status, "songid"), Some("2"), "{status}");
}

#[tokio::test]
async fn running_off_the_end_of_the_queue_forgets_the_current_song() {
    // Unlike `stop`, finishing the last song resets `current`
    // (`PlayNext`: `Stop(); current = -1`).
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 1)]).await;
    assert_ok(&client.command("play 0").await);
    // (`status` derives `state` from the engine, which flips a moment before
    // the queue manager forgets the song: wait for both.)
    let status = wait_status(&mut client, "the song to finish", |s| {
        get_field(s, "state") == Some("stop") && get_field(s, "song").is_none()
    })
    .await;
    assert!(get_field(&status, "nextsong").is_none(), "{status}");
}

#[tokio::test]
async fn next_at_the_end_of_the_queue_forgets_the_current_song() {
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("next").await);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "state"), Some("stop"), "{status}");
    assert!(get_field(&status, "song").is_none(), "{status}");
}

#[tokio::test]
async fn playback_error_is_reported_in_status_until_cleared() {
    let (_server, mut client, _tmp) = setup_playable(&[("bad.wav", 0)]).await;
    assert_ok(&client.command("play 0").await);

    // Decoding fails on the decode thread; `status` shows MPD's message
    // (`Failed to decode "<uri>": <cause>`) and playback is stopped, still
    // on that song (the one-song queue exhausts MPD's error budget).
    let status = wait_status(&mut client, "the playback error", |s| {
        get_field(s, "error").is_some()
    })
    .await;
    let error = get_field(&status, "error").unwrap();
    assert!(
        error.starts_with("Failed to decode \"bad.wav\": "),
        "unexpected error text: {error}"
    );
    let status = wait_status(&mut client, "playback to stop", |s| {
        get_field(s, "state") == Some("stop")
    })
    .await;
    assert_eq!(get_field(&status, "song"), Some("0"), "{status}");
    assert!(get_field(&status, "error").is_some(), "{status}");

    // `clearerror` forgets it; the next attempt reports it again.
    assert_ok(&client.command("clearerror").await);
    let status = client.command("status").await;
    assert!(get_field(&status, "error").is_none(), "{status}");

    assert_ok(&client.command("play").await);
    wait_status(&mut client, "the error to come back", |s| {
        get_field(s, "error").is_some()
    })
    .await;
}

#[tokio::test]
async fn a_song_that_fails_to_play_is_skipped_when_others_can_play() {
    // `ResumePlayback`: below the error budget, carry on with the next song;
    // starting it clears the error.
    let (_server, mut client, _tmp) = setup_playable(&[("bad.wav", 0), ("good.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    let status = wait_status(&mut client, "the next song to play", |s| {
        get_field(s, "state") == Some("play") && get_field(s, "song") == Some("1")
    })
    .await;
    assert!(get_field(&status, "error").is_none(), "{status}");
}

#[tokio::test]
async fn playback_stops_once_every_queued_song_failed() {
    // `error_count >= queue.GetLength()`: no endless loop over a queue of
    // unplayable files, even with repeat on.
    let (_server, mut client, _tmp) = setup_playable(&[("bad1.wav", 0), ("bad2.wav", 0)]).await;
    assert_ok(&client.command("repeat 1").await);
    assert_ok(&client.command("play 0").await);
    let status = wait_status(&mut client, "playback to give up", |s| {
        get_field(s, "state") == Some("stop")
            && get_field(s, "error").is_some()
            && get_field(s, "song") == Some("1")
    })
    .await;
    let error = get_field(&status, "error").unwrap();
    assert!(
        error.starts_with("Failed to decode \"bad2.wav\": "),
        "{error}"
    );
    // …and it stays stopped.
    assert_status_holds(
        &mut client,
        Duration::from_millis(500),
        "the stopped player",
        |s| get_field(s, "state") == Some("stop"),
    )
    .await;
}

#[tokio::test]
async fn output_that_cannot_open_stops_playback_with_an_error() {
    let (_server, mut client, _tmp) =
        setup_playable_with_output(&[("a.wav", 60), ("b.wav", 60)], "no-such-output").await;
    assert_ok(&client.command("play 0").await);
    // An output error is fatal (MPD stops instead of skipping ahead — the next
    // song would hit the same dead output), keeping the current song.
    let status = wait_status(&mut client, "the output error", |s| {
        get_field(s, "error").is_some()
    })
    .await;
    let error = get_field(&status, "error").unwrap();
    assert!(
        error.starts_with("Failed to open \"Default Output\" (no-such-output): "),
        "unexpected error text: {error}"
    );
    let status = wait_status(&mut client, "playback to stop", |s| {
        get_field(s, "state") == Some("stop")
    })
    .await;
    assert_eq!(get_field(&status, "song"), Some("0"), "{status}");
}

#[tokio::test]
async fn seek_on_an_undecodable_song_reports_the_decoder_error() {
    // MPD 0.25 "show detailed seek errors": the player's exception reaches the
    // client as ACK_ERROR_UNKNOWN (5) with its full message, instead of an
    // empty-handed OK / "Not playing".
    let (_server, mut client, _tmp) = setup_playable(&[("bad.wav", 0)]).await;
    let resp = client.command("seek 0 5").await;
    assert!(
        resp.starts_with("ACK [5@0] {seek} Failed to decode \"bad.wav\": "),
        "unexpected response: {resp}"
    );
    // Let the first attempt's failure be fully handled (error recorded,
    // player stopped) before the next attempt starts, so the two cannot
    // overlap on a slow machine.
    wait_status(&mut client, "the first attempt to settle", |s| {
        get_field(s, "state") == Some("stop") && get_field(s, "error").is_some()
    })
    .await;
    let resp = client.command("seekid 1 5").await;
    assert!(
        resp.starts_with("ACK [5@0] {seekid} Failed to decode \"bad.wav\": "),
        "unexpected response: {resp}"
    );
}

/// Open the write end of the FIFO behind `name` in the test's music
/// directory, then close it: whoever is blocked opening it for reading gets
/// an empty stream (and fails to probe it).
#[cfg(unix)]
async fn release_fifo(tmp: &TempDir, name: &str) {
    let fifo = tmp.path().join("music").join(name);
    tokio::task::spawn_blocking(move || {
        drop(std::fs::OpenOptions::new().write(true).open(fifo).unwrap());
    })
    .await
    .unwrap();
}

/// Assert `done` stays false for `window`, polling `status` (an absence
/// check: it can only wait, but it fails the moment the state is wrong).
async fn assert_status_holds(
    client: &mut MpdTestClient,
    window: Duration,
    what: &str,
    holds: impl Fn(&str) -> bool,
) {
    let until = Instant::now() + window;
    loop {
        let status = client.command("status").await;
        assert!(holds(&status), "{what} no longer holds:\n{status}");
        if Instant::now() >= until {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_song_aborted_while_opening_does_not_disturb_its_replacement() {
    // `play 0` leaves the decode thread stuck opening a FIFO. `play 2` aborts
    // it (it has to wait for that thread), and the aborted song then fails as
    // soon as its open returns. That failure belongs to a song the user has
    // already moved on from: it must not set `error:` nor stop or skip the
    // song now playing. (The last song is the replacement, so a stale failure
    // handled as "advance" would either stop at the end of the queue or step
    // back onto the song after the stuck one — both visible.)
    let (_server, mut client, tmp) = setup_media(
        &[
            ("stuck.wav", Media::Fifo),
            ("good.wav", Media::Silence(60)),
            ("last.wav", Media::Silence(60)),
        ],
        "null",
    )
    .await;
    assert_ok(&client.command("play 0").await);

    let fifo = tmp.path().join("music").join("stuck.wav");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        drop(std::fs::OpenOptions::new().write(true).open(fifo).unwrap());
    });
    assert_ok(&client.command("play 2").await);
    writer.join().unwrap();

    wait_status(&mut client, "last.wav to play", |s| {
        get_field(s, "state") == Some("play") && get_field(s, "song") == Some("2")
    })
    .await;
    assert_status_holds(
        &mut client,
        Duration::from_millis(1500),
        "playback of last.wav",
        |s| {
            get_field(s, "state") == Some("play")
                && get_field(s, "song") == Some("2")
                && get_field(s, "error").is_none()
        },
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_song_aborted_by_stop_leaves_no_error_behind() {
    let (_server, mut client, tmp) = setup_media(
        &[("stuck.wav", Media::Fifo), ("good.wav", Media::Silence(60))],
        "null",
    )
    .await;
    assert_ok(&client.command("play 0").await);

    // `stop` waits for the stuck decode thread; its failure then lands after
    // the stop and is nobody's error.
    let fifo = tmp.path().join("music").join("stuck.wav");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        drop(std::fs::OpenOptions::new().write(true).open(fifo).unwrap());
    });
    assert_ok(&client.command("stop").await);
    writer.join().unwrap();

    assert_status_holds(
        &mut client,
        Duration::from_millis(1000),
        "the stopped player with no error",
        |s| {
            get_field(s, "state") == Some("stop")
                && get_field(s, "error").is_none()
                && get_field(s, "song") == Some("0")
        },
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_seek_waiting_for_its_verdict_does_not_block_other_clients() {
    // The decode thread is stuck opening a FIFO, so `seekcur`'s verdict (up to
    // several seconds) cannot arrive. Waiting for it must not hold the engine
    // lock: any engine writer queued behind it — here `setvol` — would stall,
    // and with it every later `status`/`stats` reader.
    let (server, mut client, tmp) = setup_media(&[("stuck.wav", Media::Fifo)], "null").await;
    assert_ok(&client.command("play 0").await);

    let seeker = tokio::spawn(async move { client.command("seekcur 5").await });
    // Give the seek time to be queued and start waiting. (Too short a wait
    // can only make this test less strict, never fail it: until the seek is
    // queued there is no lock to be stuck behind.)
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!seeker.is_finished(), "the seek must still be waiting");

    let mut other = MpdTestClient::connect(server.port()).await;
    let started = Instant::now();
    assert_ok(&other.command("setvol 50").await);
    let status = other.command("status").await;
    assert_eq!(get_field(&status, "volume"), Some("50"), "{status}");
    assert_ok(&other.command("stats").await);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "engine users stalled behind a pending seek for {:?}",
        started.elapsed()
    );

    // Let the song fail: the waiting seek then reports why.
    release_fifo(&tmp, "stuck.wav").await;
    let resp = seeker.await.unwrap();
    assert!(
        resp.starts_with("ACK [5@0] {seekcur} Failed to decode \"stuck.wav\": "),
        "unexpected response: {resp}"
    );
}

#[tokio::test]
async fn seek_to_another_song_starts_it_at_the_requested_offset() {
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60), ("b.wav", 60)]).await;
    assert_ok(&client.command("seek 1 20").await);
    let status = wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_eq!(get_field(&status, "song"), Some("1"), "{status}");
    let elapsed: f64 = get_field(&status, "elapsed").unwrap().parse().unwrap();
    assert!(
        (20.0..30.0).contains(&elapsed),
        "seek 1 20 must start song 1 at 20 s, got elapsed {elapsed}"
    );

    // Same for seekid, from a stopped state (the current song survives stop).
    assert_ok(&client.command("stop").await);
    assert_ok(&client.command("seekid 1 30").await);
    let status = wait_status(&mut client, "playback to restart", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_eq!(get_field(&status, "song"), Some("0"), "{status}");
    let elapsed: f64 = get_field(&status, "elapsed").unwrap().parse().unwrap();
    assert!((30.0..40.0).contains(&elapsed), "elapsed {elapsed}");
}

#[tokio::test]
async fn seekcur_past_the_end_is_not_an_error() {
    // MPD clamps the seek to the end of the song (`SeekDecoder`).
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60), ("b.wav", 60)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    assert_ok(&client.command("seekcur 100000").await);
    // Seeking to the end finishes the song: playback moves on to the next.
    wait_status(&mut client, "playback to move on", |s| {
        get_field(s, "song") == Some("1")
    })
    .await;
}

#[tokio::test]
async fn stats_playtime_counts_played_audio_and_ignores_pauses() {
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 60)]).await;
    assert_eq!(playtime(&mut client).await, 0, "nothing played yet");

    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "playback to start", |s| {
        get_field(s, "state") == Some("play")
    })
    .await;
    let playing = wait_playtime(&mut client, "audio to be counted", |t| t > 0).await;
    assert!(playing > 0, "audio handed to the output must count");

    // Paused time does not count (the decode thread writes nothing).
    assert_ok(&client.command("pause 1").await);
    // The decode thread only notices the pause once the write it is blocked
    // in (paced in real time, up to one 0.512 s chunk) returns: settle first,
    // polling at an interval longer than a chunk.
    let mut paused = playtime(&mut client).await;
    let settle_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let now = playtime(&mut client).await;
        if now == paused {
            break;
        }
        assert!(
            Instant::now() < settle_deadline,
            "playtime never settled after the pause (still moving: {paused} -> {now})"
        );
        paused = now;
    }
    // An absence check, so it can only wait: playtime must still be flat.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        playtime(&mut client).await,
        paused,
        "playtime must not advance while paused"
    );

    // A stop keeps the total: it is a since-startup counter.
    assert_ok(&client.command("stop").await);
    assert!(playtime(&mut client).await >= paused);
}

#[tokio::test]
async fn stats_playtime_of_a_finished_song_is_its_length() {
    let (_server, mut client, _tmp) = setup_playable(&[("a.wav", 2)]).await;
    assert_ok(&client.command("play 0").await);
    wait_status(&mut client, "the song to finish", |s| {
        get_field(s, "state") == Some("stop")
    })
    .await;
    assert_eq!(playtime(&mut client).await, 2);
}

#[tokio::test]
async fn consume_with_random_and_repeat_never_consumes_the_song_that_plays() {
    // MPD 0.24.16 "fix consuming the wrong song after reshuffle in
    // random+repeat mode": the song that was playing is consumed, never the
    // one that was just chosen to follow it — and, per `GetNextOrder`, the last
    // consumed song cannot wrap onto itself under repeat.
    let (_server, mut client, _tmp) =
        setup_playable(&[("a.wav", 1), ("b.wav", 1), ("c.wav", 1)]).await;
    assert_ok(&client.command("random 1").await);
    assert_ok(&client.command("repeat 1").await);
    assert_ok(&client.command("consume 1").await);
    assert_ok(&client.command("play 0").await);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen_lengths = std::collections::BTreeSet::new();
    let mut dangling = None::<String>;
    loop {
        let status = client.command("status").await;
        let queue = client.command("playlistinfo").await;
        seen_lengths.insert(
            get_field(&status, "playlistlength")
                .unwrap()
                .parse::<u32>()
                .unwrap(),
        );
        if let Some(id) = get_field(&status, "songid") {
            let queued = queue.contains(&format!("Id: {id}\n"));
            // A momentary mismatch is just two commands straddling a song
            // change; a stable one is a current song that is not in the queue.
            if !queued && dangling.as_deref() == Some(id) {
                panic!("current song {id} is not in the queue:\n{status}\n{queue}");
            }
            dangling = (!queued).then(|| id.to_string());
        } else {
            dangling = None;
        }
        if get_field(&status, "state") == Some("stop")
            && get_field(&status, "playlistlength") == Some("0")
        {
            assert!(get_field(&status, "song").is_none(), "{status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "queue never drained; last status:\n{status}\n{queue}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Every song was consumed one at a time: 3 -> 2 -> 1 -> 0.
    assert!(
        seen_lengths.contains(&3) && seen_lengths.contains(&0),
        "{seen_lengths:?}"
    );
}
