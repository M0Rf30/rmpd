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
    pub decoder: DecoderConfig,
    #[serde(default)]
    pub plugins: PluginConfig,
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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    #[serde(default = "default_output")]
    pub default_output: String,
    #[serde(default = "default_buffer_time")]
    pub buffer_time: u32,
    #[serde(default)]
    pub resampler_quality: ResamplerQuality,
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

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct DecoderConfig {
    #[serde(default = "default_enabled_decoders")]
    pub enabled: Vec<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct PluginConfig {
    #[serde(default = "default_plugin_dirs")]
    pub plugin_dirs: Vec<Utf8PathBuf>,
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
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

// Default value functions
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
    -17.0
}

fn default_true() -> bool {
    true
}

fn default_enabled_decoders() -> Vec<String> {
    vec!["symphonia".to_owned()]
}

fn default_plugin_dirs() -> Vec<Utf8PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config_dir) = dirs::config_dir() {
        if let Ok(path) = Utf8PathBuf::try_from(config_dir.join("rmpd/plugins")) {
            dirs.push(path);
        }
    }
    dirs.push(Utf8PathBuf::from("/usr/lib/rmpd/plugins"));
    dirs
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
            .map_err(|e| RmpdError::Config(format!("Failed to read config: {}", e)))?;

        let mut config: Config = toml::from_str(&content)
            .map_err(|e| RmpdError::Config(format!("Failed to parse config: {}", e)))?;

        config.expand_paths();
        config.validate()?;
        Ok(config)
    }

    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
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
        // Helper function to expand tilde
        fn expand_tilde(path: &Utf8PathBuf) -> Utf8PathBuf {
            let path_str = path.as_str();
            if path_str.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    if let Some(home_str) = home.to_str() {
                        return Utf8PathBuf::from(path_str.replacen("~", home_str, 1));
                    }
                }
            }
            path.clone()
        }

        self.general.music_directory = expand_tilde(&self.general.music_directory);
        self.general.playlist_directory = expand_tilde(&self.general.playlist_directory);
        self.general.db_file = expand_tilde(&self.general.db_file);
        self.general.state_file = expand_tilde(&self.general.state_file);
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
                music_directory: Utf8PathBuf::from("~/Music"),
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
            },
            audio: AudioConfig {
                default_output: default_output(),
                buffer_time: default_buffer_time(),
                resampler_quality: ResamplerQuality::default(),
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
            decoder: DecoderConfig::default(),
            plugins: PluginConfig::default(),
            database: DatabaseConfig::default(),
        }
    }
}
