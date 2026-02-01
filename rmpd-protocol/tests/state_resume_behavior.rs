/// Tests for state persistence and resume behavior simulating full restarts
///
/// These tests simulate the full lifecycle of saving state, "restarting"
/// the application, and restoring the previous state from the saved file.

use rmpd_core::queue::Queue;
use rmpd_core::state::{ConsumeMode, PlayerState, SingleMode};
use rmpd_protocol::statefile::{SavedState, StateFile};

mod common;
use common::state_helpers::{create_test_queue, create_test_song, StatusBuilder, TempStateFile};

/// Simulates a full restart cycle: save state, "restart", restore state
async fn simulate_restart(
    initial_status: &rmpd_core::state::PlayerStatus,
    initial_queue: &Queue,
) -> SavedState {
    let temp = TempStateFile::new_empty();
    let statefile = StateFile::new(temp.path_str());

    // Save state before "shutdown"
    statefile
        .save(initial_status, initial_queue)
        .await
        .unwrap();

    // Simulate restart by creating a new StateFile instance
    let statefile_after_restart = StateFile::new(temp.path_str());

    // Load state after "restart"
    statefile_after_restart
        .load()
        .unwrap()
        .expect("State file should exist")
}

#[tokio::test]
async fn test_restore_queue_from_state() {
    let queue = create_test_queue(10);
    let status = StatusBuilder::new()
        .volume(85)
        .state(PlayerState::Play)
        .current_position(3, 3)
        .elapsed(42)
        .build(10);

    let restored = simulate_restart(&status, &queue).await;

    assert_eq!(restored.playlist_paths.len(), 10);
    assert_eq!(restored.playlist_paths[0], "/music/song0.mp3");
    assert_eq!(restored.playlist_paths[9], "/music/song9.mp3");
    assert_eq!(restored.current_position, Some(3));
    assert_eq!(restored.volume, 85);
}

#[tokio::test]
async fn test_restore_with_missing_songs() {
    // Simulate a scenario where the queue has songs, but after restart
    // some songs might not be available in the database
    let mut queue = Queue::new();
    queue.add(create_test_song("/music/song1.mp3", 1));
    queue.add(create_test_song("/music/deleted.mp3", 2));
    queue.add(create_test_song("/music/song3.mp3", 3));

    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Stop)
        .build(3);

    let restored = simulate_restart(&status, &queue).await;

    // All paths should be restored, even if files don't exist yet
    assert_eq!(restored.playlist_paths.len(), 3);
    assert_eq!(restored.playlist_paths[0], "/music/song1.mp3");
    assert_eq!(restored.playlist_paths[1], "/music/deleted.mp3");
    assert_eq!(restored.playlist_paths[2], "/music/song3.mp3");
}

#[tokio::test]
async fn test_auto_resume_playback() {
    // Simulate resuming playback from a paused state
    let queue = create_test_queue(5);
    let status = StatusBuilder::new()
        .volume(70)
        .state(PlayerState::Play)
        .current_position(2, 2)
        .elapsed(120)
        .build(5);

    let restored = simulate_restart(&status, &queue).await;

    assert_eq!(restored.state, Some(PlayerState::Play));
    assert_eq!(restored.current_position, Some(2));
    assert_eq!(restored.elapsed_seconds, Some(120.0));
}

#[tokio::test]
async fn test_restore_paused_mode() {
    let queue = create_test_queue(3);
    let status = StatusBuilder::new()
        .volume(50)
        .state(PlayerState::Pause)
        .current_position(1, 1)
        .elapsed(90)
        .build(3);

    let restored = simulate_restart(&status, &queue).await;

    assert_eq!(restored.state, Some(PlayerState::Pause));
    assert_eq!(restored.current_position, Some(1));
    assert_eq!(restored.elapsed_seconds, Some(90.0));
}

#[tokio::test]
async fn test_elapsed_time_seek() {
    // Test that elapsed time is preserved for seeking after restart
    let queue = create_test_queue(1);
    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Pause)
        .current_position(0, 0)
        .elapsed(157) // 2:37
        .build(1);

    let restored = simulate_restart(&status, &queue).await;

    assert_eq!(restored.elapsed_seconds, Some(157.0));
}

#[tokio::test]
async fn test_restore_playback_options() {
    let queue = create_test_queue(10);
    let status = StatusBuilder::new()
        .volume(80)
        .state(PlayerState::Play)
        .random(true)
        .repeat(true)
        .single(SingleMode::Oneshot)
        .consume(ConsumeMode::On)
        .crossfade(10)
        .mixramp_db(-12.0)
        .mixramp_delay(5.0)
        .build(10);

    let restored = simulate_restart(&status, &queue).await;

    assert!(restored.random);
    assert!(restored.repeat);
    assert_eq!(restored.single, SingleMode::Oneshot);
    assert_eq!(restored.consume, ConsumeMode::On);
    assert_eq!(restored.crossfade, 10);
    assert_eq!(restored.mixramp_db, -12.0);
    assert_eq!(restored.mixramp_delay, 5.0);
}

