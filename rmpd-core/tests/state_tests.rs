use rmpd_core::state::{ConsumeMode, PlayerState, PlayerStatus, ReplayGainMode, SingleMode};
use std::time::Duration;

#[test]
fn test_player_state_from_atomic_stop() {
    let state = PlayerState::from_atomic(0);
    assert_eq!(state, PlayerState::Stop);
}

#[test]
fn test_player_state_from_atomic_play() {
    let state = PlayerState::from_atomic(1);
    assert_eq!(state, PlayerState::Play);
}

#[test]
fn test_player_state_from_atomic_pause() {
    let state = PlayerState::from_atomic(2);
    assert_eq!(state, PlayerState::Pause);
}

#[test]
fn test_player_state_from_atomic_invalid() {
    let state = PlayerState::from_atomic(99);
    assert_eq!(state, PlayerState::Stop);
}

#[test]
fn test_player_state_display() {
    assert_eq!(PlayerState::Stop.to_string(), "stop");
    assert_eq!(PlayerState::Play.to_string(), "play");
    assert_eq!(PlayerState::Pause.to_string(), "pause");
}

#[test]
fn test_replay_gain_mode_as_str() {
    assert_eq!(ReplayGainMode::Off.as_str(), "off");
    assert_eq!(ReplayGainMode::Track.as_str(), "track");
    assert_eq!(ReplayGainMode::Album.as_str(), "album");
    assert_eq!(ReplayGainMode::Auto.as_str(), "auto");
}

#[test]
fn test_replay_gain_mode_from_str() {
    assert_eq!(ReplayGainMode::parse_mode("off"), ReplayGainMode::Off);
    assert_eq!(ReplayGainMode::parse_mode("track"), ReplayGainMode::Track);
    assert_eq!(ReplayGainMode::parse_mode("album"), ReplayGainMode::Album);
    assert_eq!(ReplayGainMode::parse_mode("auto"), ReplayGainMode::Auto);
}

#[test]
fn test_replay_gain_mode_from_str_invalid() {
    assert_eq!(ReplayGainMode::parse_mode("invalid"), ReplayGainMode::Off);
    assert_eq!(ReplayGainMode::parse_mode(""), ReplayGainMode::Off);
}

#[test]
fn test_replay_gain_mode_round_trip() {
    let modes = vec![
        ReplayGainMode::Off,
        ReplayGainMode::Track,
        ReplayGainMode::Album,
        ReplayGainMode::Auto,
    ];

    for mode in modes {
        let as_str = mode.as_str();
        let from_str = ReplayGainMode::parse_mode(as_str);
        assert_eq!(mode, from_str);
    }
}

#[test]
fn test_replay_gain_mode_display() {
    assert_eq!(ReplayGainMode::Off.to_string(), "off");
    assert_eq!(ReplayGainMode::Track.to_string(), "track");
    assert_eq!(ReplayGainMode::Album.to_string(), "album");
    assert_eq!(ReplayGainMode::Auto.to_string(), "auto");
}

#[test]
fn test_player_status_default() {
    let status = PlayerStatus::default();

    assert_eq!(status.state, PlayerState::Stop);
    assert_eq!(status.volume, 100);
    assert!(!status.repeat);
    assert!(!status.random);
    assert_eq!(status.single, SingleMode::Off);
    assert_eq!(status.consume, ConsumeMode::Off);
    assert!(status.current_song.is_none());
    assert!(status.next_song.is_none());
    assert_eq!(status.elapsed, None);
    assert_eq!(status.duration, None);
    assert_eq!(status.bitrate, None);
    assert_eq!(status.audio_format, None);
    assert_eq!(status.crossfade, 0);
    assert_eq!(status.mixramp_db, 0.0);
    assert_eq!(status.mixramp_delay, 0.0);
    assert_eq!(status.playlist_version, 0);
    assert_eq!(status.playlist_length, 0);
    assert_eq!(status.updating_db, None);
    assert_eq!(status.error, None);
    assert_eq!(status.replay_gain_mode, ReplayGainMode::Off);
}

#[test]
fn test_player_status_sensible_defaults() {
    let status = PlayerStatus::default();

    // Volume should be at a reasonable level
    assert!(status.volume <= 100);
    assert!(status.volume > 0);

    // Flags should be off by default
    assert!(!status.repeat);
    assert!(!status.random);

    // Replay gain should be off by default
    assert_eq!(status.replay_gain_mode, ReplayGainMode::Off);
}

#[test]
fn test_player_status_with_custom_values() {
    let status = PlayerStatus {
        state: PlayerState::Play,
        volume: 75,
        repeat: true,
        random: true,
        replay_gain_mode: ReplayGainMode::Album,
        ..Default::default()
    };

    assert_eq!(status.state, PlayerState::Play);
    assert_eq!(status.volume, 75);
    assert!(status.repeat);
    assert!(status.random);
    assert_eq!(status.replay_gain_mode, ReplayGainMode::Album);
}

#[test]
fn test_player_status_display() {
    let status = PlayerStatus {
        state: PlayerState::Play,
        volume: 80,
        repeat: true,
        random: false,
        playlist_length: 42,
        ..Default::default()
    };

    let display = status.to_string();

    assert!(display.contains("play"));
    assert!(display.contains("80"));
    assert!(display.contains("on")); // repeat is on
    assert!(display.contains("42")); // playlist_length
}

#[test]
fn test_single_mode_default() {
    let mode = SingleMode::default();
    assert_eq!(mode, SingleMode::Off);
}

#[test]
fn test_consume_mode_default() {
    let mode = ConsumeMode::default();
    assert_eq!(mode, ConsumeMode::Off);
}

#[test]
fn test_player_state_default() {
    let state = PlayerState::default();
    assert_eq!(state, PlayerState::Stop);
}

#[test]
fn test_player_state_equality() {
    assert_eq!(PlayerState::Stop, PlayerState::Stop);
    assert_eq!(PlayerState::Play, PlayerState::Play);
    assert_eq!(PlayerState::Pause, PlayerState::Pause);
    assert_ne!(PlayerState::Stop, PlayerState::Play);
}

#[test]
fn test_replay_gain_mode_equality() {
    assert_eq!(ReplayGainMode::Off, ReplayGainMode::Off);
    assert_eq!(ReplayGainMode::Track, ReplayGainMode::Track);
    assert_ne!(ReplayGainMode::Off, ReplayGainMode::Track);
}

#[test]
fn test_player_status_with_durations() {
    let status = PlayerStatus {
        elapsed: Some(Duration::from_secs(30)),
        duration: Some(Duration::from_secs(180)),
        ..Default::default()
    };

    assert_eq!(status.elapsed, Some(Duration::from_secs(30)));
    assert_eq!(status.duration, Some(Duration::from_secs(180)));
}

#[test]
fn test_player_status_with_error() {
    let status = PlayerStatus {
        error: Some("Playback error".to_string()),
        ..Default::default()
    };

    assert_eq!(status.error, Some("Playback error".to_string()));
}
