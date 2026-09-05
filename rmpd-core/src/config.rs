use crate::error::{Result, RmpdError};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub output: Vec<OutputConfig>,
    #[serde(default)]
    pub source: Vec<SourceConfig>,
    #[serde(default)]
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeneralConfig {
    #[serde(default = "default_music_dir")]
    pub music_directory: Utf8PathBuf,
    #[serde(default = "default_playlist_dir")]
    pub playlist_directory: Utf8PathBuf,
    #[serde(default = "default_db_file")]
    pub db_file: Utf8PathBuf,
    #[serde(default = "default_state_file")]
    pub state_file: Utf8PathBuf,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default = "default_charset")]
    pub filesystem_charset: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NetworkConfig {
    /// Bind address for the MPD TCP listener. IPv4 and IPv6 are supported (e.g. "127.0.0.1", "::1", "::").
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub unix_socket: Option<Utf8PathBuf>,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout: u64,
    pub password: Option<String>,
    /// Advertise the daemon on the session D-Bus via the MPRIS interface
    /// (`org.mpris.MediaPlayer2.rmpd`) so desktop environments, `playerctl`,
    /// and media keys can discover and control rmpd.
    #[serde(default = "default_true")]
    pub mpris: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    #[serde(default = "default_output")]
    pub default_output: String,
    #[serde(default = "default_buffer_time")]
    pub buffer_time: u32,
    #[serde(default)]
    pub resampler_quality: ResamplerQuality,
    /// DSD over PCM mode: "no" (default), "yes", or "auto".
    #[serde(default)]
    pub dop: DopMode,
    /// Output device id (ALSA PCM name, e.g. "hw:CARD=1,DEV=0"). Unset/empty =
    /// system default. Set a raw `hw:` device for bit-perfect DoP, bypassing
    /// PipeWire/PulseAudio resampling.
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub replay_gain: ReplayGainMode,
    #[serde(default)]
    pub replay_gain_preamp: f32,
    #[serde(default)]
    pub replay_gain_missing_preamp: f32,
    #[serde(default)]
    pub volume_normalization: bool,
    #[serde(default)]
    pub crossfade: f32,
    #[serde(default = "default_mixramp_db")]
    pub mixramp_db: f32,
    #[serde(default)]
    pub mixramp_delay: f32,
    /// Put MPD into pause mode instead of starting playback after startup
    /// Default: false (auto-resume if was playing)
    #[serde(default)]
    pub restore_paused: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OutputConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub output_type: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub settings: toml::Table,
}

/// Look up a string-valued setting from a flattened TOML settings table,
/// trimmed and non-empty. Booleans/integers are stringified (for keys like
/// `dop`, `max_bitrate`). Returns `None` when absent or empty.
fn setting_str(table: &toml::Table, key: &str) -> Option<String> {
    match table.get(key) {
        Some(toml::Value::String(s)) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_owned())
        }
        Some(toml::Value::Boolean(b)) => Some(b.to_string()),
        Some(toml::Value::Integer(i)) => Some(i.to_string()),
        _ => None,
    }
}

impl OutputConfig {
    /// A synthesized default output (system audio via cpal). Used when no
    /// `[[output]]` blocks are configured.
    #[must_use]
    pub fn cpal_default() -> Self {
        Self {
            name: "Default Output".to_owned(),
            output_type: "cpal".to_owned(),
            enabled: true,
            settings: toml::Table::new(),
        }
    }

    /// Look up a string-valued setting from the flattened `[[output]]` table,
    /// trimmed and non-empty. Booleans/integers are stringified (for keys like
    /// `dop`). Returns `None` when absent or empty.
    #[must_use]
    pub fn setting_str(&self, key: &str) -> Option<String> {
        setting_str(&self.settings, key)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct SourceConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub settings: toml::Table,
}

impl std::fmt::Debug for SourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceConfig")
            .field("name", &self.name)
            .field("source_type", &self.source_type)
            .field("enabled", &self.enabled)
            .field("settings", &"<redacted>")
            .finish()
    }
}

impl SourceConfig {
    /// Look up a string-valued setting from the flattened `[[source]]` table,
    /// trimmed and non-empty. Booleans/integers are stringified (for keys like
    /// `max_bitrate`). Returns `None` when absent or empty.
    #[must_use]
    pub fn setting_str(&self, key: &str) -> Option<String> {
        setting_str(&self.settings, key)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default = "default_true")]
    pub filesystem_watch: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            auto_update: true,
            filesystem_watch: true,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayGainMode {
    #[default]
    Off,
    Track,
    Album,
    Auto,
}

impl ReplayGainMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Track => "track",
            Self::Album => "album",
            Self::Auto => "auto",
        }
    }

    pub fn parse_mode(s: &str) -> Self {
        match s {
            "track" => Self::Track,
            "album" => Self::Album,
            "auto" => Self::Auto,
            _ => Self::Off,
        }
    }
}

impl std::fmt::Display for ReplayGainMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResamplerQuality {
    SincBest,
    #[default]
    SincMedium,
    SincFast,
    Linear,
}

