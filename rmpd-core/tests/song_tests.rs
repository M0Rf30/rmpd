use rmpd_core::song::{intern_tag_key, Song};
use rmpd_core::test_utils::create_test_song_with_metadata;
use std::borrow::Cow;

#[test]
fn test_intern_tag_key_known_tag_borrowed() {
    let key = intern_tag_key("artist");
    
    // Known tags should return Cow::Borrowed
    assert!(matches!(key, Cow::Borrowed(_)));
    assert_eq!(key.as_ref(), "artist");
}

#[test]
fn test_intern_tag_key_known_tags() {
    let known_tags = vec![
        "artist", "album", "title", "track", "name", "genre", "date",
        "composer", "performer", "comment", "disc", "label", "albumartist",
        "musicbrainz_artistid", "musicbrainz_albumid",
    ];
    
    for tag in known_tags {
        let key = intern_tag_key(tag);
        assert_eq!(key.as_ref(), tag);
        assert!(matches!(key, Cow::Borrowed(_)));
    }
}

#[test]
fn test_intern_tag_key_unknown_tag_owned() {
    let key = intern_tag_key("custom_tag");
    
    // Unknown tags should return Cow::Owned
    assert!(matches!(key, Cow::Owned(_)));
    assert_eq!(key.as_ref(), "custom_tag");
}

#[test]
fn test_intern_tag_key_case_insensitive() {
    let key1 = intern_tag_key("Artist");
    let key2 = intern_tag_key("ARTIST");
    let key3 = intern_tag_key("artist");
    
    // All should normalize to lowercase
    assert_eq!(key1.as_ref(), "artist");
    assert_eq!(key2.as_ref(), "artist");
    assert_eq!(key3.as_ref(), "artist");
}

#[test]
fn test_song_tag_retrieval() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        Some("Test Title"),
        Some("Test Artist"),
        Some("Test Album"),
    );
    
    assert_eq!(song.tag("title"), Some("Test Title"));
    assert_eq!(song.tag("artist"), Some("Test Artist"));
    assert_eq!(song.tag("album"), Some("Test Album"));
}

#[test]
fn test_song_tag_case_insensitive() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        Some("Test Title"),
        Some("Test Artist"),
        None,
    );
    
    // Tag lookup should be case-insensitive
    assert_eq!(song.tag("title"), Some("Test Title"));
    assert_eq!(song.tag("Title"), Some("Test Title"));
    assert_eq!(song.tag("TITLE"), Some("Test Title"));
}

#[test]
fn test_song_tag_not_found() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        Some("Test Title"),
        None,
        None,
    );
    
    assert_eq!(song.tag("artist"), None);
    assert_eq!(song.tag("genre"), None);
}

#[test]
fn test_song_with_empty_tags() {
    let song = Song {
        id: 1,
        path: "test.mp3".into(),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: vec![],
    };
    
    assert_eq!(song.tag("title"), None);
    assert_eq!(song.tag("artist"), None);
    assert_eq!(song.tag_values("title").count(), 0);
}

#[test]
fn test_song_tag_values_single() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        Some("Test Title"),
        Some("Test Artist"),
        None,
    );
    
    let values: Vec<&str> = song.tag_values("title").collect();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0], "Test Title");
}

#[test]
fn test_song_tag_values_multiple() {
    let song = Song {
        id: 1,
        path: "test.mp3".into(),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: vec![
            (intern_tag_key("artist"), "Artist 1".to_string()),
            (intern_tag_key("artist"), "Artist 2".to_string()),
            (intern_tag_key("title"), "Test Title".to_string()),
        ],
    };
    
    let values: Vec<&str> = song.tag_values("artist").collect();
    assert_eq!(values.len(), 2);
    assert!(values.contains(&"Artist 1"));
    assert!(values.contains(&"Artist 2"));
}

#[test]
fn test_song_display_title() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        Some("My Title"),
        None,
        None,
    );
    
    assert_eq!(song.display_title(), "My Title");
}

#[test]
fn test_song_display_title_fallback_to_filename() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        None,
        None,
        None,
    );
    
    assert_eq!(song.display_title(), "test.mp3");
}

#[test]
fn test_song_display_artist() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        None,
        Some("Test Artist"),
        None,
    );
    
    assert_eq!(song.display_artist(), "Test Artist");
}

#[test]
fn test_song_display_artist_fallback_to_albumartist() {
    let song = Song {
        id: 1,
        path: "test.mp3".into(),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: vec![
            (intern_tag_key("albumartist"), "Album Artist".to_string()),
        ],
    };
    
    assert_eq!(song.display_artist(), "Album Artist");
}

#[test]
fn test_song_display_artist_fallback_to_unknown() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        None,
        None,
        None,
    );
    
    assert_eq!(song.display_artist(), "Unknown Artist");
}

#[test]
fn test_song_display_album() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        None,
        None,
        Some("Test Album"),
    );
    
    assert_eq!(song.display_album(), "Test Album");
}

#[test]
fn test_song_display_album_fallback_to_unknown() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        None,
        None,
        None,
    );
    
    assert_eq!(song.display_album(), "Unknown Album");
}

#[test]
fn test_song_tag_eq() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        Some("Test Title"),
        Some("Test Artist"),
        None,
    );
    
    assert!(song.tag_eq("title", "Test Title"));
    assert!(song.tag_eq("artist", "Test Artist"));
    assert!(!song.tag_eq("title", "Wrong Title"));
}

#[test]
fn test_song_tag_contains() {
    let song = create_test_song_with_metadata(
        1,
        "test.mp3",
        Some("Test Title"),
        Some("Test Artist"),
        None,
    );
    
    assert!(song.tag_contains("title", "test"));
    assert!(song.tag_contains("title", "title"));
    assert!(!song.tag_contains("title", "wrong"));
}

#[test]
fn test_intern_tag_key_all_known_tags() {
    let known_tags = vec![
        "artist", "artistsort", "album", "albumsort", "albumartist",
        "albumartistsort", "title", "titlesort", "track", "name",
        "genre", "mood", "date", "originaldate", "composer",
        "composersort", "performer", "conductor", "work", "movement",
        "movementnumber", "ensemble", "location", "grouping", "comment",
        "disc", "label", "musicbrainz_artistid", "musicbrainz_albumid",
        "musicbrainz_albumartistid", "musicbrainz_trackid",
        "musicbrainz_releasetrackid", "musicbrainz_workid",
        "musicbrainz_releasegroupid",
    ];
    
    for tag in known_tags {
        let key = intern_tag_key(tag);
        assert_eq!(key.as_ref(), tag);
        assert!(matches!(key, Cow::Borrowed(_)), "Tag {} should be borrowed", tag);
    }
}

#[test]
fn test_song_tag_with_fallback() {
    let song = Song {
        id: 1,
        path: "test.mp3".into(),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: vec![
            (intern_tag_key("artist"), "Test Artist".to_string()),
        ],
    };
    
    // albumartist should fall back to artist
    assert_eq!(song.tag_with_fallback("albumartist"), Some("Test Artist"));
    // artist should return itself
    assert_eq!(song.tag_with_fallback("artist"), Some("Test Artist"));
}
