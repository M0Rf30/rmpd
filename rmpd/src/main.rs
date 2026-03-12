use anyhow::Result;
use clap::Parser;
use daemonize;
use tracing::info;

mod app;

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
}

fn make_bind_addr(addr: &str, port: u16) -> String {
    // IPv6 bare addresses (contain ':' but aren't already bracketed) need wrapping
    if addr.contains(':') && !addr.starts_with('[') {
        format!("[{addr}]:{port}")
    } else {
        format!("{addr}:{port}")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose { "debug" } else { "info" };
    if args.syslog || args.daemonize {
        #[cfg(target_os = "linux")]
        {
            use tracing_subscriber::prelude::*;
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level));
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
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
            )
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
            )
            .init();
    }

    info!("starting rmpd v{}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = if let Some(config_path) = args.config {
        rmpd_core::config::Config::load_from_path(config_path)?
    } else {
        rmpd_core::config::Config::load_or_default()
    };

    // Override with CLI arguments
    let bind_address = args
        .bind
        .unwrap_or_else(|| config.network.bind_address.clone());
    let port = args.port.unwrap_or(config.network.port);

    let full_address = make_bind_addr(&bind_address, port);

    info!("configuration loaded");
    info!("music directory: {}", config.general.music_directory);
    info!("database: {}", config.general.db_file);

    if args.daemonize {
        daemonize::Daemonize::new()
            .start()
            .map_err(|e| anyhow::anyhow!("Failed to daemonize: {e}"))?;
    }

    // Start the server
    app::run(full_address, config).await?;

    Ok(())
}
