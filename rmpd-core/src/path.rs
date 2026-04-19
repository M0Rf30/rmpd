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
    if rel_path.starts_with('/') {
        return rel_path.to_string();
    }

    if let Some(music_dir) = music_dir {
        let music_dir = music_dir.trim_end_matches('/');
        format!("{music_dir}/{rel_path}")
    } else {
        rel_path.to_string()
    }
}
