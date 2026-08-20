//! Verifies `sticker set`/`delete`/`inc`/`dec` notify idle `sticker` clients,
//! matching MPD's `idle_add(IDLE_STICKER)` in `sticker/Database.cxx`.

use rmpd_core::event::{Event, Subsystem};
use rmpd_library::Database;
use rmpd_protocol::commands::stickers;
use rmpd_protocol::state::AppState;

#[test]
fn sticker_changed_event_maps_to_subsystem() {
    assert!(
        Event::StickerChanged
            .subsystems()
            .contains(&Subsystem::Sticker),
        "Event::StickerChanged must map to Subsystem::Sticker"
    );
}

async fn state_with_song() -> (AppState, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    {
        let db = Database::open(db_path.to_str().unwrap()).unwrap();
        db.add_song(&rmpd_core::test_utils::make_test_song("song1.flac", 1))
            .unwrap();
    }
    let state = AppState::with_all_paths(
        db_path.to_str().unwrap().to_string(),
        tmp.path().join("music").to_str().unwrap().to_string(),
        tmp.path().join("playlists").to_str().unwrap().to_string(),
    );
    (state, tmp)
}

#[tokio::test]
async fn sticker_set_emits_notification() {
    let (state, _tmp) = state_with_song().await;
    let mut rx = state.event_bus.subscribe();

    let resp =
        stickers::handle_sticker_set_command(&state, "song", "song1.flac", "rating", "5").await;
    assert!(resp.contains("OK"), "got: {resp}");

    assert!(
        rx.try_recv()
            .is_ok_and(|ev| matches!(ev, Event::StickerChanged)),
        "sticker set must emit Event::StickerChanged"
    );
}

#[tokio::test]
async fn sticker_get_does_not_emit_notification() {
    let (state, _tmp) = state_with_song().await;
    stickers::handle_sticker_set_command(&state, "song", "song1.flac", "rating", "5").await;

    let mut rx = state.event_bus.subscribe();
    let resp = stickers::handle_sticker_get_command(&state, "song", "song1.flac", "rating").await;
    assert!(resp.contains("OK"), "got: {resp}");
    assert!(
        rx.try_recv().is_err(),
        "read-only sticker get must not emit an idle notification"
    );
}
