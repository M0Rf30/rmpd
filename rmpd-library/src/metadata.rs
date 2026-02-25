use camino::Utf8PathBuf;
use lofty::config::ParseOptions;
use lofty::flac::FlacFile;
use lofty::mp4::{AtomData, AtomIdent, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::{OpusFile, VorbisFile};
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::Song;
use std::fs;
use std::io::BufReader;
use std::time::SystemTime;

use crate::database::system_time_to_unix_secs;

fn is_bogus_dsf_comment(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.len() >= 16 && trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == ' ')
}

/// Normalize a Track or Disc value the same way MPD does (Handler.cxx NormalizeDecimal):
/// strip leading zeros, strip non-digit suffix, treat all-zero result as empty (skip).
fn normalize_decimal(s: &str) -> Option<String> {
    let s = s.trim();
    // Find first non-zero digit
    let start = s.chars().position(|c| c != '0').unwrap_or(s.len());
    // Take only ASCII digits from that position
    let tail: String = s[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if tail.is_empty() {
        // All digits were zeros (e.g. "0", "00") — preserve as "0"
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
const VORBIS_TAG_MAP: &[(&str, &str)] = &[
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

const ITEM_KEY_TAG_MAP: &[(ItemKey, &str)] = &[
    (ItemKey::TrackTitle, "title"),
    (ItemKey::TrackArtist, "artist"),
    (ItemKey::AlbumTitle, "album"),
    (ItemKey::AlbumArtist, "albumartist"),
    (ItemKey::TrackNumber, "track"),
    (ItemKey::DiscNumber, "disc"),
    (ItemKey::Genre, "genre"),
    (ItemKey::Composer, "composer"),
    (ItemKey::Performer, "performer"),
    (ItemKey::Comment, "comment"),
    (ItemKey::ContentGroup, "grouping"),
    (ItemKey::Label, "label"),
    (ItemKey::Publisher, "label"),
    (ItemKey::TrackArtistSortOrder, "artistsort"),
    (ItemKey::AlbumArtistSortOrder, "albumartistsort"),
    (ItemKey::MusicBrainzRecordingId, "musicbrainz_trackid"),
    (ItemKey::MusicBrainzReleaseId, "musicbrainz_albumid"),
    (ItemKey::MusicBrainzArtistId, "musicbrainz_artistid"),
    (
        ItemKey::MusicBrainzReleaseArtistId,
        "musicbrainz_albumartistid",
    ),
    (
        ItemKey::MusicBrainzReleaseGroupId,
        "musicbrainz_releasegroupid",
    ),
    (ItemKey::MusicBrainzTrackId, "musicbrainz_releasetrackid"),
    (ItemKey::MusicBrainzWorkId, "musicbrainz_workid"),
];

#[derive(Debug, Clone)]
pub struct Artwork {
    pub picture_type: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Copy, Clone)]
pub struct MetadataExtractor;

impl MetadataExtractor {
    pub fn extract_from_file(path: &Utf8PathBuf) -> Result<Song> {
        let metadata = fs::metadata(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to read file metadata: {e}")))?;

        let mtime = system_time_to_unix_secs(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));

        let tagged_file = Probe::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?
            .guess_file_type()
            .map_err(|e| RmpdError::Library(format!("Failed to detect file type: {e}")))?
            .read()
            .map_err(|e| RmpdError::Library(format!("Failed to read file: {e}")))?;

        let properties = tagged_file.properties();
        let duration = Some(properties.duration());
        let sample_rate = properties.sample_rate();
        let channels = properties.channels();
        let bitrate = properties.audio_bitrate();

        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag());

        tracing::debug!("extracting metadata from: {}", path);
        let mut tags: Vec<(String, String)> = Vec::new();

        // For VorbisComment-based formats (FLAC/OGG/Opus), use raw key extraction
        // with MPD's canonical key mapping to avoid lofty mapping non-standard key
        // variants (e.g., "ALBUM ARTIST" with space) to the same ItemKey.
        let is_vorbis_format = matches!(
            tagged_file.file_type(),
            lofty::file::FileType::Flac
                | lofty::file::FileType::Vorbis
                | lofty::file::FileType::Opus
        );

        if is_vorbis_format {
            // For VorbisComment-based formats, read raw (key, value) pairs directly
            // using lofty's format-specific API. This preserves the original raw key
            // names and avoids lofty normalizing e.g. "ALBUM ARTIST" (space variant)
            // to the same ItemKey as the canonical "ALBUMARTIST".
            let raw_vc_pairs: Vec<(String, String)> = match tagged_file.file_type() {
                lofty::file::FileType::Flac => {
                    let file = std::fs::File::open(path.as_str()).ok();
                    let mut pairs = Vec::new();
                    if let Some(f) = file {
                        let mut reader = BufReader::new(f);
                        if let Ok(flac) = FlacFile::read_from(&mut reader, ParseOptions::default())
                            && let Some(vc) = flac.vorbis_comments()
                        {
                            for (k, v) in vc.items() {
                                pairs.push((k.to_string(), v.to_string()));
                            }
                        }
                    }
                    pairs
                }
                lofty::file::FileType::Vorbis => {
                    let file = std::fs::File::open(path.as_str()).ok();
                    let mut pairs: Vec<(String, String)> = Vec::new();
                    if let Some(f) = file {
                        let mut reader = BufReader::new(f);
                        if let Ok(ogg) = VorbisFile::read_from(&mut reader, ParseOptions::default())
                        {
                            for (k, v) in ogg.vorbis_comments().items() {
                                pairs.push((k.to_string(), v.to_string()));
                            }
                        }
                    }
                    pairs
                }
                lofty::file::FileType::Opus => {
                    let file = std::fs::File::open(path.as_str()).ok();
                    let mut pairs: Vec<(String, String)> = Vec::new();
                    if let Some(f) = file {
                        let mut reader = BufReader::new(f);
                        if let Ok(opus) = OpusFile::read_from(&mut reader, ParseOptions::default())
                        {
                            for (k, v) in opus.vorbis_comments().items() {
                                pairs.push((k.to_string(), v.to_string()));
                            }
                        }
                    }
                    pairs
                }
                _ => Vec::new(),
            };
            // Apply MPD-canonical key mapping to raw VorbisComment pairs
            for (raw_key, val) in raw_vc_pairs {
                if val.is_empty() {
                    continue;
                }
                let key_lower = raw_key.to_lowercase();
                if let Some(&tag_name) = VORBIS_TAG_MAP
                    .iter()
                    .find(|(k, _)| *k == key_lower)
                    .map(|(_, v)| v)
                {
                    // Normalize Track/Disc: strip leading zeros, preserve zero values
                    let effective_val = if tag_name == "track" || tag_name == "disc" {
                        match normalize_decimal(&val) {
                            Some(v) => v,
                            None => continue,
                        }
                    } else {
                        val
                    };
                    tags.push((tag_name.to_string(), effective_val));
                }
            }
        } else if let Some(tag) = tag {
            let mut seen_tags: Vec<(ItemKey, String)> = Vec::new();
            for item in tag.items() {
                if let Some(val) = item.value().text() {
                    if val.is_empty() {
                        continue;
                    }
                    seen_tags.push((item.key(), val.to_string()));
                }
            }
            for (item_key, tag_name) in ITEM_KEY_TAG_MAP {
                for (key, val) in &seen_tags {
                    if key == item_key {
                        if tag_name == &"comment" && is_bogus_dsf_comment(val) {
                            continue;
                        }
                        // Normalize Track/Disc: strip leading zeros, preserve zero values
                        if *tag_name == "track" || *tag_name == "disc" {
                            if let Some(normalized) = normalize_decimal(val) {
                                tags.push((tag_name.to_string(), normalized));
                            }
                        } else {
                            tags.push((tag_name.to_string(), val.clone()));
                        }
                    }
                }
            }

            // Date: RecordingDate or Year (pick first available via items iteration)
            let mut has_date = false;
            for (key, val) in &seen_tags {
                if *key == ItemKey::RecordingDate {
                    tags.push(("date".to_string(), val.clone()));
                    has_date = true;
                    break;
                }
            }
            if !has_date {
                for (key, val) in &seen_tags {
                    if *key == ItemKey::Year {
                        tags.push(("date".to_string(), val.clone()));
                        break;
                    }
                }
            }

            // OriginalDate: pick the longest (most precise) value
            let mut best_original_date: Option<String> = None;
            for (key, val) in &seen_tags {
                if *key == ItemKey::OriginalReleaseDate
                    && best_original_date
                        .as_ref()
                        .is_none_or(|b| val.len() > b.len())
                {
                    best_original_date = Some(val.clone());
                }
            }
            if let Some(od) = best_original_date {
                tags.push(("originaldate".to_string(), od));
            }

            // TrackNumber and DiscNumber from tag convenience methods (handles "3/12" format)
            // Only add if not already present from items iteration
            if !tags.iter().any(|(k, _)| k == "track")
                && let Some(track) = tag.track()
                && let Some(norm) = normalize_decimal(&track.to_string())
            {
                tags.push(("track".to_string(), norm));
            }
            if !tags.iter().any(|(k, _)| k == "disc")
                && let Some(disc) = tag.disk()
                && let Some(norm) = normalize_decimal(&disc.to_string())
            {
                tags.push(("disc".to_string(), norm));
            }
        }

        let replay_gain = if let Some(tag) = tag {
            (
                tag.get_string(ItemKey::ReplayGainTrackGain)
                    .and_then(|s| s.trim_end_matches(" dB").parse::<f32>().ok()),
                tag.get_string(ItemKey::ReplayGainTrackPeak)
                    .and_then(|s| s.parse::<f32>().ok()),
                tag.get_string(ItemKey::ReplayGainAlbumGain)
                    .and_then(|s| s.trim_end_matches(" dB").parse::<f32>().ok()),
                tag.get_string(ItemKey::ReplayGainAlbumPeak)
                    .and_then(|s| s.parse::<f32>().ok()),
            )
        } else {
            (None, None, None, None)
        };

        Ok(Song {
            id: 0,
            path: path.clone(),
            duration,
            sample_rate,
            channels,
            bits_per_sample: {
                let ext = path.extension().map(|e| e.to_lowercase());
                match ext.as_deref() {
                    Some("m4a" | "aac") => Some(0),
                    _ => Some(properties.bit_depth().unwrap_or(16) as u16),
                }
            },
            bitrate,
            replay_gain_track_gain: replay_gain.0,
            replay_gain_track_peak: replay_gain.1,
            replay_gain_album_gain: replay_gain.2,
            replay_gain_album_peak: replay_gain.3,
            added_at: mtime,
            last_modified: mtime,
            tags,
        })
    }

    pub fn extract_artwork_from_file(path: &Utf8PathBuf) -> Result<Vec<Artwork>> {
        let tagged_file = Probe::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?
            .read()
            .map_err(|e| RmpdError::Library(format!("Failed to read file: {e}")))?;

        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag());

        let mut artworks = Vec::new();

        if let Some(tag) = tag {
            for picture in tag.pictures() {
                let picture_type = match picture.pic_type() {
                    PictureType::Other => "Other",
                    PictureType::Icon => "Icon",
                    PictureType::OtherIcon => "OtherIcon",
                    PictureType::CoverFront => "Front",
                    PictureType::CoverBack => "Back",
                    PictureType::Leaflet => "Leaflet",
                    PictureType::Media => "Media",
                    PictureType::LeadArtist => "LeadArtist",
                    PictureType::Artist => "Artist",
                    PictureType::Conductor => "Conductor",
                    PictureType::Band => "Band",
                    PictureType::Composer => "Composer",
                    PictureType::Lyricist => "Lyricist",
                    PictureType::RecordingLocation => "RecordingLocation",
                    PictureType::DuringRecording => "DuringRecording",
                    PictureType::DuringPerformance => "DuringPerformance",
                    PictureType::ScreenCapture => "ScreenCapture",
                    PictureType::BrightFish => "BrightFish",
                    PictureType::Illustration => "Illustration",
                    PictureType::BandLogo => "BandLogo",
                    PictureType::PublisherLogo => "PublisherLogo",
                    PictureType::Undefined(_) => "Undefined",
                    _ => "Other",
                };

                artworks.push(Artwork {
                    picture_type: picture_type.to_string(),
                    mime_type: picture
                        .mime_type()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| "image/jpeg".to_string()),
                    data: picture.data().to_vec(),
                });
            }
        }

        Ok(artworks)
    }

    pub fn is_supported_file(path: &Utf8PathBuf) -> bool {
        if let Some(ext) = path.extension() {
            matches!(
                ext.to_lowercase().as_str(),
                "mp3"
                    | "flac"
                    | "ogg"
                    | "opus"
                    | "m4a"
                    | "aac"
                    | "wav"
                    | "wma"
                    | "ape"
                    | "wv"
                    | "dsf"
                    | "dff"
            )
        } else {
            false
        }
    }
    /// Read raw key-value pairs directly from the audio file.
    ///
    /// Unlike `extract_from_file`, this returns the raw format-specific tag fields
    /// as they appear in the file, not normalized to rmpd's internal tag names.
    /// Used by the `readcomments` MPD command.
    pub fn read_raw_comments(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        let ext = path
            .extension()
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        match ext.as_str() {
            "flac" => MetadataExtractor::read_vorbis_comments_from_flac(path),
            "ogg" => MetadataExtractor::read_vorbis_comments_from_ogg(path),
            "opus" => MetadataExtractor::read_vorbis_comments_from_opus(path),
            "mp3" => MetadataExtractor::read_comments_from_id3v2(path),
            "m4a" | "aac" => MetadataExtractor::read_comments_from_mp4(path),
            _ => MetadataExtractor::read_comments_generic(path),
        }
    }

    fn read_vorbis_comments_from_flac(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        let file = std::fs::File::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?;
        let mut reader = BufReader::new(file);
        let flac = FlacFile::read_from(&mut reader, ParseOptions::default())
            .map_err(|e| RmpdError::Library(format!("Failed to read FLAC: {e}")))?;
        let mut pairs = Vec::new();
        if let Some(vorbis) = flac.vorbis_comments() {
            for (key, value) in vorbis.items() {
                pairs.push((key.to_string(), value.to_string()));
            }
        }
        Ok(pairs)
    }

    fn read_vorbis_comments_from_ogg(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        let file = std::fs::File::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?;
        let mut reader = BufReader::new(file);
        let ogg = VorbisFile::read_from(&mut reader, ParseOptions::default())
            .map_err(|e| RmpdError::Library(format!("Failed to read OGG: {e}")))?;
        let mut pairs = Vec::new();
        for (key, value) in ogg.vorbis_comments().items() {
            pairs.push((key.to_string(), value.to_string()));
        }
        Ok(pairs)
    }

    fn read_vorbis_comments_from_opus(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        let file = std::fs::File::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?;
        let mut reader = BufReader::new(file);
        let opus = OpusFile::read_from(&mut reader, ParseOptions::default())
            .map_err(|e| RmpdError::Library(format!("Failed to read Opus: {e}")))?;
        let mut pairs = Vec::new();
        for (key, value) in opus.vorbis_comments().items() {
            pairs.push((key.to_string(), value.to_string()));
        }
        Ok(pairs)
    }

    fn read_comments_from_id3v2(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        use lofty::id3::v2::Frame;
        let file = std::fs::File::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?;
        let mut reader = BufReader::new(file);
        let mpeg = MpegFile::read_from(&mut reader, ParseOptions::default())
            .map_err(|e| RmpdError::Library(format!("Failed to read MP3: {e}")))?;
        let mut pairs = Vec::new();
        if let Some(id3v2) = mpeg.id3v2() {
            // MPD only calls OnPair for TXXX (user-defined text) frames,
            // NOT standard text frames (TIT2, TPE1, etc.). Match that behavior.
            for frame in id3v2 {
                if let Frame::UserText(f) = &frame {
                    let key = f.description.as_ref().to_string();
                    let value = f.content.as_ref().to_string();
                    if !key.is_empty() {
                        pairs.push((key, value));
                    }
                }
            }
        }
        Ok(pairs)
    }

    /// Map a 4-byte MP4 fourcc atom identifier to a human-readable key name.
    fn fourcc_to_key(fourcc: &[u8; 4]) -> Option<&'static str> {
        match fourcc {
            b"\xa9nam" => Some("title"),
            b"\xa9ART" => Some("artist"),
            b"\xa9alb" => Some("album"),
            b"aART" => Some("album_artist"),
            b"\xa9day" => Some("date"),
            b"trkn" => Some("track"),
            b"disk" => Some("disc"),
            b"\xa9gen" => Some("genre"),
            b"gnre" => Some("genre"),
            b"\xa9wrt" => Some("composer"),
            b"\xa9cmt" => Some("comment"),
            b"cpil" => Some("compilation"),
            b"\xa9grp" => Some("grouping"),
            b"\xa9lyr" => Some("lyrics"),
            b"\xa9too" => Some("encoder"),
            b"soal" => Some("sort_album"),
            b"soar" => Some("sort_artist"),
            b"soaa" => Some("sort_album_artist"),
            b"sonm" => Some("sort_title"),
            b"soco" => Some("sort_composer"),
            b"tmpo" => Some("bpm"),
            b"rtng" => Some("rating"),
            b"desc" => Some("description"),
            _ => None,
        }
    }

    fn read_comments_from_mp4(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        let file = std::fs::File::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?;
        let mut reader = BufReader::new(file);
        let mp4 = Mp4File::read_from(&mut reader, ParseOptions::default())
            .map_err(|e| RmpdError::Library(format!("Failed to read MP4: {e}")))?;
        let mut pairs = Vec::new();
        if let Some(ilst) = mp4.ilst() {
            for atom in ilst {
                let key = match atom.ident() {
                    AtomIdent::Fourcc(fourcc) => {
                        MetadataExtractor::fourcc_to_key(fourcc).map(|s| s.to_string())
                    }
                    AtomIdent::Freeform { name, .. } => Some(name.as_ref().to_string()),
                };
                if let Some(key) = key {
                    for data in atom.data() {
                        let value = match data {
                            AtomData::UTF8(s) | AtomData::UTF16(s) => s.clone(),
                            AtomData::Bool(b) => if *b { "1" } else { "0" }.to_string(),
                            _ => continue,
                        };
                        pairs.push((key.clone(), value));
                    }
                }
            }
        }
        Ok(pairs)
    }

    fn read_comments_generic(path: &Utf8PathBuf) -> Result<Vec<(String, String)>> {
        let tagged_file = Probe::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?
            .read()
            .map_err(|e| RmpdError::Library(format!("Failed to read file: {e}")))?;
        let mut pairs = Vec::new();
        if let Some(tag) = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())
        {
            let tag_type = tag.tag_type();
            for item in tag.items() {
                if let Some(key) = item.key().map_key(tag_type)
                    && let Some(value) = item.value().text()
                {
                    pairs.push((key.to_string(), value.to_string()));
                }
            }
        }
        Ok(pairs)
    }
}
