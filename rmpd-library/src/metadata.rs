use camino::Utf8PathBuf;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::Song;
use std::fs;
use std::time::SystemTime;

use crate::database::system_time_to_unix_secs;

fn is_bogus_dsf_comment(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.len() >= 16 && trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == ' ')
}

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

        if let Some(tag) = tag {
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
                        tags.push((tag_name.to_string(), val.clone()));
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
            {
                tags.push(("track".to_string(), track.to_string()));
            }
            if !tags.iter().any(|(k, _)| k == "disc")
                && let Some(disc) = tag.disk()
            {
                tags.push(("disc".to_string(), disc.to_string()));
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
}
