// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Artwork handed to the desktop media surfaces.
//!
//! The macOS Now Playing panel takes a URL rather than bytes, so the picture has
//! to exist as a file somewhere. Everything here is platform-neutral and unit
//! tested; the macOS integration only turns the returned path into a `file://`
//! URL.

use std::path::{Path, PathBuf};
use tracing::debug;

/// File extension for an image MIME type.
fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/jxl" => "jxl",
        "image/gif" => "gif",
        _ => "jpg",
    }
}

/// Largest cover we ask the library for. It skips files above 5 MiB anyway;
/// passing a ceiling avoids handing `offset + chunk_size` an extreme value.
const MAX_ARTWORK_BYTES: usize = 16 * 1024 * 1024;

/// How many picture files to keep around. One file per distinct picture, so a
/// long-running daemon with a big library would otherwise grow this without end.
const CACHE_KEEP: usize = 64;

/// Directory used to publish artwork for the OS media surfaces.
///
/// `$XDG_CACHE_HOME` when set, then the platform convention: `~/Library/Caches`
/// on macOS, `~/.cache` elsewhere, and the temp dir as a last resort.
pub fn cache_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("rmpd").join("nowplaying");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    if let Some(home) = &home {
        return home.join("Library/Caches/rmpd/nowplaying");
    }
    home.map(|h| h.join(".cache/rmpd/nowplaying"))
        .unwrap_or_else(std::env::temp_dir)
}

/// Write a picture into `dir` under a name derived from its content, and return
/// the path.
///
/// Reusing one file per distinct picture keeps the media panel's cache warm and
/// avoids rewriting the same bytes at every track change.
pub fn cache_artwork(dir: &Path, bytes: &[u8], mime: &str) -> std::io::Result<PathBuf> {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let name = format!("art-{:016x}.{}", hasher.finish(), extension_for(mime));
    let path = dir.join(name);
    if path.exists() {
        return Ok(path);
    }

    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    prune_cache(dir, CACHE_KEEP);
    Ok(path)
}

/// Drop the oldest files until `keep` remain. Best effort: a cache that cannot
/// be pruned is not worth failing over.
fn prune_cache(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime)); // newest first
    for (_, path) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

/// Last published picture: the song it belongs to, and the URL handed out.
///
/// Artwork lookup reads and probes the file, and the media panel asks for a
/// refresh on every position tick, so the answer is memoised per song.
static PUBLISHED: std::sync::Mutex<Option<(String, Option<String>)>> = std::sync::Mutex::new(None);

/// `file://` URL of the artwork for `song_path`, ready for the OS media surface.
///
/// Returns `None` when the song has no embedded picture and no cover file next
/// to it. Resolved paths are relative to the music directory.
pub fn artwork_url_for_song(music_dir: Option<&str>, song_path: &str) -> Option<String> {
    if let Ok(cache) = PUBLISHED.lock()
        && let Some((path, url)) = cache.as_ref()
        && path == song_path
    {
        return url.clone();
    }

    let url = resolve_artwork_url(music_dir, song_path);
    if let Ok(mut cache) = PUBLISHED.lock() {
        *cache = Some((song_path.to_string(), url.clone()));
    }
    url
}

fn resolve_artwork_url(music_dir: Option<&str>, song_path: &str) -> Option<String> {
    let absolute = rmpd_core::path::resolve_path(song_path, music_dir);
    let (bytes, mime) = artwork_for_song(Path::new(&absolute))?;
    let file = match cache_artwork(&cache_dir(), &bytes, &mime) {
        Ok(file) => file,
        Err(error) => {
            debug!("now playing: could not cache artwork: {error}");
            return None;
        }
    };
    Some(format!("file://{}", file.display()))
}

