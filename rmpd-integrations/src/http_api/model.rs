// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Conversion of rmpd types to Mopidy's JSON data models (`Track`, `TlTrack`,
//! `Ref`, `SearchResult`). Unset fields are omitted, like Mopidy's encoder.

use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use rmpd_plugin::integration::{BrowseEntry, BrowseKind};
use serde_json::{Map, Value, json};

/// Mopidy playback state name.
#[must_use]
pub fn state_name(state: PlayerState) -> &'static str {
    match state {
        PlayerState::Play => "playing",
        PlayerState::Pause => "paused",
        PlayerState::Stop => "stopped",
    }
}

/// Leading integer of a `"3"` / `"3/12"` style tag value.
fn leading_number(value: &str) -> Option<u64> {
    let digits: String = value
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn artists(names: Vec<&str>) -> Value {
    Value::Array(
        names
            .into_iter()
            .map(|n| json!({ "__model__": "Artist", "name": n }))
            .collect(),
    )
}

/// Mopidy `Track` for a song. `uri` is the song's path.
#[must_use]
pub fn track_json(song: &Song) -> Value {
    let mut m = Map::new();
    m.insert("__model__".to_owned(), json!("Track"));
    m.insert("uri".to_owned(), json!(song.path.as_str()));
    m.insert("name".to_owned(), json!(song.display_title()));
    let track_artists: Vec<&str> = song.tag_values("artist").collect();
    if !track_artists.is_empty() {
        m.insert("artists".to_owned(), artists(track_artists));
    }
    if let Some(album) = song.tag("album") {
        let mut a = Map::new();
        a.insert("__model__".to_owned(), json!("Album"));
        a.insert("name".to_owned(), json!(album));
        let album_artists: Vec<&str> = song.tag_values("albumartist").collect();
        if !album_artists.is_empty() {
            a.insert("artists".to_owned(), artists(album_artists));
        }
        if let Some(date) = song.tag("date") {
            a.insert("date".to_owned(), json!(date));
        }
        m.insert("album".to_owned(), Value::Object(a));
    }
    let composers: Vec<&str> = song.tag_values("composer").collect();
    if !composers.is_empty() {
        m.insert("composers".to_owned(), artists(composers));
    }
    if let Some(genre) = song.tag("genre") {
        m.insert("genre".to_owned(), json!(genre));
    }
    if let Some(n) = song.tag("track").and_then(leading_number) {
        m.insert("track_no".to_owned(), json!(n));
    }
    if let Some(n) = song.tag("disc").and_then(leading_number) {
        m.insert("disc_no".to_owned(), json!(n));
    }
    if let Some(date) = song.tag("date") {
        m.insert("date".to_owned(), json!(date));
    }
    if let Some(d) = song.duration {
        m.insert(
            "length".to_owned(),
            json!(u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        );
    }
    if let Some(b) = song.bitrate {
        m.insert("bitrate".to_owned(), json!(b));
    }
    Value::Object(m)
}

/// Mopidy `TlTrack` (`tlid` is the queue id).
#[must_use]
pub fn tl_track_json(tlid: u32, song: &Song) -> Value {
    json!({ "__model__": "TlTrack", "tlid": tlid, "track": track_json(song) })
}

/// Mopidy `Ref` for a browse entry.
#[must_use]
pub fn ref_json(entry: &BrowseEntry) -> Value {
    json!({
        "__model__": "Ref",
        "type": match entry.kind {
            BrowseKind::Directory => "directory",
            BrowseKind::Track => "track",
        },
        "name": entry.name,
        "uri": entry.uri,
    })
}

/// Mopidy `SearchResult` holding `songs`.
#[must_use]
pub fn search_result_json(songs: &[Song]) -> Value {
    json!({
        "__model__": "SearchResult",
        "uri": "rmpd:search",
        "tracks": songs.iter().map(track_json).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_api::testutil::song;

    #[test]
    fn state_names_follow_mopidy() {
        assert_eq!(state_name(PlayerState::Play), "playing");
        assert_eq!(state_name(PlayerState::Pause), "paused");
        assert_eq!(state_name(PlayerState::Stop), "stopped");
    }

    #[test]
    fn track_maps_tags_and_length() {
        let s = song(
            "a/b.flac",
            &[
                ("title", "Song"),
                ("artist", "Me"),
                ("album", "LP"),
                ("track", "3/12"),
                ("date", "2020"),
            ],
        );
        let t = track_json(&s);
        assert_eq!(t["__model__"], "Track");
        assert_eq!(t["uri"], "a/b.flac");
        assert_eq!(t["name"], "Song");
        assert_eq!(t["artists"][0]["name"], "Me");
        assert_eq!(t["album"]["name"], "LP");
        assert_eq!(t["track_no"], 3);
        assert_eq!(t["length"], 183_500);
        assert_eq!(t["bitrate"], 320);
        assert!(t.get("genre").is_none());
    }

    #[test]
    fn tl_track_wraps_track() {
        let s = song("x.mp3", &[]);
        let t = tl_track_json(7, &s);
        assert_eq!(t["__model__"], "TlTrack");
        assert_eq!(t["tlid"], 7);
        assert_eq!(t["track"]["name"], "x.mp3");
    }

    #[test]
    fn ref_kinds() {
        let d = BrowseEntry {
            kind: BrowseKind::Directory,
            uri: "a".to_owned(),
            name: "a".to_owned(),
        };
        assert_eq!(ref_json(&d)["type"], "directory");
        let t = BrowseEntry {
            kind: BrowseKind::Track,
            uri: "a/b".to_owned(),
            name: "b".to_owned(),
        };
        assert_eq!(ref_json(&t)["type"], "track");
    }

    #[test]
    fn leading_number_parses_prefix() {
        assert_eq!(leading_number("3/12"), Some(3));
        assert_eq!(leading_number("x"), None);
    }
}
