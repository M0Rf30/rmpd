// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mopidy-style config layering for the CLI: several `--config` files (or
//! directories of `*.toml`) merged in order, `-o section.key=value`
//! overrides on top, and a secrets-masked dump of the effective config.

use super::{Config, ConfigLoad, ConfigSource, Diagnostic, DiscoverOptions, to_utf8};
use crate::error::{Result, RmpdError};
use std::path::{Path, PathBuf};

/// Placeholder printed instead of a secret value.
const MASK: &str = "********";

/// Key fragments whose string values are secrets.
const SECRET_KEYS: &[&str] = &["password", "token", "secret", "api_key", "session_key"];

/// Deep-merge `overlay` into `base`: tables merge recursively, anything else
/// (scalars, arrays, arrays of tables) is replaced by the overlay's value.
pub fn merge_tables(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge_tables(b, o),
            (_, v) => {
                base.insert(key, v);
            }
        }
    }
}

/// Apply one `section.key=value` (or Mopidy's `section/key=value`) override.
/// Nested tables use more segments (`stream.proxy.url=...`). The value is
/// parsed as a TOML literal (`8000`, `true`, `["a", "b"]`) and falls back to
/// a plain string, so `general.music_directory=~/Music` needs no quoting.
///
/// # Errors
/// Malformed option, or a path segment that is not a table (e.g. inside an
/// `[[output]]` array).
pub fn apply_override(root: &mut toml::Table, option: &str) -> Result<()> {
    let bad = |why: &str| RmpdError::Config(format!("invalid option `{option}`: {why}"));
    let (path, raw) = option
        .split_once('=')
        .ok_or_else(|| bad("expected section.key=value"))?;
    let segments: Vec<&str> = path.split(['.', '/']).map(str::trim).collect();
    if segments.len() < 2 || segments.iter().any(|s| s.is_empty()) {
        return Err(bad("expected section.key=value"));
    }
    let raw = raw.trim();
    let value = toml::from_str::<toml::Table>(&format!("v = {raw}"))
        .ok()
        .and_then(|mut t| t.remove("v"))
        .unwrap_or_else(|| toml::Value::String(raw.to_owned()));

    let (last, parents) = segments.split_last().ok_or_else(|| bad("empty key"))?;
    let mut table = root;
    for seg in parents {
        let entry = table
            .entry((*seg).to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        table = match entry {
            toml::Value::Table(t) => t,
            _ => return Err(bad(&format!("`{seg}` is not a table"))),
        };
    }
    table.insert((*last).to_owned(), value);
    Ok(())
}

/// Replace every secret value in `value` with a mask; strip credentials
/// embedded in URLs (`scheme://user:pass@host`).
pub fn mask_secrets(value: &mut toml::Value) {
    match value {
        toml::Value::Table(t) => {
            for (k, v) in t.iter_mut() {
                let key = k.to_ascii_lowercase();
                if SECRET_KEYS.iter().any(|s| key.contains(s)) {
                    mask_leaf(v);
                } else {
                    mask_secrets(v);
                }
            }
        }
        toml::Value::Array(items) => items.iter_mut().for_each(mask_secrets),
        toml::Value::String(s) => {
            if let Some(stripped) = strip_url_credentials(s) {
                *s = stripped;
            }
        }
        _ => {}
    }
}

fn mask_leaf(v: &mut toml::Value) {
    match v {
        toml::Value::String(s) if !s.is_empty() => *s = MASK.to_owned(),
        toml::Value::Table(_) | toml::Value::Array(_) => {
            // e.g. `passwords = [{ password, permissions }]`
            if let toml::Value::Array(items) = v {
                for item in items {
                    if let toml::Value::Table(t) = item {
                        for (k, iv) in t.iter_mut() {
                            if SECRET_KEYS
                                .iter()
                                .any(|s| k.to_ascii_lowercase().contains(s))
                            {
                                mask_leaf(iv);
                            }
                        }
                    } else {
                        mask_leaf(item);
                    }
                }
            } else {
                mask_secrets(v);
            }
        }
        _ => {}
    }
}

fn strip_url_credentials(s: &str) -> Option<String> {
    let (scheme, rest) = s.split_once("://")?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let at = rest[..authority_end].rfind('@')?;
    Some(format!("{scheme}://{MASK}@{}", &rest[at + 1..]))
}

/// Expand a `--config` argument: a file, a directory (all `*.toml` inside,
/// sorted), or several of either separated by `:` (Mopidy syntax).
fn expand_config_args(args: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for arg in args {
        let s = arg.to_string_lossy();
        for part in s.split(':').filter(|p| !p.is_empty()) {
            let p = Path::new(part);
            if p.is_dir() {
                let mut files: Vec<PathBuf> = std::fs::read_dir(p)
                    .map_err(|e| {
                        RmpdError::Config(format!("failed to read config dir {}: {e}", p.display()))
                    })?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|f| f.extension().is_some_and(|x| x == "toml") && f.is_file())
                    .collect();
                files.sort();
                out.extend(files);
            } else {
                out.push(p.to_path_buf());
            }
        }
    }
    Ok(out)
}

fn read_table(path: &Path) -> Result<toml::Table> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| RmpdError::Config(format!("failed to read config {}: {e}", path.display())))?;
    toml::from_str(&content)
        .map_err(|e| RmpdError::Config(format!("failed to parse config {}: {e}", path.display())))
}

