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

/// Mirrors MPD's `uri_safe_local()`: a non-empty `/`-separated path with no
/// `.` or `..` segments (used to validate client-supplied relative paths
/// before joining them onto the music directory, e.g. the legacy `base`
/// filter pair and `update`/`rescan`'s path argument).
#[must_use]
pub fn uri_safe_local(uri: &str) -> bool {
    !uri.is_empty()
        && uri
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Orders two song paths the way MPD's database tree walk does
/// (`Directory::Walk` in `db/plugins/simple/Directory.cxx`): within each
/// directory, songs are visited before subdirectories, and both songs and
/// subdirectories are name-sorted. This is deliberately NOT the same as
/// sorting the full path strings — under a plain string sort, `"rock/..."`
/// can sort before `"song1..."` (because `/` compares low), which puts a
/// subdirectory's files ahead of root-level files. Comparing segment-wise
/// and letting "no more segments left" (a file) win over "one more segment"
/// (a subdirectory) at the point of divergence reproduces the tree order.
///
/// This is the *default* order (no `sort TAG` given) for `find`/`search`/
/// `count`/`searchcount`/`findadd`/`searchadd`/`searchaddpl`/`list`.
#[must_use]
pub fn compare_db_path(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_segs = a.split('/');
    let mut b_segs = b.split('/');
    loop {
        match (a_segs.next(), b_segs.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(sa), Some(sb)) => {
                let a_is_last = a_segs.clone().next().is_none();
                let b_is_last = b_segs.clone().next().is_none();
                if a_is_last && !b_is_last {
                    return std::cmp::Ordering::Less;
                }
                if b_is_last && !a_is_last {
                    return std::cmp::Ordering::Greater;
                }
                match sa.cmp(sb) {
                    std::cmp::Ordering::Equal => continue,
                    other => return other,
                }
            }
        }
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

    #[test]
    fn compare_db_path_root_files_before_subdirectory() {
        // MPD's Directory::Walk visits a directory's own songs before its
        // subdirectories, so root-level files sort before ANY path under a
        // subdirectory — even "rock" < "song1" would say otherwise under a
        // plain string sort.
        let mut paths = vec![
            "rock/track2.flac",
            "rock/track1.flac",
            "song3.flac",
            "song1.flac",
            "song2.flac",
        ];
        paths.sort_by(|a, b| compare_db_path(a, b));
        assert_eq!(
            paths,
            vec![
                "song1.flac",
                "song2.flac",
                "song3.flac",
                "rock/track1.flac",
                "rock/track2.flac",
            ]
        );
    }

    #[test]
    fn compare_db_path_file_before_deeper_subdirectory_even_when_name_sorts_after() {
        // "a/zzz.flac" (a file directly in "a") sorts before "a/deep/w.flac"
        // (a file inside "a"'s subdirectory "deep") even though "deep" <
        // "zzz" alphabetically — files always precede subdirectories at the
        // point they diverge.
        use std::cmp::Ordering;
        assert_eq!(
            compare_db_path("a/zzz.flac", "a/deep/w.flac"),
            Ordering::Less
        );
        assert_eq!(compare_db_path("a/x.flac", "a/deep/w.flac"), Ordering::Less);
        assert_eq!(compare_db_path("a/deep/w.flac", "b/y.flac"), Ordering::Less);
    }

    #[test]
    fn compare_db_path_same_directory_sorts_by_name() {
        assert_eq!(
            compare_db_path("dir/b.flac", "dir/a.flac"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_db_path("dir/a.flac", "dir/a.flac"),
            std::cmp::Ordering::Equal
        );
    }
}
