// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

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

/// Resolve one of MPD's `$VARIABLE` path prefixes (`ParsePath` in
/// `src/config/Path.cxx`) to a directory.
///
/// The supported names are exactly MPD's: `HOME`, `XDG_CONFIG_HOME`,
/// `XDG_MUSIC_DIR`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_RUNTIME_DIR` and
/// `XDG_STATE_HOME`. Returns `Err` for an unknown name and `Ok(None)` when the
/// name is known but this system has no value for it.
fn resolve_variable(name: &str) -> Result<Option<std::path::PathBuf>, String> {
    match name {
        "HOME" => Ok(dirs::home_dir()),
        "XDG_CONFIG_HOME" => Ok(dirs::config_dir()),
        "XDG_MUSIC_DIR" => Ok(dirs::audio_dir()),
        "XDG_DATA_HOME" => Ok(dirs::data_dir()),
        "XDG_CACHE_HOME" => Ok(dirs::cache_dir()),
        "XDG_RUNTIME_DIR" => Ok(dirs::runtime_dir()),
        "XDG_STATE_HOME" => Ok(dirs::state_dir()),
        _ => Err(format!("unknown variable: {name:?}")),
    }
}

/// Expand a configured path the way MPD's `ParsePath` does: a leading `~`
/// (`~` or `~/...`) becomes the home directory and a leading `$NAME` (see
/// [`resolve_variable`]) becomes that directory; everything else is returned
/// unchanged. `~user/...` is left alone (not supported).
///
/// # Errors
/// Returns a message when the path starts with `$` but the variable is
/// unknown or has no value on this system.
pub fn try_expand_path(path: &str) -> Result<String, String> {
    expand_path_with(path, resolve_variable)
}

/// [`try_expand_path`] with an injectable variable resolver (`HOME` is
/// requested for `~`), so the logic can be tested without touching the
/// process environment.
fn expand_path_with(
    path: &str,
    resolve: impl Fn(&str) -> Result<Option<std::path::PathBuf>, String>,
) -> Result<String, String> {
    fn join(base: &std::path::Path, rest: &str) -> Result<String, String> {
        let base = base
            .to_str()
            .ok_or_else(|| "directory is not valid UTF-8".to_owned())?;
        let rest = rest.trim_start_matches('/');
        Ok(if rest.is_empty() {
            base.to_owned()
        } else {
            format!("{}/{rest}", base.trim_end_matches('/'))
        })
    }

    if let Some(rest) = path.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            let Some(home) = resolve("HOME")? else {
                return Ok(path.to_owned());
            };
            return join(&home, rest);
        }
        // "~user/..." is not supported; leave it as is.
        return Ok(path.to_owned());
    }

    if let Some(rest) = path.strip_prefix('$') {
        let (name, rest) = rest.split_once('/').unwrap_or((rest, ""));
        let dir = resolve(name)?.ok_or_else(|| format!("no value for variable: {name:?}"))?;
        return join(&dir, rest);
    }

    Ok(path.to_owned())
}

/// Expand `~` and MPD's `$XDG_*`/`$HOME` prefixes in `path`, returning it
/// unchanged (and the reason) when a variable cannot be resolved.
pub fn expand_path(path: &Utf8PathBuf) -> (Utf8PathBuf, Option<String>) {
    match try_expand_path(path.as_str()) {
        Ok(expanded) => (Utf8PathBuf::from(expanded), None),
        Err(e) => (path.clone(), Some(e)),
    }
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

    #[test]
    fn expand_path_with_tilde_and_variables() {
        use std::path::PathBuf;
        let resolve = |name: &str| -> Result<Option<PathBuf>, String> {
            match name {
                "HOME" => Ok(Some(PathBuf::from("/home/u"))),
                "XDG_STATE_HOME" => Ok(Some(PathBuf::from("/home/u/.local/state"))),
                "XDG_RUNTIME_DIR" => Ok(None),
                _ => Err(format!("unknown variable: {name:?}")),
            }
        };
        assert_eq!(expand_path_with("~", resolve).unwrap(), "/home/u");
        assert_eq!(
            expand_path_with("~/Music", resolve).unwrap(),
            "/home/u/Music"
        );
        assert_eq!(
            expand_path_with("$XDG_STATE_HOME/rmpd/state", resolve).unwrap(),
            "/home/u/.local/state/rmpd/state"
        );
        assert_eq!(
            expand_path_with("$HOME", resolve).unwrap(),
            "/home/u",
            "bare variable without a trailing component"
        );
        // Absolute, relative and ~user paths pass through untouched.
        assert_eq!(expand_path_with("/abs/x", resolve).unwrap(), "/abs/x");
        assert_eq!(expand_path_with("rel/x", resolve).unwrap(), "rel/x");
        assert_eq!(expand_path_with("~bob/x", resolve).unwrap(), "~bob/x");
        // Unknown variable / variable without a value are errors (MPD throws).
        assert!(
            expand_path_with("$NOPE/x", resolve)
                .unwrap_err()
                .contains("unknown")
        );
        assert!(
            expand_path_with("$XDG_RUNTIME_DIR/mpd/socket", resolve)
                .unwrap_err()
                .contains("no value")
        );
    }

    #[test]
    fn expand_path_leaves_unresolvable_variable_unchanged() {
        let p = Utf8PathBuf::from("$DEFINITELY_NOT_A_VARIABLE/x");
        let (out, err) = expand_path(&p);
        assert_eq!(out, p);
        assert!(err.is_some());
        let (out, err) = expand_path(&Utf8PathBuf::from("/plain"));
        assert_eq!(out, "/plain");
        assert!(err.is_none());
    }
}