impl Config {
    /// [`Config::discover`] plus layering: every `--config` argument (files,
    /// directories, `a:b` lists) is merged in order, then `overrides` are
    /// applied. Without extra files or overrides this is exactly `discover`.
    ///
    /// # Errors
    /// Unreadable/unparseable explicit files, malformed overrides, or an
    /// invalid merged config.
    pub fn discover_layered(
        configs: &[PathBuf],
        overrides: &[String],
        opts: DiscoverOptions,
    ) -> Result<ConfigLoad> {
        let files = expand_config_args(configs)?;
        if files.len() <= 1 && overrides.is_empty() {
            return Self::discover(files.first().map(PathBuf::as_path), opts);
        }

        let mut diagnostics = Vec::new();
        let (mut table, source) = if files.is_empty() {
            let base = Self::discover(None, opts)?;
            match &base.source {
                ConfigSource::File(p) | ConfigSource::Generated(p) => {
                    (read_table(p.as_std_path())?, base.source.clone())
                }
                ConfigSource::Defaults => (toml::Table::new(), ConfigSource::Defaults),
            }
        } else {
            let mut merged = toml::Table::new();
            for f in &files {
                merge_tables(&mut merged, read_table(f)?);
            }
            if files.len() > 1 {
                let names: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
                diagnostics.push(Diagnostic::info(format!(
                    "merged config files (later wins): {}",
                    names.join(", ")
                )));
            }
            let last = files.last().map(|p| to_utf8(p)).unwrap_or_default();
            (merged, ConfigSource::File(last))
        };

        for o in overrides {
            apply_override(&mut table, o)?;
        }
        if !overrides.is_empty() {
            diagnostics.push(Diagnostic::info(format!(
                "{} command-line config override(s) applied",
                overrides.len()
            )));
        }

        let content = toml::to_string(&table)
            .map_err(|e| RmpdError::Config(format!("failed to serialize merged config: {e}")))?;
        let (config, mut more) = Self::load_content(&content, "(merged)")?;
        diagnostics.append(&mut more);
        Ok(ConfigLoad {
            config,
            source,
            diagnostics,
        })
    }

    /// The effective config as TOML with secrets masked (`rmpd config`).
    ///
    /// # Errors
    /// Serialization failure.
    pub fn to_masked_toml(&self) -> Result<String> {
        let mut value = toml::Value::try_from(self)
            .map_err(|e| RmpdError::Config(format!("failed to serialize config: {e}")))?;
        mask_secrets(&mut value);
        toml::to_string_pretty(&value)
            .map_err(|e| RmpdError::Config(format!("failed to serialize config: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> toml::Table {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn merge_is_deep_and_later_wins() {
        let mut base = t("[network]\nport = 6600\nbind_address = \"a\"\n");
        merge_tables(&mut base, t("[network]\nport = 7000\n"));
        assert_eq!(base["network"]["port"].as_integer(), Some(7000));
        assert_eq!(base["network"]["bind_address"].as_str(), Some("a"));
    }

    #[test]
    fn override_parses_literals_and_strings() {
        let mut root = toml::Table::new();
        apply_override(&mut root, "network.port=7000").unwrap();
        apply_override(&mut root, "general/music_directory=~/Music").unwrap();
        apply_override(&mut root, "stream.proxy.url = http://p:3128").unwrap();
        apply_override(&mut root, "stream.metadata_blacklist=[\"a*\"]").unwrap();
        assert_eq!(root["network"]["port"].as_integer(), Some(7000));
        assert_eq!(root["general"]["music_directory"].as_str(), Some("~/Music"));
        assert_eq!(
            root["stream"]["proxy"]["url"].as_str(),
            Some("http://p:3128")
        );
        assert!(root["stream"]["metadata_blacklist"].is_array());
    }

    #[test]
    fn override_rejects_malformed() {
        let mut root = t("output = [{ name = \"x\" }]");
        assert!(apply_override(&mut root, "novalue").is_err());
        assert!(apply_override(&mut root, "port=1").is_err());
        assert!(apply_override(&mut root, "output.name=y").is_err());
    }

    #[test]
    fn secrets_are_masked() {
        let mut v = toml::Value::Table(t(r#"
[network]
password = "hunter2"
passwords = [{ password = "p", permissions = ["read"] }]
[stream.proxy]
url = "http://user:pw@proxy:3128/"
[[source]]
name = "home"
api_key = "k"
url = "https://music.example"
"#));
        mask_secrets(&mut v);
        let s = toml::to_string(&v).unwrap();
        for secret in ["hunter2", "\"p\"", "pw@", "\"k\""] {
            assert!(!s.contains(secret), "{secret} leaked in {s}");
        }
        assert!(s.contains("read"));
        assert!(s.contains("https://music.example"));
    }

    #[test]
    fn layered_files_merge_in_order() {
        let dir = std::env::temp_dir().join(format!("rmpd-layers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.toml");
        let b = dir.join("b.toml");
        std::fs::write(&a, "[network]\nport = 7000\n").unwrap();
        std::fs::write(&b, "[network]\nport = 7001\n").unwrap();
        let arg = PathBuf::from(format!("{}:{}", a.display(), b.display()));
        let load = Config::discover_layered(
            &[arg],
            &["network.bind_address=127.0.0.2".to_owned()],
            DiscoverOptions::default(),
        )
        .unwrap();
        assert_eq!(load.config.network.port, 7001);
        assert_eq!(load.config.network.bind_address, "127.0.0.2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
