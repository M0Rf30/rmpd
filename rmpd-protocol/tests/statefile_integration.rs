use rmpd_core::state::{ConsumeMode, PlayerState, SingleMode};
use rmpd_protocol::statefile::StateFile;

mod common;
use common::state_helpers::{create_test_queue, save_and_load, StatusBuilder, TempStateFile};

#[tokio::test]
async fn test_save_and_load_with_queue() {
    let queue = create_test_queue(5);
    let status = StatusBuilder::new()
        .volume(85)
        .state(PlayerState::Play)
        .current_position(2, 2)
        .elapsed(42)
        .repeat(true)
        .build(5);

    let loaded = save_and_load(&status, &queue).await.unwrap();

    assert_eq!(loaded.volume, 85);
    assert_eq!(loaded.state, Some(PlayerState::Play));
    assert_eq!(loaded.current_position, Some(2));
    assert_eq!(loaded.elapsed_seconds, Some(42.0));
    assert_eq!(loaded.repeat, true);
    assert_eq!(loaded.playlist_paths.len(), 5);
}

#[tokio::test]
async fn test_all_playback_options_roundtrip() {
    let queue = create_test_queue(3);
    let status = StatusBuilder::new()
        .volume(75)
        .state(PlayerState::Pause)
        .current_position(1, 1)
        .elapsed(30)
        .random(true)
        .repeat(true)
        .single(SingleMode::Oneshot)
        .consume(ConsumeMode::On)
        .crossfade(5)
        .mixramp_db(-17.0)
        .mixramp_delay(2.0)
        .build(3);

    let loaded = save_and_load(&status, &queue).await.unwrap();

    assert_eq!(loaded.volume, 75);
    assert_eq!(loaded.state, Some(PlayerState::Pause));
    assert_eq!(loaded.random, true);
    assert_eq!(loaded.repeat, true);
    assert_eq!(loaded.single, SingleMode::Oneshot);
    assert_eq!(loaded.consume, ConsumeMode::On);
    assert_eq!(loaded.crossfade, 5);
    assert_eq!(loaded.mixramp_db, -17.0);
    assert_eq!(loaded.mixramp_delay, 2.0);
}

#[tokio::test]
async fn test_empty_queue_state() {
    let queue = create_test_queue(0);
    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Stop)
        .build(0);

    let loaded = save_and_load(&status, &queue).await.unwrap();

    assert_eq!(loaded.playlist_paths.len(), 0);
    assert_eq!(loaded.current_position, None);
    assert_eq!(loaded.elapsed_seconds, None);
}

#[tokio::test]
async fn test_large_queue_performance() {
    let queue = create_test_queue(5000);
    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Stop)
        .build(5000);

    let start = std::time::Instant::now();
    let loaded = save_and_load(&status, &queue).await.unwrap();
    let elapsed = start.elapsed();

    // Should complete within reasonable time (< 1 second for 5000 songs)
    assert!(elapsed.as_secs() < 1, "Took too long: {elapsed:?}");
    assert_eq!(loaded.playlist_paths.len(), 5000);
}

#[tokio::test]
async fn test_state_file_format_compatibility() {
    // Test that we can read a manually created state file
    let content = r#"sw_volume: 80
state: play
current: 1
time: 42.500000
random: 1
repeat: 0
single: 2
consume: 1
crossfade: 3
mixrampdb: -17.000000
mixrampdelay: 2.500000
playlist_begin
0:/music/song1.mp3
1:/music/song2.mp3
2:/music/song3.mp3
playlist_end
"#;

    let temp = TempStateFile::new(content);
    let statefile = StateFile::new(temp.path_str());

    let loaded = statefile.load().unwrap().unwrap();

    assert_eq!(loaded.volume, 80);
    assert_eq!(loaded.state, Some(PlayerState::Play));
    assert_eq!(loaded.current_position, Some(1));
    assert_eq!(loaded.elapsed_seconds, Some(42.5));
    assert_eq!(loaded.random, true);
    assert_eq!(loaded.repeat, false);
    assert_eq!(loaded.single, SingleMode::Oneshot);
    assert_eq!(loaded.consume, ConsumeMode::On);
    assert_eq!(loaded.crossfade, 3);
    assert_eq!(loaded.mixramp_db, -17.0);
    assert_eq!(loaded.mixramp_delay, 2.5);
    assert_eq!(loaded.playlist_paths.len(), 3);
    assert_eq!(loaded.playlist_paths[0], "/music/song1.mp3");
}

#[tokio::test]
async fn test_concurrent_save_load() {
    use tokio::task;

    let queue = create_test_queue(10);
    let status = StatusBuilder::new()
        .volume(90)
        .state(PlayerState::Play)
        .build(10);

    let mut handles = vec![];

    // Spawn multiple concurrent save/load operations
    for _ in 0..10 {
        let q = queue.clone();
        let s = status.clone();
        let handle = task::spawn(async move { save_and_load(&s, &q).await });
        handles.push(handle);
    }

    // All should succeed
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}

#[test]
fn test_malformed_state_file_recovery() {
    // Test that loading a partially malformed file doesn't crash
    let content = r#"sw_volume: invalid_number
state: unknown_state
current: not_a_number
random: maybe
repeat: yes
playlist_begin
invalid_entry
0:/music/valid.mp3
playlist_end
"#;

    let temp = TempStateFile::new(content);
    let statefile = StateFile::new(temp.path_str());

    // Should not panic, should return Some with default/parsed values
    let loaded = statefile.load().unwrap().unwrap();

    // Invalid values should use defaults
    assert_eq!(loaded.volume, 100); // default on parse error
    assert_eq!(loaded.state, None); // invalid state
    assert_eq!(loaded.current_position, None); // invalid parse
    assert_eq!(loaded.random, false); // invalid bool
    assert_eq!(loaded.repeat, false); // invalid bool

    // Valid playlist entry should be parsed
    assert_eq!(loaded.playlist_paths.len(), 1);
    assert_eq!(loaded.playlist_paths[0], "/music/valid.mp3");
}

#[test]
fn test_future_compatibility_unknown_keys() {
    // Test that unknown keys (future fields) are gracefully ignored
    let content = r#"sw_volume: 100
state: play
future_field_1: some_value
unknown_option: 42
playlist_begin
0:/music/song1.mp3
playlist_end
future_field_2: another_value
"#;

    let temp = TempStateFile::new(content);
    let statefile = StateFile::new(temp.path_str());

    let loaded = statefile.load().unwrap().unwrap();

    // Should parse known fields correctly, ignore unknown ones
    assert_eq!(loaded.volume, 100);
    assert_eq!(loaded.state, Some(PlayerState::Play));
    assert_eq!(loaded.playlist_paths.len(), 1);
}
