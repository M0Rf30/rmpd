/// Shared path utilities: tilde expansion and path resolution.
use camino::Utf8PathBuf;

/// Expand `~/...` to the user's home directory.
pub fn expand_tilde(path: &Utf8PathBuf) -> Utf8PathBuf {
    let path_str = path.as_str();
    if path_str.starts_with("~/")
        && let Some(home) = dirs::home_dir()
        && let Some(home_str) = home.to_str()
    {
        return Utf8PathBuf::from(path_str.replacen('~', home_str, 1));
    }
    path.clone()
}

/// Resolve a relative path to an absolute path using the music directory.
/// If the path is already absolute, returns it as-is.
pub fn resolve_path(rel_path: &str, music_dir: Option<&str>) -> String {
    // Remote stream URIs (http://, https://, etc.) are absolute already and
    // must never be joined onto the music directory.
    if rel_path.starts_with('/') || is_uri(rel_path) {
        return rel_path.to_string();
    }

    if let Some(music_dir) = music_dir {
        let music_dir = music_dir.trim_end_matches('/');
        format!("{music_dir}/{rel_path}")
    } else {
        rel_path.to_string()
    }
}

/// Whether `s` begins with a URI scheme (`scheme://`), e.g. `http://host/x`.
/// Used to distinguish remote stream URIs from local relative paths.
#[must_use]
pub fn is_uri(s: &str) -> bool {
    match s.find("://") {
        Some(i) if i > 0 => {
            let scheme = &s[..i];
            scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_uri_detects_schemes() {
        assert!(is_uri("http://host/stream"));
        assert!(is_uri("https://host/stream.mp3?x=1"));
        assert!(is_uri("hls+https://host/x"));
        assert!(!is_uri("/abs/path"));
        assert!(!is_uri("rel/path.mp3"));
        assert!(!is_uri("://nohost"));
        assert!(!is_uri("C:/weird"));
    }

    #[test]
    fn resolve_path_passes_uris_through() {
        // Remote URIs must never be joined onto the music directory.
        assert_eq!(
            resolve_path("http://radio.example/stream", Some("/music")),
            "http://radio.example/stream"
        );
        // Absolute local paths pass through; relative paths join music_dir.
        assert_eq!(
            resolve_path("/abs/song.flac", Some("/music")),
            "/abs/song.flac"
        );
        assert_eq!(resolve_path("a/b.flac", Some("/music")), "/music/a/b.flac");
    }
}