/// Artwork for one song: the embedded picture when there is one, otherwise a
/// cover file sitting next to it.
///
/// Mirrors what `readpicture` and `albumart` serve, so the media panel shows the
/// same picture a client would.
pub fn artwork_for_song(absolute_path: &Path) -> Option<(Vec<u8>, String)> {
    if let Ok(path) = camino::Utf8PathBuf::from_path_buf(absolute_path.to_path_buf())
        && let Ok(visuals) = rmpd_library::MetadataExtractor::extract_artwork_from_file(&path)
    {
        // Same preference the library uses for `readpicture` (front cover, then
        // "other"), and among those the largest: files often carry a small
        // thumbnail next to the real cover.
        let mut candidates: Vec<_> = visuals
            .iter()
            .filter(|art| art.picture_type == "front" || art.picture_type == "other")
            .collect();
        if candidates.is_empty() {
            candidates = visuals.iter().collect();
        }
        candidates.sort_by_key(|art| std::cmp::Reverse(art.data.len()));
        if let Some(art) = candidates.first() {
            return Some((art.data.clone(), art.mime_type.clone()));
        }
    }

    let dir = absolute_path.parent()?;
    match rmpd_library::find_external_cover(dir, 0, MAX_ARTWORK_BYTES) {
        rmpd_library::ArtLookup::Found(cover) => {
            let mime = sniff_mime(&cover.data, cover.filename);
            Some((cover.data, mime.to_string()))
        }
        _ => None,
    }
}

