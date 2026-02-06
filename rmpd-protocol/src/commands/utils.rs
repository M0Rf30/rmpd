//! Shared utility functions and constants for command handlers

use crate::response::ResponseBuilder;

/// MPD protocol error codes (ACK error types)
/// Reference: https://mpd.readthedocs.io/en/latest/protocol.html#ack-errors
pub const ACK_ERROR_ARG: i32 = 2;
pub const ACK_ERROR_UNKNOWN: i32 = 5;
pub const ACK_ERROR_SYSTEM: i32 = 50;

/// Open the music database, returning an error response string on failure.
/// This eliminates the repeated db_path check + Database::open boilerplate.
pub fn open_db(
    state: &crate::state::AppState,
    command: &str,
) -> Result<rmpd_library::Database, String> {
    let db_path = state
        .db_path
        .as_ref()
        .ok_or_else(|| ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, command, "database not configured"))?;
    rmpd_library::Database::open(db_path)
        .map_err(|e| ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, command, &format!("database error: {e}")))
}

/// Convert Unix timestamp to ISO 8601 format (RFC 3339)
pub fn format_iso8601_timestamp(timestamp: i64) -> String {
    const SECONDS_PER_MINUTE: i64 = 60;
    const SECONDS_PER_HOUR: i64 = 3600;
    const SECONDS_PER_DAY: i64 = 86400;

    let mut days = timestamp / SECONDS_PER_DAY;
    let remaining = timestamp % SECONDS_PER_DAY;
    let hours = remaining / SECONDS_PER_HOUR;
    let minutes = (remaining % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let seconds = remaining % SECONDS_PER_MINUTE;

    // Calculate year starting from 1970
    let mut year = 1970;
    loop {
        let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap_year { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    // Calculate month and day
    let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let days_in_month = if leap_year {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &dim in &days_in_month {
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    let day = days + 1;

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Check if a song's tag matches an exact value.
pub fn song_tag_eq(song: &rmpd_core::song::Song, tag: &str, value: &str) -> bool {
    match tag {
        "artist" => song.artist.as_deref() == Some(value),
        "album" => song.album.as_deref() == Some(value),
        "title" => song.title.as_deref() == Some(value),
        "genre" => song.genre.as_deref() == Some(value),
        "albumartist" => song.album_artist.as_deref() == Some(value),
        "composer" => song.composer.as_deref() == Some(value),
        "performer" => song.performer.as_deref() == Some(value),
        "date" => song.date.as_deref() == Some(value),
        _ => false,
    }
}

/// Check if a song's tag contains a value (case-insensitive).
pub fn song_tag_contains(song: &rmpd_core::song::Song, tag: &str, value_lower: &str) -> bool {
    let field = match tag {
        "artist" => song.artist.as_deref(),
        "album" => song.album.as_deref(),
        "title" => song.title.as_deref(),
        "genre" => song.genre.as_deref(),
        "albumartist" => song.album_artist.as_deref(),
        "composer" => song.composer.as_deref(),
        "performer" => song.performer.as_deref(),
        "date" => song.date.as_deref(),
        _ => return false,
    };
    field
        .map(|s| s.to_lowercase().contains(value_lower))
        .unwrap_or(false)
}

/// Build a FilterExpression from multiple tag/value pairs joined with AND.
/// Panics if `filters` is empty.
pub fn build_and_filter(filters: &[(String, String)]) -> rmpd_core::filter::FilterExpression {
    use rmpd_core::filter::{CompareOp, FilterExpression};

    let mut expr = FilterExpression::Compare {
        tag: filters[0].0.clone(),
        op: CompareOp::Equal,
        value: filters[0].1.clone(),
    };

    for filter in &filters[1..] {
        let next_expr = FilterExpression::Compare {
            tag: filter.0.clone(),
            op: CompareOp::Equal,
            value: filter.1.clone(),
        };
        expr = FilterExpression::And(Box::new(expr), Box::new(next_expr));
    }

    expr
}

/// Apply a range/window filter to a slice, returning the filtered sub-slice.
pub fn apply_range<T>(items: &[T], range: Option<(u32, u32)>) -> &[T] {
    if let Some((start, end)) = range {
        let start_idx = start as usize;
        let end_idx = end.min(items.len() as u32) as usize;
        if start_idx < items.len() {
            &items[start_idx..end_idx]
        } else {
            &[]
        }
    } else {
        items
    }
}

/// Append queue item metadata (priority, range) to the response.
pub fn add_queue_item_metadata(resp: &mut crate::response::ResponseBuilder, item: &rmpd_core::queue::QueueItem) {
    if item.priority > 0 {
        resp.field("Prio", item.priority);
    }
    if let Some((start, end)) = item.range {
        resp.field("Range", format!("{start:.3}-{end:.3}"));
    }
}

/// Update next_song in status based on the current position in the queue.
pub fn update_next_song(
    status: &mut rmpd_core::state::PlayerStatus,
    queue: &rmpd_core::queue::Queue,
    current_pos: u32,
) {
    status.next_song = queue.get(current_pos + 1).map(|next_item| {
        rmpd_core::state::QueuePosition {
            position: current_pos + 1,
            id: next_item.id,
        }
    });
}

/// Clone a song and resolve its path to an absolute path for playback.
pub fn prepare_song_for_playback(song: &rmpd_core::song::Song, music_dir: Option<&str>) -> rmpd_core::song::Song {
    let mut playback_song = song.clone();
    playback_song.path = resolve_path(song.path.as_str(), music_dir).into();
    playback_song
}

/// Resolve a relative path to an absolute path using the music directory.
/// If the path is already absolute, returns it as-is.
pub fn resolve_path(rel_path: &str, music_dir: Option<&str>) -> String {
    if rel_path.starts_with('/') {
        return rel_path.to_string();
    }

    if let Some(music_dir) = music_dir {
        let music_dir = music_dir.trim_end_matches('/');
        format!("{music_dir}/{rel_path}")
    } else {
        rel_path.to_string()
    }
}
