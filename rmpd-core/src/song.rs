use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Song {
    pub id: u64,
    pub path: Utf8PathBuf,
    pub duration: Option<Duration>,

    // Core metadata
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub performer: Option<String>,
    pub comment: Option<String>,

    // MusicBrainz IDs
    pub musicbrainz_trackid: Option<String>,
    pub musicbrainz_albumid: Option<String>,
    pub musicbrainz_artistid: Option<String>,
    pub musicbrainz_albumartistid: Option<String>,
    pub musicbrainz_releasegroupid: Option<String>,
    pub musicbrainz_releasetrackid: Option<String>,

    // Extended metadata
    pub artist_sort: Option<String>,
    pub album_artist_sort: Option<String>,
    pub original_date: Option<String>,
    pub label: Option<String>,

    // Audio properties
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bits_per_sample: Option<u8>,
    pub bitrate: Option<u32>,

    // ReplayGain
    pub replay_gain_track_gain: Option<f32>,
    pub replay_gain_track_peak: Option<f32>,
    pub replay_gain_album_gain: Option<f32>,
    pub replay_gain_album_peak: Option<f32>,

    // Timestamps
    pub added_at: i64,
    pub last_modified: i64,
}

impl Song {
    pub fn display_title(&self) -> &str {
        self.title
            .as_deref()
            .unwrap_or_else(|| self.path.file_name().unwrap_or("Unknown"))
    }

    pub fn display_artist(&self) -> &str {
        self.artist
            .as_deref()
            .or(self.album_artist.as_deref())
            .unwrap_or("Unknown Artist")
    }

    pub fn display_album(&self) -> &str {
        self.album.as_deref().unwrap_or("Unknown Album")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
}

impl AudioFormat {
    pub fn new(sample_rate: u32, channels: u8, bits_per_sample: u8) -> Self {
        Self {
            sample_rate,
            channels,
            bits_per_sample,
        }
    }
}