/// MIME type for a cover file. The bytes decide when they can, since a file
/// named `.jpg` sometimes holds a PNG; the name is the fallback.
fn sniff_mime(data: &[u8], filename: &str) -> &'static str {
    if data.starts_with(b"\xFF\xD8\xFF") {
        "image/jpeg"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        match filename.rsplit('.').next() {
            Some("png") => "image/png",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            _ => "image/jpeg",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        format!(
            "{}/../rmpd-library/tests/fixtures/samples/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    /// A fresh directory per call: tests run in parallel and several of them
    /// write a `cover.jpg`.
    fn tempdir() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rmpd-art-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cache_naming_depends_on_content_and_mime() {
        let dir = tempdir().join("cache");
        let png = cache_artwork(&dir, b"one picture", "image/png").unwrap();
        let same = cache_artwork(&dir, b"one picture", "image/png").unwrap();
        let other = cache_artwork(&dir, b"another picture", "image/jpeg").unwrap();

        assert_eq!(png, same, "identical bytes should reuse the same file");
        assert_ne!(png, other, "different bytes should get their own file");
        assert_eq!(png.extension().unwrap(), "png");
        assert_eq!(other.extension().unwrap(), "jpg");
        assert_eq!(std::fs::read(&png).unwrap(), b"one picture");
    }

    #[test]
    fn embedded_picture_wins_over_a_cover_file() {
        // A FLAC with an embedded PNG, built by appending a PICTURE block to a
        // fixture (same trick as the artwork command tests).
        let dir = tempdir();
        let flac = build_flac_with_picture(&dir);
        std::fs::write(dir.join("cover.jpg"), b"not the embedded picture").unwrap();

        let (bytes, mime) = artwork_for_song(&flac).expect("embedded picture");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, PICTURE_PAYLOAD);
    }

    #[test]
    fn cover_file_is_used_when_nothing_is_embedded() {
        let dir = tempdir();
        let flac = dir.join("plain.flac");
        std::fs::copy(fixture("basic.flac"), &flac).unwrap();
        std::fs::write(dir.join("cover.jpg"), b"standalone cover").unwrap();

        let (bytes, mime) = artwork_for_song(&flac).expect("external cover");
        assert_eq!(bytes, b"standalone cover");
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn no_artwork_anywhere_is_none() {
        let dir = tempdir().join("empty");
        std::fs::create_dir_all(&dir).unwrap();
        let flac = dir.join("plain.flac");
        std::fs::copy(fixture("basic.flac"), &flac).unwrap();
        assert!(artwork_for_song(&flac).is_none());
    }

    const PICTURE_PAYLOAD: &[u8] = b"\x89PNG\r\n\x1a\nembedded";

    /// Append PICTURE metadata blocks, so a fixture can carry several images.
    fn build_flac_with_pictures(dir: &Path, pictures: &[(u32, &[u8])]) -> PathBuf {
        let raw = std::fs::read(fixture("basic.flac")).unwrap();
        assert_eq!(&raw[..4], b"fLaC");

        let mut pos = 4;
        let mut blocks: Vec<(u8, Vec<u8>)> = Vec::new();
        loop {
            let header = raw[pos];
            let last = header & 0x80 != 0;
            let length = u32::from_be_bytes([0, raw[pos + 1], raw[pos + 2], raw[pos + 3]]) as usize;
            blocks.push((header & 0x7F, raw[pos + 4..pos + 4 + length].to_vec()));
            pos += 4 + length;
            if last {
                break;
            }
        }
        let audio = &raw[pos..];

        for (kind, data) in pictures {
            let mut payload = Vec::new();
            payload.extend(kind.to_be_bytes()); // FLAC picture type: 3 front, 1 icon
            payload.extend((b"image/png".len() as u32).to_be_bytes());
            payload.extend(b"image/png");
            payload.extend(0u32.to_be_bytes());
            payload.extend(64u32.to_be_bytes());
            payload.extend(64u32.to_be_bytes());
            payload.extend(24u32.to_be_bytes());
            payload.extend(0u32.to_be_bytes());
            payload.extend((data.len() as u32).to_be_bytes());
            payload.extend(*data);
            blocks.push((6, payload)); // PICTURE
        }

        let mut out = Vec::from(&b"fLaC"[..]);
        let total = blocks.len();
        for (index, (kind, data)) in blocks.iter().enumerate() {
            out.push(kind | if index + 1 == total { 0x80 } else { 0 });
            out.extend((data.len() as u32).to_be_bytes()[1..].to_vec());
            out.extend(data);
        }
        out.extend(audio);

        let path = dir.join("with-art.flac");
        std::fs::write(&path, &out).unwrap();
        path
    }

    fn build_flac_with_picture(dir: &Path) -> PathBuf {
        build_flac_with_pictures(dir, &[(3, PICTURE_PAYLOAD)])
    }

    #[test]
    fn the_front_cover_wins_over_a_thumbnail() {
        let dir = tempdir();
        let thumbnail = b"tiny".to_vec();
        let cover = vec![b'C'; 4096];
        // a 32x32 icon first, the front cover (type 3) second
        let flac =
            build_flac_with_pictures(&dir, &[(1, thumbnail.as_slice()), (3, cover.as_slice())]);

        let (bytes, mime) = artwork_for_song(&flac).expect("front cover");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes.len(), 4096, "the front cover should win");
    }

    #[test]
    fn cover_mime_follows_the_bytes_not_the_name() {
        let dir = tempdir();
        let flac = dir.join("plain.flac");
        std::fs::copy(fixture("basic.flac"), &flac).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&vec![0u8; 300]);
        std::fs::write(dir.join("cover.jpg"), &png).unwrap();

        let (_, mime) = artwork_for_song(&flac).expect("cover file");
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn cache_prunes_old_entries() {
        let dir = tempdir().join("prune");
        for i in 0..(CACHE_KEEP + 8) {
            cache_artwork(&dir, &[i as u8; 64], "image/jpeg").unwrap();
        }
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert!(files <= CACHE_KEEP, "cache should be pruned, found {files}");
    }

    #[test]
    fn song_with_embedded_art_yields_a_file_url() {
        let dir = tempdir();
        build_flac_with_picture(&dir);
        let url = artwork_url_for_song(Some(dir.to_str().unwrap()), "with-art.flac")
            .expect("embedded picture should produce a url");

        assert!(url.starts_with("file:///"), "unexpected url: {url}");
        let path = url.trim_start_matches("file://");
        assert_eq!(std::fs::read(path).unwrap(), PICTURE_PAYLOAD);
        assert_eq!(
            artwork_url_for_song(Some(dir.to_str().unwrap()), "with-art.flac"),
            Some(url)
        );
    }

    #[test]
    fn song_without_art_yields_nothing() {
        let dir = tempdir();
        let flac = dir.join("plain.flac");
        std::fs::copy(fixture("basic.flac"), &flac).unwrap();
        assert_eq!(
            artwork_url_for_song(Some(dir.to_str().unwrap()), "plain.flac"),
            None
        );
    }
}
