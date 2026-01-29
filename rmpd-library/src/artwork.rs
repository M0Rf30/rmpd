use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use rmpd_core::error::{Result, RmpdError};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::database::Database;

const MAX_ARTWORK_SIZE: usize = 5 * 1024 * 1024; // 5MB

pub struct AlbumArtExtractor {
    db: Database,
}

impl AlbumArtExtractor {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Extract album art from a file and cache it
    /// `cache_key`: relative path for cache lookup (e.g., "01.m4a")
    /// `file_path`: absolute path for file reading (e.g., "/home/user/Music/01.m4a")
    pub fn extract_and_cache(&self, cache_key: &str, file_path: &str) -> Result<Option<Vec<u8>>> {
        // Check cache first using relative path as key
        if let Some(data) = self.db.get_artwork(cache_key, "front")? {
            return Ok(Some(data));
        }

        // Not in cache, extract from file using absolute path
        let abs_path = Path::new(file_path);
        let tagged_file = lofty::read_from_path(abs_path)
            .map_err(|e| RmpdError::Library(format!("Failed to read file: {}", e)))?;

        // Try to find front cover
        let picture = if let Some(primary_tag) = tagged_file.primary_tag() {
            primary_tag
                .pictures()
                .iter()
                .find(|p| matches!(p.pic_type(), PictureType::CoverFront | PictureType::Other))
                .or_else(|| primary_tag.pictures().first())
        } else {
            None
        };

        if let Some(pic) = picture {
            let data = pic.data();

            // Check size limit
            if data.len() > MAX_ARTWORK_SIZE {
                return Err(RmpdError::Library(format!(
                    "Artwork too large: {} bytes (max {})",
                    data.len(),
                    MAX_ARTWORK_SIZE
                )));
            }

            // Calculate hash for deduplication
            let mut hasher = Sha256::new();
            hasher.update(data);
            let hash = format!("{:x}", hasher.finalize());

            // Get MIME type
            let mime_type = pic.mime_type().map(|m| m.to_string()).unwrap_or_else(|| {
                // Try to infer from data
                if data.starts_with(b"\xFF\xD8\xFF") {
                    "image/jpeg".to_string()
                } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
                    "image/png".to_string()
                } else {
                    "application/octet-stream".to_string()
                }
            });

            // Store in cache using relative path as key
            self.db.store_artwork(
                cache_key,
                "front",
                &mime_type,
                data,
                &hash,
            )?;

            Ok(Some(data.to_vec()))
        } else {
            Ok(None)
        }
    }

    /// Get album art from cache or extract if not cached
    /// `cache_key`: relative path for cache lookup (e.g., "01.m4a")
    /// `file_path`: absolute path for file reading (e.g., "/home/user/Music/01.m4a")
    pub fn get_artwork(&self, cache_key: &str, file_path: &str, offset: usize) -> Result<Option<ArtworkData>> {
        let data = match self.extract_and_cache(cache_key, file_path)? {
            Some(data) => data,
            None => return Ok(None),
        };

        // Infer MIME type from data
        let mime_type = if data.starts_with(b"\xFF\xD8\xFF") {
            "image/jpeg"
        } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else {
            "application/octet-stream"
        };

        // Handle offset for chunked transfer
        // MPD protocol uses 8KB (8192 byte) chunks
        const CHUNK_SIZE: usize = 8192;

        let chunk = if offset >= data.len() {
            // Return empty chunk when offset is past the end
            // This is needed for proper MPD protocol compliance
            Vec::new()
        } else {
            let end = (offset + CHUNK_SIZE).min(data.len());
            data[offset..end].to_vec()
        };

        Ok(Some(ArtworkData {
            mime_type: mime_type.to_string(),
            total_size: data.len(),
            data: chunk,
        }))
    }

    /// Extract all picture types from a file
    pub fn extract_all_pictures(&self, path: &str) -> Result<Vec<ExtractedPicture>> {
        let file_path = Path::new(path);
        let tagged_file = lofty::read_from_path(file_path)
            .map_err(|e| RmpdError::Library(format!("Failed to read file: {}", e)))?;

        let mut pictures = Vec::new();

        if let Some(primary_tag) = tagged_file.primary_tag() {
            for pic in primary_tag.pictures() {
                let pic_type = picture_type_to_string(pic.pic_type());
                let mime_type = pic.mime_type()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "application/octet-stream".to_string());

                pictures.push(ExtractedPicture {
                    picture_type: pic_type,
                    mime_type,
                    data: pic.data().to_vec(),
                });
            }
        }

        Ok(pictures)
    }
}

pub struct ArtworkData {
    pub mime_type: String,
    pub total_size: usize,
    pub data: Vec<u8>,
}

pub struct ExtractedPicture {
    pub picture_type: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

fn picture_type_to_string(pic_type: PictureType) -> String {
    match pic_type {
        PictureType::CoverFront => "front",
        PictureType::CoverBack => "back",
        PictureType::Icon => "icon",
        PictureType::OtherIcon => "other_icon",
        PictureType::Leaflet => "leaflet",
        PictureType::Media => "media",
        PictureType::LeadArtist => "artist",
        PictureType::Artist => "artist",
        PictureType::Conductor => "conductor",
        PictureType::Band => "band",
        PictureType::Composer => "composer",
        PictureType::Lyricist => "lyricist",
        PictureType::RecordingLocation => "recording_location",
        PictureType::DuringRecording => "during_recording",
        PictureType::DuringPerformance => "during_performance",
        PictureType::ScreenCapture => "screen_capture",
        PictureType::BrightFish => "bright_fish",
        PictureType::Illustration => "illustration",
        PictureType::BandLogo => "band_logo",
        PictureType::PublisherLogo => "publisher_logo",
        _ => "other",
    }
    .to_string()
}