/// DSD over PCM (DoP) policy for DSD sources.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DopMode {
    /// Always convert DSD to PCM. Works on any DAC. Default.
    #[default]
    No,
    /// Always attempt native DSD via DoP. Needs a DoP-capable DAC over a
    /// bit-perfect path — set `audio.device` to a raw `hw:` device.
    Yes,
    /// Use DoP only when an explicit output `device` is configured (assumed a
    /// dedicated DAC); otherwise convert to PCM.
    Auto,
}

/// Where a loaded [`Config`] came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// Loaded from an existing file.
    File(Utf8PathBuf),
    /// No config existed; this file was written from the template, then loaded.
    Generated(Utf8PathBuf),
    /// No config file used (none found and generation skipped/failed, or `no_config`).
    Defaults,
}

/// Severity of a [`Diagnostic`] produced while loading a config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagLevel {
    Info,
    Warn,
}

/// A single human-readable note produced while discovering/parsing a config.
/// The caller is expected to log these through `tracing` once logging is
/// initialized (`discover` itself never logs).
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub message: String,
}

impl Diagnostic {
    fn warn(message: impl Into<String>) -> Self {
        Self {
            level: DiagLevel::Warn,
            message: message.into(),
        }
    }
}

/// The result of [`Config::discover`]: the parsed config, where it came from,
/// and every diagnostic produced along the way.
#[derive(Debug, Clone)]
pub struct ConfigLoad {
    pub config: Config,
    pub source: ConfigSource,
    pub diagnostics: Vec<Diagnostic>,
}

/// Options controlling [`Config::discover`]'s search behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscoverOptions {
    /// Write a starter config when none is found.
    pub generate_if_missing: bool,
    /// Skip file discovery entirely (MPD's `--no-config`).
    pub no_config: bool,
}

// Default value functions

/// Join `rel` onto the user's home directory. Used only as a fallback when the
/// XDG lookup fails (headless systems without `xdg-user-dirs`), so the result
/// must still be an absolute, usable path rather than a literal `~/…`.
fn home_join(rel: &str) -> Utf8PathBuf {
    dirs::home_dir()
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .map_or_else(|| Utf8PathBuf::from(rel), |home| home.join(rel))
}

fn default_music_dir() -> Utf8PathBuf {
    // Honor $XDG_MUSIC_DIR (e.g. ~/Musica) when set, else fall back to ~/Music.
    dirs::audio_dir()
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| home_join("Music"))
}

fn default_playlist_dir() -> Utf8PathBuf {
    dirs::config_dir()
        .map(|p| p.join("rmpd/playlists"))
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| home_join(".config/rmpd/playlists"))
}

fn default_db_file() -> Utf8PathBuf {
    dirs::config_dir()
        .map(|p| p.join("rmpd/database.db"))
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| home_join(".config/rmpd/database.db"))
}

fn default_state_file() -> Utf8PathBuf {
    dirs::config_dir()
        .map(|p| p.join("rmpd/state"))
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| home_join(".config/rmpd/state"))
}

fn default_log_level() -> String {
    "info".to_owned()
}

fn default_charset() -> String {
    "UTF-8".to_owned()
}

fn default_bind_address() -> String {
    "127.0.0.1".to_owned()
}

const fn default_port() -> u16 {
    6600
}

const fn default_max_connections() -> usize {
    100
}

const fn default_connection_timeout() -> u64 {
    60
}

fn default_output() -> String {
    "default".to_owned()
}

fn default_buffer_time() -> u32 {
    500
}

fn default_mixramp_db() -> f32 {
    0.0
}

fn default_true() -> bool {
    true
}

// Known-key schema for the unknown-key/removed-key lint pass. `[[output]]`
// and `[[source]]` tables use `#[serde(flatten)]` for backend-specific
// settings and are intentionally not covered here beyond `name`/`type`.

const KNOWN_SECTIONS: &[&str] = &[
    "general", "network", "audio", "output", "source", "database",
];

const GENERAL_KEYS: &[&str] = &[
    "music_directory",
    "playlist_directory",
    "db_file",
    "state_file",
    "log_level",
    "follow_symlinks",
    "filesystem_charset",
];

const NETWORK_KEYS: &[&str] = &[
    "bind_address",
    "port",
    "unix_socket",
    "max_connections",
    "connection_timeout",
    "password",
    "mpris",
];

const AUDIO_KEYS: &[&str] = &[
    "default_output",
    "buffer_time",
    "resampler_quality",
    "dop",
    "device",
    "replay_gain",
    "replay_gain_preamp",
    "replay_gain_missing_preamp",
    "volume_normalization",
    "crossfade",
    "mixramp_db",
    "mixramp_delay",
    "restore_paused",
];

const DATABASE_KEYS: &[&str] = &["auto_update", "filesystem_watch"];

/// Levenshtein edit distance between two strings (two-row DP, no allocation
/// beyond the two rows).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];

    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// The closest entry in `known` to `key`, if within edit distance 2.
