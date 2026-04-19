/// Shared tag utilities: fallback chains, normalization, and canonical mappings.

/// Return the fallback chain for a tag (MPD's Fallback.hxx).
/// For most tags, returns a single-element vec.
pub fn tag_fallback_chain(tag: &str) -> Vec<&str> {
    match tag {
        "albumartist" => vec!["albumartist", "artist"],
        "artistsort" => vec!["artistsort", "artist"],
        "albumartistsort" => vec!["albumartistsort", "albumartist", "artistsort", "artist"],
        "albumsort" => vec!["albumsort", "album"],
        "titlesort" => vec!["titlesort", "title"],
        "composersort" => vec!["composersort", "composer"],
        "artist" => vec!["artist"],
        "album" => vec!["album"],
        "title" => vec!["title"],
        "track" => vec!["track"],
        "date" => vec!["date"],
        "originaldate" => vec!["originaldate"],
        "genre" => vec!["genre"],
        "composer" => vec!["composer"],
        "performer" => vec!["performer"],
        "disc" => vec!["disc"],
        "comment" => vec!["comment"],
        "grouping" => vec!["grouping"],
        "label" => vec!["label"],
        "musicbrainz_artistid" => vec!["musicbrainz_artistid"],
        "musicbrainz_albumid" => vec!["musicbrainz_albumid"],
        "musicbrainz_albumartistid" => vec!["musicbrainz_albumartistid"],
        "musicbrainz_trackid" => vec!["musicbrainz_trackid"],
        "musicbrainz_releasetrackid" => vec!["musicbrainz_releasetrackid"],
        "musicbrainz_releasegroupid" => vec!["musicbrainz_releasegroupid"],
        "musicbrainz_workid" => vec!["musicbrainz_workid"],
        _ => vec![tag], // unknown tags: search as-is
    }
}

/// Normalize a Track or Disc value the same way MPD does (Handler.cxx NormalizeDecimal):
/// strip leading zeros, strip non-digit suffix, treat all-zero result as empty (skip).
pub fn normalize_decimal(s: &str) -> Option<String> {
    let s = s.trim();
    // Find first non-zero digit
    let start = s.chars().position(|c| c != '0').unwrap_or(s.len());
    // Take only ASCII digits from that position
    let tail: String = s[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if tail.is_empty() {
        // All digits were zeros (e.g. "0", "00") -- preserve as "0"
        if s.starts_with('0') {
            Some("0".to_string())
        } else {
            None
        }
    } else {
        Some(tail)
    }
}

/// MPD-canonical VorbisComment key -> tag name mapping.
/// Derived from MPD's tag/Names.cxx (tag_item_names) + lib/xiph/XiphTags.cxx.
/// Only these exact key names (case-insensitive) are recognized for structured tags.
pub const VORBIS_TAG_MAP: &[(&str, &str)] = &[
    ("title", "title"),
    ("artist", "artist"),
    ("album", "album"),
    ("albumartist", "albumartist"),
    ("albumartistsort", "albumartistsort"),
    ("artistsort", "artistsort"),
    ("composer", "composer"),
    ("composersort", "composersort"),
    ("performer", "performer"),
    ("conductor", "conductor"),
    ("comment", "comment"),
    ("description", "comment"),
    ("genre", "genre"),
    ("mood", "mood"),
    ("work", "work"),
    ("movement", "movement"),
    ("movementnumber", "movementnumber"),
    ("ensemble", "ensemble"),
    ("location", "location"),
    ("grouping", "grouping"),
    ("track", "track"),
    ("tracknumber", "track"),
    ("disc", "disc"),
    ("discnumber", "disc"),
    ("date", "date"),
    ("originaldate", "originaldate"),
    ("label", "label"),
    ("musicbrainz_trackid", "musicbrainz_trackid"),
    ("musicbrainz_albumid", "musicbrainz_albumid"),
    ("musicbrainz_artistid", "musicbrainz_artistid"),
    ("musicbrainz_albumartistid", "musicbrainz_albumartistid"),
    ("musicbrainz_releasegroupid", "musicbrainz_releasegroupid"),
    ("musicbrainz_releasetrackid", "musicbrainz_releasetrackid"),
    ("musicbrainz_workid", "musicbrainz_workid"),
];