#[tokio::test]
async fn test_restore_stopped_state() {
    let queue = create_test_queue(5);
    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Stop)
        .build(5);

    let restored = simulate_restart(&status, &queue).await;

    assert_eq!(restored.state, Some(PlayerState::Stop));
    assert_eq!(restored.current_position, None);
    assert_eq!(restored.elapsed_seconds, None);
    assert_eq!(restored.playlist_paths.len(), 5);
}

#[tokio::test]
async fn test_restore_empty_queue() {
    let queue = Queue::new();
    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Stop)
        .build(0);

    let restored = simulate_restart(&status, &queue).await;

    assert_eq!(restored.playlist_paths.len(), 0);
    assert_eq!(restored.current_position, None);
}

#[tokio::test]
async fn test_multiple_restart_cycles() {
    // Test multiple save/load cycles to ensure consistency
    let mut queue = create_test_queue(3);
    let mut status = StatusBuilder::new()
        .volume(75)
        .state(PlayerState::Play)
        .current_position(0, 0)
        .build(3);

    let temp = TempStateFile::new_empty();
    let path = temp.path_str();

    // First save
    let statefile1 = StateFile::new(path.clone());
    statefile1.save(&status, &queue).await.unwrap();

    // First load
    let statefile2 = StateFile::new(path.clone());
    let restored1 = statefile2.load().unwrap().unwrap();
    assert_eq!(restored1.volume, 75);

    // Modify and save again
    status.volume = 60;
    queue.add(create_test_song("/music/new.mp3", 3));
    let statefile3 = StateFile::new(path.clone());
    statefile3.save(&status, &queue).await.unwrap();

    // Load again
    let statefile4 = StateFile::new(path.clone());
    let restored2 = statefile4.load().unwrap().unwrap();
    assert_eq!(restored2.volume, 60);
    assert_eq!(restored2.playlist_paths.len(), 4);
}

#[tokio::test]
async fn test_restore_with_unicode_paths() {
    let mut queue = Queue::new();
    queue.add(create_test_song("/music/日本語/song.mp3", 0));
    queue.add(create_test_song("/music/Ελληνικά/τραγούδι.mp3", 1));
    queue.add(create_test_song("/music/العربية/أغنية.mp3", 2));

    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Stop)
        .build(3);

    let restored = simulate_restart(&status, &queue).await;

    assert_eq!(restored.playlist_paths.len(), 3);
    assert_eq!(restored.playlist_paths[0], "/music/日本語/song.mp3");
    assert_eq!(restored.playlist_paths[1], "/music/Ελληνικά/τραγούδι.mp3");
    assert_eq!(restored.playlist_paths[2], "/music/العربية/أغنية.mp3");
}

#[test]
fn test_restore_from_mpd_state_file() {
    // Test compatibility with actual MPD state file format
    let mpd_state = r#"sw_volume: 85
state: pause
current: 5
time: 67.234567
random: 0
repeat: 1
single: 0
consume: 0
crossfade: 0
mixrampdb: 0.000000
mixrampdelay: -1.000000
playlist_begin
0:/var/lib/mpd/music/artist1/album1/01-track.flac
1:/var/lib/mpd/music/artist1/album1/02-track.flac
2:/var/lib/mpd/music/artist1/album1/03-track.flac
3:/var/lib/mpd/music/artist2/album2/01-track.mp3
4:/var/lib/mpd/music/artist2/album2/02-track.mp3
5:/var/lib/mpd/music/artist3/album3/01-track.ogg
playlist_end
"#;

    let temp = TempStateFile::new(mpd_state);
    let statefile = StateFile::new(temp.path_str());

    let restored = statefile.load().unwrap().unwrap();

    assert_eq!(restored.volume, 85);
    assert_eq!(restored.state, Some(PlayerState::Pause));
    assert_eq!(restored.current_position, Some(5));
    assert!(!restored.random);
    assert!(restored.repeat);
    assert_eq!(restored.playlist_paths.len(), 6);
    assert!(restored.playlist_paths[0].ends_with("01-track.flac"));
}

#[tokio::test]
async fn test_crash_recovery() {
    // Simulate a crash during save by writing incomplete data
    // Then verify we can still load the previous good state

    let temp = TempStateFile::new_empty();
    let path = temp.path_str();

    // First, save a good state
    let queue = create_test_queue(2);
    let status = StatusBuilder::new()
        .volume(100)
        .state(PlayerState::Play)
        .build(2);

    let statefile = StateFile::new(path.clone());
    statefile.save(&status, &queue).await.unwrap();

    // Verify good state loads
    let statefile2 = StateFile::new(path.clone());
    let good_state = statefile2.load().unwrap().unwrap();
    assert_eq!(good_state.volume, 100);

    // Simulate incomplete write (the atomic write should prevent this in practice)
    // but test that loading corrupted data doesn't crash
    std::fs::write(temp.path.as_path(), "sw_volume: 50\nsta").unwrap();

    let statefile3 = StateFile::new(path);
    // Should not panic
    let maybe_corrupted = statefile3.load();
    assert!(maybe_corrupted.is_ok());
}
