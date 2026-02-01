use camino::Utf8PathBuf;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::Song;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

        let mtime = metadata
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| {
                tracing::warn!("System time before UNIX_EPOCH, using 0");
                Duration::ZERO
            })
            .as_secs() as i64;

        // Parse audio file with lofty (now supports DSF/DFF with ID3v2 tags)
        let tagged_file = Probe::open(path.as_str())
            .map_err(|e| RmpdError::Library(format!("Failed to open file: {e}")))?
            .read()
            .map_err(|e| RmpdError::Library(format!("Failed to read file: {e}")))?;

        // Extract audio properties
        let properties = tagged_file.properties();
        let duration = Some(Duration::from_secs(properties.duration().as_secs()));
        let sample_rate = properties.sample_rate();
        let channels = properties.channels();
        let bitrate = properties.audio_bitrate();

        // Extract tags
        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag());

        tracing::debug!("Extracting metadata from: {}", path);
        tracing::debug!("Tag present: {}", tag.is_some());

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
            musicbrainz_trackid,
            musicbrainz_albumid,
            musicbrainz_artistid,
            musicbrainz_albumartistid,
            musicbrainz_releasegroupid,
            musicbrainz_releasetrackid,
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
                tag.date().map(|d| d.to_string()),
                tag.genre().map(|s| s.to_string()),
                tag.get_string(ItemKey::Composer).map(|s| s.to_string()),
                tag.get_string(ItemKey::Performer).map(|s| s.to_string()),
                tag.comment().map(|s| s.to_string()),
                // MusicBrainz IDs
                tag.get_string(ItemKey::MusicBrainzTrackId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzReleaseId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzArtistId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzReleaseArtistId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzReleaseGroupId)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::MusicBrainzRecordingId)
                    .map(|s| s.to_string()),
                // Extended metadata
                tag.get_string(ItemKey::TrackArtistSortOrder)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::AlbumArtistSortOrder)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::OriginalReleaseDate)
                    .map(|s| s.to_string()),
                tag.get_string(ItemKey::Label).map(|s| s.to_string()),
            )
        } else {
            (
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None, None, None, None, None, None,
            )
        };

        tracing::debug!("Extracted MB TrackID: {:?}", musicbrainz_trackid);
        tracing::debug!("Extracted MB AlbumID: {:?}", musicbrainz_albumid);
        tracing::debug!("Extracted Label: {:?}", label);

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
            musicbrainz_trackid,
            musicbrainz_albumid,
            musicbrainz_artistid,
            musicbrainz_albumartistid,
            musicbrainz_releasegroupid,
            musicbrainz_releasetrackid,
            artist_sort,
            album_artist_sort,
            original_date,
            label,
            sample_rate,
            channels,
            bits_per_sample: Some(properties.bit_depth().unwrap_or(16)),
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
                    mime_type: picture.mime_type().map(|m| m.to_string()).unwrap_or_else(|| "image/jpeg".to_string()),
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
