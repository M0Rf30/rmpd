use anyhow::{Result, anyhow};
use clap::Parser;
use rmpd_core::config::{Config, ConfigSource, DiagLevel, DiscoverOptions};
use tracing::{info, warn};

mod app;

/// Daemonize the process using double-fork + setsid.
#[cfg(unix)]
#[allow(clippy::disallowed_methods)] // process::exit is required by the double-fork daemonize pattern
fn daemonize() -> Result<()> {
    use nix::unistd::{ForkResult, fork, setsid};

    // First fork — parent exits so the shell thinks the command is done.
    match unsafe { fork()? } {
        ForkResult::Parent { .. } => std::process::exit(0),
        ForkResult::Child => {}
    }

    // Become session leader, detach from controlling terminal.
    setsid()?;

    // Second fork — ensures we can never re-acquire a controlling terminal.
    match unsafe { fork()? } {
        ForkResult::Parent { .. } => std::process::exit(0),
        ForkResult::Child => {}
    }

    // Redirect stdin / stdout / stderr to /dev/null.
    let devnull = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    nix::unistd::dup2_stdin(&devnull)?;
    nix::unistd::dup2_stdout(&devnull)?;
    nix::unistd::dup2_stderr(&devnull)?;

    // Change to root to avoid holding a mount point.
    std::env::set_current_dir("/")?;

    Ok(())
}

#[derive(Parser, Debug)]
#[command(author, version, about = "rmpd - Rust Music Player Daemon", long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<String>,

    /// Bind address
    #[arg(short, long)]
    bind: Option<String>,

    /// Port number
    #[arg(short, long)]
    port: Option<u16>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Run as a background daemon
    #[arg(short = 'd', long)]
    daemonize: bool,

    /// Log to syslog/journald instead of stdout (useful when running as a daemon)
    #[arg(long)]
    syslog: bool,

    /// Skip configuration file discovery entirely and use built-in defaults
    #[arg(long)]
    no_config: bool,

    /// Write a starter configuration file to the default search location and exit
    #[arg(long)]
    generate_config: bool,

    /// Print the configuration file path that would be used and exit
    #[arg(long)]
    print_config_path: bool,
}

fn make_bind_addr(addr: &str, port: u16) -> String {
    // IPv6 bare addresses (contain ':' but aren't already bracketed) need wrapping
    if addr.contains(':') && !addr.starts_with('[') {
        format!("[{addr}]:{port}")
    } else {
        format!("{addr}:{port}")
    }
}

/// Build the tracing filter. Honors `RUST_LOG` when set; otherwise applies
/// `level` to rmpd's own crates while pinning noisy third-party crates down so
/// the default (non-debug) output stays readable.
fn default_env_filter(level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!(
            "{level},\
             lofty=error,\
             symphonia=error,symphonia_core=error,symphonia_bundle_mp3=error,\
             symphonia_format_isomp4=error,symphonia_format_ogg=error,\
             symphonia_codec_vorbis=error,symphonia_metadata=error,\
             cpal=warn,zbus=warn"
        ))
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.generate_config {
        let search_paths = Config::search_paths();
        let path = &search_paths[0];
        return match Config::write_template(path.as_std_path()) {
            Ok(()) => {
                println!("{path}");
                Ok(())
            }
            Err(e) => Err(anyhow!(e)),
        };
    }

    if args.print_config_path {
        let path = Config::search_paths()
            .into_iter()
            .find(|p| p.exists())
            .unwrap_or_else(|| Config::search_paths()[0].clone());
        println!("{path}");
        return Ok(());
    }

    // Load configuration before any logging is set up, so the effective log
    // level (from config or --verbose) can drive the tracing filter from the
    // very first line of output.
    let discover_opts = DiscoverOptions {
        generate_if_missing: !args.no_config,
        no_config: args.no_config,
    };
    // anyhow prints the error to stderr on exit, which is the only channel
    // available here: the tracing subscriber is not up yet, by design.
    let load = Config::discover(
        args.config.as_ref().map(std::path::Path::new),
        discover_opts,
    )?;
    let config = load.config;

    // Initialize logging
    let log_level = if args.verbose {
        "debug".to_owned()
    } else {
        config.general.log_level.clone()
    };
    if args.syslog || args.daemonize {
        #[cfg(target_os = "linux")]
        {
            use tracing_subscriber::prelude::*;
            let env_filter = default_env_filter(&log_level);
            match tracing_journald::layer() {
                Ok(journald) => {
                    tracing_subscriber::registry()
                        .with(env_filter)
                        .with(journald)
                        .init();
                }
                Err(e) => {
                    eprintln!("warning: journald unavailable ({e}), logging to stderr");
                    tracing_subscriber::fmt()
                        .with_ansi(false)
                        .with_writer(std::io::stderr)
                        .with_env_filter(env_filter)
                        .init();
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .with_env_filter(default_env_filter(&log_level))
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(default_env_filter(&log_level))
            .init();
    }

    info!("starting rmpd v{}", env!("CARGO_PKG_VERSION"));

    // Replay diagnostics collected during config discovery now that the
    // subscriber is up; discover() never logs directly so nothing is lost.
    for d in &load.diagnostics {
        match d.level {
            DiagLevel::Warn => warn!("{}", d.message),
            DiagLevel::Info => info!("{}", d.message),
        }
    }

    match &load.source {
        ConfigSource::File(p) => info!("configuration loaded from {p}"),
        ConfigSource::Generated(p) => {
            info!("no configuration file found; wrote a starter config to {p}")
        }
        ConfigSource::Defaults => {
            warn!("no configuration file in use; running with built-in defaults")
        }
    }

    // Override with CLI arguments
    let bind_address = args
        .bind
        .unwrap_or_else(|| config.network.bind_address.clone());
    let port = args.port.unwrap_or(config.network.port);

    let full_address = make_bind_addr(&bind_address, port);

    info!("music directory: {}", config.general.music_directory);
    info!("database: {}", config.general.db_file);

    if args.daemonize {
        daemonize()?;
    }

    // Start the server
    app::run(full_address, config).await?;

    Ok(())
}
