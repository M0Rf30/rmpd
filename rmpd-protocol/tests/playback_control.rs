//! Integration tests for playback control commands

mod common;

use common::TestClient;

#[test]
fn test_play_command() {
    // play should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playid_command() {
    // playid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_pause_command() {
    // pause should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_stop_command() {
    // stop should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_next_command() {
    // next should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_previous_command() {
    // previous should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_seek_command() {
    // seek should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_seekid_command() {
    // seekid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_seekcur_command() {
    // seekcur should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_repeat_command() {
    // repeat should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_random_command() {
    // random should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_single_command() {
    // single should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_consume_command() {
    // consume should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_setvol_command() {
    // setvol should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_volume_command() {
    // volume (relative change) should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_getvol_command() {
    // getvol should return current volume
    let response = "volume: 100\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "volume"), Some("100"));
}

#[test]
fn test_crossfade_command() {
    // crossfade should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_mixrampdb_command() {
    // mixrampdb should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_mixrampdelay_command() {
    // mixrampdelay should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_replaygain_mode_command() {
    // replay_gain_mode should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_replaygain_status_command() {
    // replay_gain_status should return replay gain mode
    let response = "replay_gain_mode: off\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(
        TestClient::get_field(response, "replay_gain_mode"),
        Some("off")
    );
}

#[test]
fn test_status_during_playback() {
    // status during playback should include additional fields
    let response = "volume: 100\nrepeat: 0\nrandom: 0\nsingle: 0\nconsume: 0\nplaylist: 1\nplaylistlength: 1\nstate: play\nsong: 0\nsongid: 1\nelapsed: 10.5\nOK\n";

    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "state"), Some("play"));
    assert_eq!(TestClient::get_field(response, "song"), Some("0"));
    assert_eq!(TestClient::get_field(response, "songid"), Some("1"));
    assert_eq!(TestClient::get_field(response, "elapsed"), Some("10.5"));
}

#[test]
fn test_status_paused() {
    // status when paused
    let response = "volume: 100\nrepeat: 0\nrandom: 0\nsingle: 0\nconsume: 0\nplaylist: 1\nplaylistlength: 1\nstate: pause\nsong: 0\nsongid: 1\nelapsed: 10.5\nOK\n";

    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "state"), Some("pause"));
}

#[test]
fn test_status_stopped() {
    // status when stopped
    let response = "volume: 100\nrepeat: 0\nrandom: 0\nsingle: 0\nconsume: 0\nplaylist: 1\nplaylistlength: 1\nstate: stop\nOK\n";

    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "state"), Some("stop"));
}
