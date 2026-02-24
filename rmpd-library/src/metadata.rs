use camino::Utf8PathBuf;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::Song;
use std::fs;
use std::time::SystemTime;

use crate::database::system_time_to_unix_secs;

/// Detect bogus DSF ReplayGain hex data stored in the Comment field.
/// DSF files sometimes embed raw ReplayGain binary data in the COMM frame
/// which lofty decodes as a hex string. MPD suppresses these; we do the same.
fn is_bogus_dsf_comment(s: &str) -> bool {
    // Bogus comments are long hex strings (typically 32+ chars of 0-9a-fA-F),
    // possibly with spaces between groups (e.g. " 00000219 00000219 ...")
    let trimmed = s.trim();
    trimmed.len() >= 16 && trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == ' ')
}

/// Represents extracted artwork from an audio file
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
        // Get file metadata (mtime)
        let metadata = fs::metadata(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to read file metadata: {e}")))?;

        let mtime = system_time_to_unix_secs(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));

        // Parse audio file with lofty (now supports DSF/DFF with ID3v2 tags)
        let tagged_file = Probe::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?
            .read()
            .map_err(|e| RmpdError::Library(format!("Failed to read file: {e}")))?;

        // Extract audio properties
        let properties = tagged_file.properties();
        let duration = Some(properties.duration());
        let sample_rate = properties.sample_rate();
        let channels = properties.channels();
        let bitrate = properties.audio_bitrate();

        // Extract tags
        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag());

        tracing::debug!("extracting metadata from: {}", path);

        let (
            title,
            artist,
            album,
            album_artist,
            track,
            disc,
            date,
            genre,
            composer,
            performer,
            comment,
            grouping,
            musicbrainz_trackid,
            musicbrainz_albumid,
            musicbrainz_artistid,
            musicbrainz_albumartistid,
            musicbrainz_releasegroupid,
            musicbrainz_releasetrackid,
            musicbrainz_workid,
            artist_sort,
            album_artist_sort,
            original_date,
            label,
        ) = if let Some(tag) = tag {
            (
                tag.title().map(|s| s.to_string()),
                tag.artist().map(|s| s.to_string()),
                tag.album().map(|s| s.to_string()),
                tag.get_string(ItemKey::AlbumArtist).map(|s| s.to_string()),
                tag.track(),
                tag.disk(),
                tag.get_string(ItemKey::RecordingDate)
                    .or_else(|| tag.get_string(ItemKey::Year))
                    .map(|s| s.to_string()),
                tag.genre().map(|s| s.to_string()),
                tag.get_string(ItemKey::Composer).map(|s| s.to_string()),
                tag.get_string(ItemKey::Performer).map(|s| s.to_string()),
                tag.comment()
                    .filter(|s| !is_bogus_dsf_comment(s))
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::ContentGroup).map(|s| s.to_string()),
                // MusicBrainz IDs
                // Note: lofty's MusicBrainzRecordingId = Vorbis MUSICBRAINZ_TRACKID = MPD's MUSICBRAINZ_TRACKID
                // and lofty's MusicBrainzTrackId = Vorbis MUSICBRAINZ_RELEASETRACKID = MPD's MUSICBRAINZ_RELEASETRACKID
                tag.get_string(ItemKey::MusicBrainzRecordingId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzReleaseId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzArtistId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzReleaseArtistId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzReleaseGroupId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzTrackId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzWorkId)
                    .map(|s| s.to_string()),
                // Extended metadata
                tag.get_string(ItemKey::TrackArtistSortOrder)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::AlbumArtistSortOrder)
                    .map(|s| s.to_string()),
                {
                    // Pick the longest (most precise) OriginalReleaseDate value,
                    // since lofty maps both ORIGINALDATE and ORIGINALYEAR to the same key.
                    let mut best: Option<String> = None;
                    for item in tag.items() {
                        if item.key() == ItemKey::OriginalReleaseDate
                            && let Some(val) = item.value().text()
                            && best.as_ref().is_none_or(|b| val.len() > b.len())
                        {
                            best = Some(val.to_string());
                        }
                    }
                    best
                },
                tag.get_string(ItemKey::Label)
                    .or_else(|| tag.get_string(ItemKey::Publisher))
                    .map(|s| s.to_string()),
            )
        } else {
            (
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None, None, None,
            )
        };

        // ReplayGain (not all formats support this)
        let (
            replay_gain_track_gain,
            replay_gain_track_peak,
            replay_gain_album_gain,
            replay_gain_album_peak,
        ) = if let Some(tag) = tag {
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
            id: 0, // Will be set by database
            path: path.clone(),
            duration,
            title,
            artist,
            album,
            album_artist,
            track,
            disc,
            date,
            genre,
            composer,
            performer,
            comment,
            grouping,
            musicbrainz_trackid,
            musicbrainz_albumid,
            musicbrainz_artistid,
            musicbrainz_albumartistid,
            musicbrainz_releasegroupid,
            musicbrainz_releasetrackid,
            musicbrainz_workid,
            artist_sort,
            album_artist_sort,
            original_date,
            label,
            sample_rate,
            channels,
            // For lossy codecs decoded as float (M4A/AAC), use 0 as sentinel for "f" format.
            // MPD's FAAD decoder always outputs SampleFormat::FLOAT for AAC.
            bits_per_sample: {
                let ext = path.extension().map(|e| e.to_lowercase());
                match ext.as_deref() {
                    Some("m4a" | "aac") => Some(0), // float
                    _ => Some(properties.bit_depth().unwrap_or(16)),
                }
            },
            bitrate,
            replay_gain_track_gain,
            replay_gain_track_peak,
            replay_gain_album_gain,
            replay_gain_album_peak,
            added_at: mtime,
            last_modified: mtime,
        })
    }

    /// Extract artwork/album art from an audio file
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
