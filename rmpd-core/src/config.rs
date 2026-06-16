use crate::error::{Result, RmpdError};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub network: NetworkConfig,
    pub audio: AudioConfig,
    #[serde(default)]
    pub output: Vec<OutputConfig>,
    #[serde(default)]
    pub source: Vec<SourceConfig>,
    #[serde(default)]
    pub decoder: DecoderConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeneralConfig {
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
    #[serde(default = "default_true")]
    pub gapless: bool,
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
        match self.settings.get(key) {
            Some(toml::Value::String(s)) => {
                let t = s.trim();
                (!t.is_empty()).then(|| t.to_owned())
            }
            Some(toml::Value::Boolean(b)) => Some(b.to_string()),
            Some(toml::Value::Integer(i)) => Some(i.to_string()),
            _ => None,
        }
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
        match self.settings.get(key) {
            Some(toml::Value::String(s)) => {
                let t = s.trim();
                (!t.is_empty()).then(|| t.to_owned())
            }
            Some(toml::Value::Boolean(b)) => Some(b.to_string()),
            Some(toml::Value::Integer(i)) => Some(i.to_string()),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct DecoderConfig {
    #[serde(default = "default_enabled_decoders")]
    pub enabled: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default = "default_true")]
    pub filesystem_watch: bool,
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,
    #[serde(default = "default_true")]
    pub fts_enabled: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            auto_update: true,
            filesystem_watch: true,
            cache_size: 64,
            fts_enabled: true,
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

// Default value functions
fn default_music_dir() -> Utf8PathBuf {
    // Honor $XDG_MUSIC_DIR (e.g. ~/Musica) when set, else fall back to ~/Music.
    dirs::audio_dir()
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("~/Music"))
}

fn default_playlist_dir() -> Utf8PathBuf {
    dirs::config_dir()
        .map(|p| p.join("rmpd/playlists"))
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("~/.config/rmpd/playlists"))
}

fn default_db_file() -> Utf8PathBuf {
    dirs::config_dir()
        .map(|p| p.join("rmpd/database.db"))
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("~/.config/rmpd/database.db"))
}

fn default_state_file() -> Utf8PathBuf {
    dirs::config_dir()
        .map(|p| p.join("rmpd/state"))
        .and_then(|p| Utf8PathBuf::try_from(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("~/.config/rmpd/state"))
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

fn default_enabled_decoders() -> Vec<String> {
    vec!["symphonia".to_owned()]
}

fn default_cache_size() -> usize {
    64
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::find_config_file()?;
        Self::load_from_path(&config_path)
    }

    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| RmpdError::Config(format!("Failed to read config: {e}")))?;

        let mut config: Config = toml::from_str(&content)
            .map_err(|e| RmpdError::Config(format!("Failed to parse config: {e}")))?;

        config.expand_paths();
        config.ensure_directories();
        config.validate()?;
        Ok(config)
    }

    #[must_use]
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
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

    fn find_config_file() -> Result<PathBuf> {
        let candidates = [
            dirs::config_dir().map(|p| p.join("rmpd/rmpd.toml")),
            Some(PathBuf::from("/etc/rmpd/rmpd.toml")),
        ];

        for candidate in candidates.into_iter().flatten() {
            if candidate.exists() {
                return Ok(candidate);
            }
        }

        Err(RmpdError::Config("Config file not found".to_owned()))
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
    /// is intentionally left to `validate`, since it must be supplied by the
    /// user rather than created automatically.
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

    fn validate(&self) -> Result<()> {
        if !self.general.music_directory.exists() {
            return Err(RmpdError::Config(format!(
                "Music directory not found: {}",
                self.general.music_directory
            )));
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig {
                music_directory: default_music_dir(),
                playlist_directory: default_playlist_dir(),
                db_file: default_db_file(),
                state_file: default_state_file(),
                log_level: default_log_level(),
                follow_symlinks: false,
                filesystem_charset: default_charset(),
            },
            network: NetworkConfig {
                bind_address: default_bind_address(),
                port: default_port(),
                unix_socket: None,
                max_connections: default_max_connections(),
                connection_timeout: default_connection_timeout(),
                password: None,
                mpris: true,
            },
            audio: AudioConfig {
                default_output: default_output(),
                buffer_time: default_buffer_time(),
                resampler_quality: ResamplerQuality::default(),
                dop: DopMode::default(),
                device: None,
                replay_gain: ReplayGainMode::default(),
                replay_gain_preamp: 0.0,
                replay_gain_missing_preamp: 0.0,
                volume_normalization: false,
                gapless: true,
                crossfade: 0.0,
                mixramp_db: default_mixramp_db(),
                mixramp_delay: 0.0,
                restore_paused: false,
            },
            output: vec![],
            source: Vec::new(),
            decoder: DecoderConfig::default(),
            database: DatabaseConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let base = std::env::temp_dir().join(format!("rmpd-cfgtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let base = Utf8PathBuf::try_from(base).unwrap();

        // music_directory must already exist (validate requires it).
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
        assert!(c.validate().is_ok());

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
}