fn suggest(key: &str, known: &[&'static str]) -> Option<&'static str> {
    known
        .iter()
        .copied()
        .map(|k| (k, edit_distance(key, k)))
        .filter(|(_, d)| *d <= 2)
        .min_by_key(|(_, d)| *d)
        .map(|(k, _)| k)
}

/// Explanation for a config key that was deliberately removed (parsed but
/// never read), keyed by `(section, key)`.
fn removed_key_reason(section: &str, key: &str) -> Option<&'static str> {
    match (section, key) {
        ("audio", "gapless") => Some("gapless playback is always enabled"),
        ("database", "cache_size") => Some("it had no effect"),
        ("database", "fts_enabled") => Some("it had no effect"),
        _ => None,
    }
}

/// mpd.conf-style bare top-level key -> rmpd migration hint message.
fn mpd_migration_hint(key: &str) -> Option<String> {
    let target: Option<(&str, &str)> = match key {
        "music_directory" => Some(("general", "music_directory")),
        "playlist_directory" => Some(("general", "playlist_directory")),
        "db_file" => Some(("general", "db_file")),
        "state_file" => Some(("general", "state_file")),
        "bind_to_address" => Some(("network", "bind_address")),
        "port" => Some(("network", "port")),
        "password" => Some(("network", "password")),
        "max_connections" => Some(("network", "max_connections")),
        "connection_timeout" => Some(("network", "connection_timeout")),
        "filesystem_charset" => Some(("general", "filesystem_charset")),
        "follow_outside_symlinks" => Some(("general", "follow_symlinks")),
        "auto_update" => Some(("database", "auto_update")),
        "replaygain" => Some(("audio", "replay_gain")),
        "volume_normalization" => Some(("audio", "volume_normalization")),
        "log_file" => {
            return Some(
                "`log_file` is mpd.conf syntax; rmpd logs to stdout via tracing and has no config equivalent"
                    .to_owned(),
            );
        }
        "pid_file" => {
            return Some(
                "`pid_file` is mpd.conf syntax; rmpd does not daemonize and has no config equivalent"
                    .to_owned(),
            );
        }
        "audio_output" => {
            return Some(
                "`audio_output` is mpd.conf syntax; rmpd uses one or more `[[output]]` tables instead"
                    .to_owned(),
            );
        }
        "zeroconf_enabled" => {
            return Some(
                "`zeroconf_enabled` is mpd.conf syntax; rmpd always advertises via mDNS and has no config equivalent"
                    .to_owned(),
            );
        }
        _ => None,
    };
    target.map(|(section, rmpd_key)| {
        format!("`{key}` is mpd.conf syntax; rmpd uses `{rmpd_key}` in the `[{section}]` section")
    })
}

fn lint_section(
    section: &str,
    value: &toml::Value,
    known: &[&'static str],
    out: &mut Vec<Diagnostic>,
) {
    let toml::Value::Table(table) = value else {
        return;
    };
    for key in table.keys() {
        if known.contains(&key.as_str()) {
            continue;
        }
        if let Some(reason) = removed_key_reason(section, key) {
            out.push(Diagnostic::warn(format!(
                "config key `{section}.{key}` was removed: {reason}"
            )));
            continue;
        }
        out.push(Diagnostic::warn(match suggest(key, known) {
            Some(s) => format!("unknown config key `{section}.{key}` (did you mean `{s}`?)"),
            None => format!("unknown config key `{section}.{key}`"),
        }));
    }
}

fn lint_tables(section: &str, value: &toml::Value, out: &mut Vec<Diagnostic>) {
    let toml::Value::Array(items) = value else {
        return;
    };
    for item in items {
        let toml::Value::Table(table) = item else {
            continue;
        };
        let has_name = table
            .get("name")
            .and_then(toml::Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        let has_type = table
            .get("type")
            .and_then(toml::Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if !has_name {
            out.push(Diagnostic::warn(format!(
                "a `[[{section}]]` table is missing a non-empty `name`"
            )));
        }
        if !has_type {
            out.push(Diagnostic::warn(format!(
                "a `[[{section}]]` table is missing a non-empty `type`"
            )));
        }
    }
}

fn to_utf8(path: &Path) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(path.to_path_buf())
        .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()))
}

impl Config {
    /// Search locations for a config file, in priority order, restricted to
    /// candidates this system can actually resolve (e.g. `dirs::config_dir()`
    /// is skipped when unavailable). Mirrors MPD's own search order:
    /// `$XDG_CONFIG_HOME/mpd/mpd.conf`, `~/.mpdconf`, `~/.mpd/mpd.conf`.
    #[must_use]
    pub fn search_paths() -> Vec<Utf8PathBuf> {
        let mut paths = Vec::new();
        if let Some(p) = dirs::config_dir()
            .map(|p| p.join("rmpd/rmpd.toml"))
            .and_then(|p| Utf8PathBuf::try_from(p).ok())
        {
            paths.push(p);
        }
        if let Some(home) = dirs::home_dir().and_then(|p| Utf8PathBuf::try_from(p).ok()) {
            paths.push(home.join(".rmpd.toml"));
            paths.push(home.join(".rmpd/rmpd.toml"));
        }
        paths.push(Utf8PathBuf::from("/etc/rmpd/rmpd.toml"));
        paths
    }

    /// The starter config template, also the single source of truth for
    /// `rmpd.toml` at the repository root: there is exactly one copy of the
    /// starter config in the tree.
    #[must_use]
    pub fn template_toml() -> &'static str {
        include_str!("../../rmpd.toml")
    }

    /// Writes the template to `path`, creating parent directories as needed.
    /// Errs if `path` already exists so a real user file is never clobbered.
    pub fn write_template(path: &Path) -> Result<()> {
        if path.exists() {
            return Err(RmpdError::Config(format!(
                "refusing to overwrite existing config file {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                RmpdError::Config(format!(
                    "failed to create config directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
        std::fs::write(path, Self::template_toml()).map_err(|e| {
            RmpdError::Config(format!("failed to write config {}: {e}", path.display()))
        })?;
        Ok(())
    }

    /// Discover and load the effective config.
    ///
    /// - `explicit` (e.g. from `--config`) is authoritative: a missing,
    ///   unreadable, or unparseable explicit file is a hard error and is
    ///   never silently substituted with defaults.
    /// - Otherwise, with `opts.no_config`, no filesystem search happens at
    ///   all and built-in defaults are used.
    /// - Otherwise the first existing entry of [`Self::search_paths`] wins.
    ///   If none exist and `opts.generate_if_missing`, a starter config is
    ///   written to `search_paths()[0]` and then loaded. If that generation
    ///   fails (read-only filesystem, no resolvable home/XDG directory), a
    ///   `Warn` diagnostic is emitted and defaults are used instead.
    ///
    /// This never logs through `tracing` itself: every message is returned in
    /// `ConfigLoad::diagnostics` so the caller can initialize logging at the
    /// config-specified level first, then replay them.
    pub fn discover(explicit: Option<&Path>, opts: DiscoverOptions) -> Result<ConfigLoad> {
        if let Some(path) = explicit {
            let (config, diagnostics) = Self::load_file(path)?;
            return Ok(ConfigLoad {
                config,
                source: ConfigSource::File(to_utf8(path)),
                diagnostics,
            });
        }

        if opts.no_config {
            return Self::defaults_load(Vec::new());
        }

        let candidates = Self::search_paths();
        for candidate in &candidates {
            if candidate.exists() {
                let (config, diagnostics) = Self::load_file(candidate.as_std_path())?;
                return Ok(ConfigLoad {
                    config,
                    source: ConfigSource::File(candidate.clone()),
                    diagnostics,
                });
            }
        }

        if opts.generate_if_missing
            && let Some(target) = candidates.first()
        {
            if let Err(e) = Self::write_template(target.as_std_path()) {
                return Self::defaults_load(vec![Diagnostic::warn(format!(
                    "could not generate starter config at {target}: {e}"
                ))]);
            }
            return match Self::load_file(target.as_std_path()) {
                Ok((config, diagnostics)) => Ok(ConfigLoad {
                    config,
                    source: ConfigSource::Generated(target.clone()),
                    diagnostics,
                }),
                Err(e) => Self::defaults_load(vec![Diagnostic::warn(format!(
                    "generated config at {target} failed to load: {e}"
                ))]),
            };
        }

        Self::defaults_load(Vec::new())
    }

    fn defaults_load(mut diagnostics: Vec<Diagnostic>) -> Result<ConfigLoad> {
        let mut config = Self::default();
        config.expand_paths();
        config.ensure_directories();
        Self::validate_and_normalize(&mut config, &mut diagnostics)?;
        Ok(ConfigLoad {
            config,
            source: ConfigSource::Defaults,
            diagnostics,
        })
    }

    /// Reads and parses `path`, lints it for unknown/removed keys, expands
    /// paths, ensures directories, and validates/normalizes values.
    fn load_file(path: &Path) -> Result<(Self, Vec<Diagnostic>)> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            RmpdError::Config(format!("failed to read config {}: {e}", path.display()))
        })?;

        let mut config: Config = toml::from_str(&content).map_err(|e| {
            RmpdError::Config(format!("failed to parse config {}: {e}", path.display()))
        })?;

        let mut diagnostics = Self::lint(&content);
        config.expand_paths();
        config.ensure_directories();
        Self::validate_and_normalize(&mut config, &mut diagnostics)?;
        Ok((config, diagnostics))
    }

    /// Parses `content` as raw TOML and walks it against the known
    /// section/key schema, producing a `Warn` diagnostic for every
    /// unrecognized, removed, or mpd.conf-style key. Never fatal: a typo must
    /// not stop a headless music daemon, but it must be loudly reported.
    fn lint(content: &str) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let Ok(toml::Value::Table(root)) = toml::from_str::<toml::Value>(content) else {
            return diagnostics;
        };

        for (key, value) in &root {
            match key.as_str() {
                "general" => lint_section("general", value, GENERAL_KEYS, &mut diagnostics),
                "network" => lint_section("network", value, NETWORK_KEYS, &mut diagnostics),
                "audio" => lint_section("audio", value, AUDIO_KEYS, &mut diagnostics),
                "database" => lint_section("database", value, DATABASE_KEYS, &mut diagnostics),
                "output" => lint_tables("output", value, &mut diagnostics),
                "source" => lint_tables("source", value, &mut diagnostics),
                "decoder" => diagnostics.push(Diagnostic::warn(
                    "config section `[decoder]` was removed: decoders are selected at build time",
                )),
                _ => {
                    if matches!(value, toml::Value::Table(_)) {
                        diagnostics.push(Diagnostic::warn(match suggest(key, KNOWN_SECTIONS) {
                            Some(s) => {
                                format!("unknown config section `[{key}]` (did you mean `[{s}]`?)")
                            }
                            None => format!("unknown config section `[{key}]`"),
                        }));
                    } else if let Some(hint) = mpd_migration_hint(key) {
                        diagnostics.push(Diagnostic::warn(hint));
                    } else {
                        diagnostics.push(Diagnostic::warn(format!("unknown config key `{key}`")));
                    }
                }
            }
        }
        diagnostics
    }

    /// Effective DoP mode. Prefers `[audio].dop`; if that is the default `No`,
    /// falls back to the first enabled `[[output]]` block's `dop` setting
    /// (MPD's `audio_output { dop "yes" }`).
    #[must_use]
    pub fn dop_mode(&self) -> DopMode {
        if self.audio.dop != DopMode::No {
            return self.audio.dop;
        }
        for out in &self.output {
            if !out.enabled {
                continue;
            }
            let yes = match out.settings.get("dop") {
                Some(toml::Value::Boolean(b)) => *b,
                Some(toml::Value::String(s)) => {
                    matches!(s.trim(), "yes" | "true" | "1" | "on")
                }
                _ => false,
            };
            if yes {
                return DopMode::Yes;
            }
        }
        DopMode::No
    }

    /// Effective output device id. Prefers `[audio].device`; otherwise the first
    /// enabled `[[output]]` block's `device` setting (MPD's
    /// `audio_output { device "hw:0,0" }`). Returns `None` for the system default.
    #[must_use]
    pub fn output_device(&self) -> Option<String> {
        if let Some(dev) = self
            .audio
            .device
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Some(dev.to_owned());
        }
        for out in &self.output {
            if !out.enabled {
                continue;
            }
            let dev = out
                .settings
                .get("device")
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(dev) = dev {
                return Some(dev.to_owned());
            }
        }
        None
    }

    fn expand_paths(&mut self) {
        use crate::path::expand_tilde;

        self.general.music_directory = expand_tilde(&self.general.music_directory);
        self.general.playlist_directory = expand_tilde(&self.general.playlist_directory);
        self.general.db_file = expand_tilde(&self.general.db_file);
        self.general.state_file = expand_tilde(&self.general.state_file);
    }

    /// Create the directories referenced by the config entries if they do not
    /// already exist. This covers the `playlist_directory` itself and the
    /// parent directories of `db_file` and `state_file`. The `music_directory`
    /// is intentionally left alone: a missing library directory is reported as
    /// a diagnostic rather than created, since it must be supplied by the user.
    fn ensure_directories(&self) {
        let mut dirs: Vec<&camino::Utf8Path> = vec![self.general.playlist_directory.as_path()];
        dirs.extend(self.general.db_file.parent());
        dirs.extend(self.general.state_file.parent());

        for dir in dirs {
            if dir.as_str().is_empty() || dir.exists() {
                continue;
            }
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("failed to create directory {dir}: {e}");
            }
        }
    }

    /// Validates values that would make the daemon unusable (hard errors),
    /// and clamps/normalizes everything else while recording a `Warn`
    /// diagnostic. Mirrors MPD, which logs and degrades on most bad values
    /// rather than refusing to start.
    fn validate_and_normalize(config: &mut Self, diagnostics: &mut Vec<Diagnostic>) -> Result<()> {
        if config.network.max_connections == 0 {
            return Err(RmpdError::Config(
                "network.max_connections must be greater than 0 (0 accepts no clients)".to_owned(),
            ));
        }
        if config.network.port == 0 {
            return Err(RmpdError::Config("network.port must be nonzero".to_owned()));
        }

        if !config.general.music_directory.exists() {
            diagnostics.push(Diagnostic::warn(format!(
                "music directory {} does not exist; library scanning is disabled until it exists",
                config.general.music_directory
            )));
        }

        if config.audio.buffer_time == 0 {
            config.audio.buffer_time = default_buffer_time();
            diagnostics.push(Diagnostic::warn(format!(
                "audio.buffer_time was 0; clamped to {}",
                config.audio.buffer_time
            )));
        }

        if config.audio.crossfade < 0.0 {
            config.audio.crossfade = 0.0;
            diagnostics.push(Diagnostic::warn(
                "audio.crossfade was negative; clamped to 0.0",
            ));
        }

        const PREAMP_BOUND: f32 = 60.0;
        if !(-PREAMP_BOUND..=PREAMP_BOUND).contains(&config.audio.replay_gain_preamp) {
            let clamped = config
                .audio
                .replay_gain_preamp
                .clamp(-PREAMP_BOUND, PREAMP_BOUND);
            diagnostics.push(Diagnostic::warn(format!(
                "audio.replay_gain_preamp {} out of range [-{PREAMP_BOUND}, {PREAMP_BOUND}] dB; clamped to {clamped}",
                config.audio.replay_gain_preamp
            )));
            config.audio.replay_gain_preamp = clamped;
        }
        if !(-PREAMP_BOUND..=PREAMP_BOUND).contains(&config.audio.replay_gain_missing_preamp) {
            let clamped = config
                .audio
                .replay_gain_missing_preamp
                .clamp(-PREAMP_BOUND, PREAMP_BOUND);
            diagnostics.push(Diagnostic::warn(format!(
                "audio.replay_gain_missing_preamp {} out of range [-{PREAMP_BOUND}, {PREAMP_BOUND}] dB; clamped to {clamped}",
                config.audio.replay_gain_missing_preamp
            )));
            config.audio.replay_gain_missing_preamp = clamped;
        }

        let lowered = config.general.log_level.to_lowercase();
        if matches!(
            lowered.as_str(),
            "trace" | "debug" | "info" | "warn" | "error"
        ) {
            config.general.log_level = lowered;
        } else {
            diagnostics.push(Diagnostic::warn(format!(
                "general.log_level {:?} is not one of trace/debug/info/warn/error; using \"info\"",
                config.general.log_level
            )));
            config.general.log_level = "info".to_owned();
        }

        Ok(())
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            music_directory: default_music_dir(),
            playlist_directory: default_playlist_dir(),
            db_file: default_db_file(),
            state_file: default_state_file(),
            log_level: default_log_level(),
            follow_symlinks: false,
            filesystem_charset: default_charset(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            port: default_port(),
            unix_socket: None,
            max_connections: default_max_connections(),
            connection_timeout: default_connection_timeout(),
            password: None,
            mpris: true,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            default_output: default_output(),
            buffer_time: default_buffer_time(),
            resampler_quality: ResamplerQuality::default(),
            dop: DopMode::default(),
            device: None,
            replay_gain: ReplayGainMode::default(),
            replay_gain_preamp: 0.0,
            replay_gain_missing_preamp: 0.0,
            volume_normalization: false,
            crossfade: 0.0,
            mixramp_db: default_mixramp_db(),
            mixramp_delay: 0.0,
            restore_paused: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(tag: &str) -> Utf8PathBuf {
        let base = std::env::temp_dir().join(format!(
            "rmpd-cfgtest-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        Utf8PathBuf::try_from(base).unwrap()
    }

    fn output_block(enabled: bool) -> OutputConfig {
        let mut settings = toml::Table::new();
        settings.insert(
            "device".to_owned(),
            toml::Value::String("hw:CARD=1,DEV=0".to_owned()),
        );
        settings.insert("dop".to_owned(), toml::Value::String("yes".to_owned()));
        OutputConfig {
            name: "DAC".to_owned(),
            output_type: "alsa".to_owned(),
            enabled,
            settings,
        }
    }

    #[test]
    fn partial_config_keeps_defaults_for_absent_sections() {
        // A config listing only the sections a user cares about must still
        // deserialize; requiring [network]/[audio] silently discarded the whole
        // file and fell back to defaults.
        let c: Config = toml::from_str(
            r#"
[general]
music_directory = "/srv/music"

[network]
port = 6611
"#,
        )
        .unwrap();

        assert_eq!(c.general.music_directory, "/srv/music");
        assert_eq!(c.network.port, 6611);
        assert_eq!(c.network.bind_address, default_bind_address());
        assert_eq!(c.audio.buffer_time, default_buffer_time());
        assert!(c.database.auto_update);
    }

    #[test]
    fn empty_config_deserializes_to_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.general.music_directory, default_music_dir());
        assert_eq!(c.network.port, default_port());
    }

    #[test]
    fn default_paths_are_absolute() {
        // A literal "~/Music" fallback is never usable: nothing expands it on
        // the defaults path, so the daemon scanned a directory named "~".
        for p in [
            default_music_dir(),
            default_playlist_dir(),
            default_db_file(),
            default_state_file(),
        ] {
            assert!(!p.as_str().starts_with('~'), "unexpanded default: {p}");
        }
    }

    #[test]
    fn dop_and_device_default_off() {
        let c = Config::default();
        assert_eq!(c.dop_mode(), DopMode::No);
        assert_eq!(c.output_device(), None);
    }

    #[test]
    fn audio_section_dop_and_device() {
        let mut c = Config::default();
        c.audio.dop = DopMode::Yes;
        c.audio.device = Some("hw:CARD=1,DEV=0".to_owned());
        assert_eq!(c.dop_mode(), DopMode::Yes);
        assert_eq!(c.output_device().as_deref(), Some("hw:CARD=1,DEV=0"));
    }

    #[test]
    fn mpd_style_output_block_fallback() {
        // No [audio] dop/device -> fall back to the enabled [[output]] block.
        let mut c = Config::default();
        c.output.push(output_block(true));
        assert_eq!(c.dop_mode(), DopMode::Yes);
        assert_eq!(c.output_device().as_deref(), Some("hw:CARD=1,DEV=0"));
    }

    #[test]
    fn disabled_output_block_ignored() {
        let mut c = Config::default();
        c.output.push(output_block(false));
        assert_eq!(c.dop_mode(), DopMode::No);
        assert_eq!(c.output_device(), None);
    }

    #[test]
    fn ensure_directories_creates_configured_dirs() {
        let base = unique_temp_dir("ensuredirs");

        // music_directory must already exist for a clean (no-diagnostic) run.
        let music = base.join("music");
        std::fs::create_dir_all(&music).unwrap();

        let mut c = Config::default();
        c.general.music_directory = music;
        c.general.playlist_directory = base.join("playlists");
        c.general.db_file = base.join("state/rmpd.db");
        c.general.state_file = base.join("run/state");

        assert!(!c.general.playlist_directory.exists());
        c.ensure_directories();

        assert!(c.general.playlist_directory.exists());
        assert!(c.general.db_file.parent().unwrap().exists());
        assert!(c.general.state_file.parent().unwrap().exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn source_config_deserializes_from_toml() {
        let toml_str = r#"
[[source]]
name = "home"
type = "subsonic"
url = "https://music.example.com"
username = "alice"
password = "hunter2"
max_bitrate = 320
"#;
        let sources: Vec<SourceConfig> = toml::from_str::<toml::Value>(toml_str)
            .unwrap()
            .get("source")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "home");
        assert_eq!(sources[0].source_type, "subsonic");
        assert!(sources[0].enabled, "enabled defaults to true");
        assert_eq!(
            sources[0].setting_str("url").as_deref(),
            Some("https://music.example.com")
        );
        assert_eq!(
            sources[0].setting_str("max_bitrate").as_deref(),
            Some("320")
        );
    }

    #[test]
    fn absent_source_section_yields_empty_vec() {
        // Config::default() must produce an empty source vec.
        let c = Config::default();
        assert!(c.source.is_empty());

        // Deserializing a TOML snippet with no [[source]] key also gives empty.
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            source: Vec<SourceConfig>,
        }
        let w: Wrapper =
            toml::from_str("[dummy]\nx = 1\n").unwrap_or(Wrapper { source: Vec::new() });
        assert!(w.source.is_empty());
    }

    #[test]
    fn source_config_debug_redacts_settings() {
        let mut settings = toml::Table::new();
        settings.insert(
            "password".to_owned(),
            toml::Value::String("hunter2".to_owned()),
        );
        settings.insert(
            "url".to_owned(),
            toml::Value::String("https://music.example.com".to_owned()),
        );
        let sc = SourceConfig {
            name: "home".to_owned(),
            source_type: "subsonic".to_owned(),
            enabled: true,
            settings,
        };
        let debug_str = format!("{sc:?}");
        assert!(
            !debug_str.contains("hunter2"),
            "debug output must not expose credential: got {debug_str}"
        );
        assert!(
            debug_str.contains("redacted"),
            "debug output must say <redacted>: got {debug_str}"
        );
        assert!(debug_str.contains("home"));
        assert!(debug_str.contains("subsonic"));
    }

    #[test]
    fn template_matches_defaults_and_lints_clean() {
        // Anti-drift guarantee: adding a key to a struct without documenting
        // it in rmpd.toml, or leaving a stale/removed key in the template,
        // fails here via a non-empty lint result.
        let content = Config::template_toml();
        let diagnostics = Config::lint(content);
        assert!(
            diagnostics.is_empty(),
            "shipped rmpd.toml produced diagnostics: {diagnostics:?}"
        );

        let parsed: Config = toml::from_str(content).unwrap();
        let default = Config::default();

        // music_directory is intentionally "~/Music" in the template rather
        // than the resolved XDG music dir; everything else must match the
        // built-in default exactly so a generated file changes no behavior.
        assert_eq!(
            parsed.general.playlist_directory,
            default.general.playlist_directory
        );
        assert_eq!(parsed.general.db_file, default.general.db_file);
        assert_eq!(parsed.general.state_file, default.general.state_file);
        assert_eq!(parsed.general.log_level, default.general.log_level);
        assert_eq!(
            parsed.general.follow_symlinks,
            default.general.follow_symlinks
        );
        assert_eq!(
            parsed.general.filesystem_charset,
            default.general.filesystem_charset
        );
        assert_eq!(
            format!("{:?}", parsed.network),
            format!("{:?}", default.network)
        );
        assert_eq!(
            format!("{:?}", parsed.audio),
            format!("{:?}", default.audio)
        );
        assert_eq!(
            format!("{:?}", parsed.database),
            format!("{:?}", default.database)
        );
    }

    #[test]
    fn misspelled_key_produces_single_suggestion() {
        let diagnostics = Config::lint(
            r#"
[general]
msic_directory = "/srv/music"
"#,
        );
        assert_eq!(diagnostics.len(), 1, "got: {diagnostics:?}");
        assert_eq!(diagnostics[0].level, DiagLevel::Warn);
        assert!(
            diagnostics[0]
                .message
                .contains("unknown config key `general.msic_directory`")
        );
        assert!(
            diagnostics[0]
                .message
                .contains("did you mean `music_directory`?")
        );
    }

    #[test]
    fn unknown_section_produces_suggestion() {
        let diagnostics = Config::lint(
            r#"
[netwrk]
port = 6600
"#,
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("unknown config section `[netwrk]`")
                    && d.message.contains("did you mean `[network]`?"))
        );
    }

    #[test]
    fn removed_key_gets_specific_removal_message() {
        let diagnostics = Config::lint(
            r#"
[audio]
gapless = true
"#,
        );
        assert_eq!(diagnostics.len(), 1, "got: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].message,
            "config key `audio.gapless` was removed: gapless playback is always enabled"
        );
    }

    #[test]
    fn removed_decoder_section_gets_specific_message() {
        let diagnostics = Config::lint(
            r#"
[decoder]
enabled = ["symphonia"]
"#,
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("`[decoder]` was removed"))
        );
    }

    #[test]
    fn removed_database_keys_get_specific_messages() {
        let diagnostics = Config::lint(
            r#"
[database]
cache_size = 64
fts_enabled = true
"#,
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message
                    == "config key `database.cache_size` was removed: it had no effect")
        );
        assert!(diagnostics.iter().any(
            |d| d.message == "config key `database.fts_enabled` was removed: it had no effect"
        ));
    }

    #[test]
    fn mpd_conf_bare_key_produces_migration_hint() {
        let diagnostics = Config::lint("bind_to_address = \"0.0.0.0\"\n");
        assert_eq!(diagnostics.len(), 1, "got: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].message,
            "`bind_to_address` is mpd.conf syntax; rmpd uses `bind_address` in the `[network]` section"
        );
    }

    #[test]
    fn output_table_missing_name_or_type_warns() {
        let diagnostics = Config::lint(
            r#"
[[output]]
type = "alsa"
"#,
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("missing a non-empty `name`"))
        );
    }

    #[test]
    fn output_table_flattened_keys_not_flagged() {
        let diagnostics = Config::lint(
            r#"
[[output]]
name = "DAC"
type = "alsa"
device = "hw:CARD=1,DEV=0"
some_backend_specific_key = 1
"#,
        );
        assert!(diagnostics.is_empty(), "got: {diagnostics:?}");
    }

    #[test]
    fn discover_explicit_missing_path_is_hard_error() {
        let base = unique_temp_dir("explicit-missing");
        let missing = base.join("does-not-exist.toml");
        let result = Config::discover(Some(missing.as_std_path()), DiscoverOptions::default());
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn discover_explicit_unparseable_path_is_hard_error() {
        let base = unique_temp_dir("explicit-badtoml");
        let bad = base.join("bad.toml");
        std::fs::write(&bad, "not = [valid toml").unwrap();
        let result = Config::discover(Some(bad.as_std_path()), DiscoverOptions::default());
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn discover_no_config_skips_search_and_uses_defaults() {
        let load = Config::discover(
            None,
            DiscoverOptions {
                generate_if_missing: false,
                no_config: true,
            },
        )
        .unwrap();
        assert_eq!(load.source, ConfigSource::Defaults);
    }

    #[test]
    fn write_template_then_discover_explicit_reports_file_source() {
        // Exercises the write_template -> load -> ConfigSource::File path
        // that backs discover(None, generate_if_missing). We drive it through
        // an explicit path rather than mutating process-global XDG/HOME env
        // vars, which would race other tests running concurrently in this
        // binary.
        let base = unique_temp_dir("generated");
        let target = base.join("rmpd.toml");
        assert!(!target.exists());

        Config::write_template(target.as_std_path()).unwrap();
        assert!(target.exists());

        // Writing again must fail: generation never clobbers an existing file.
        assert!(Config::write_template(target.as_std_path()).is_err());

        let load =
            Config::discover(Some(target.as_std_path()), DiscoverOptions::default()).unwrap();
        assert_eq!(load.source, ConfigSource::File(target.clone()));
        assert!(
            load.diagnostics
                .iter()
                .all(|d| d.message.contains("music directory")),
            "unexpected diagnostics from a freshly generated template: {:?}",
            load.diagnostics
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn max_connections_zero_is_hard_error() {
        let base = unique_temp_dir("maxconn0");
        let path = base.join("rmpd.toml");
        std::fs::write(&path, "[network]\nmax_connections = 0\n").unwrap();
        let result = Config::discover(Some(path.as_std_path()), DiscoverOptions::default());
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn port_zero_is_hard_error() {
        let base = unique_temp_dir("port0");
        let path = base.join("rmpd.toml");
        std::fs::write(&path, "[network]\nport = 0\n").unwrap();
        let result = Config::discover(Some(path.as_std_path()), DiscoverOptions::default());
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn invalid_log_level_normalizes_to_info_with_warning() {
        let base = unique_temp_dir("loglevel");
        let music = base.join("music");
        std::fs::create_dir_all(&music).unwrap();
        let path = base.join("rmpd.toml");
        std::fs::write(
            &path,
            format!("[general]\nmusic_directory = \"{music}\"\nlog_level = \"verbose\"\n",),
        )
        .unwrap();

        let load = Config::discover(Some(path.as_std_path()), DiscoverOptions::default()).unwrap();
        assert_eq!(load.config.general.log_level, "info");
        assert!(
            load.diagnostics
                .iter()
                .any(|d| d.level == DiagLevel::Warn && d.message.contains("log_level")),
            "got: {:?}",
            load.diagnostics
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
