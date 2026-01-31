use anyhow::Result;
use clap::Parser;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    info!("Starting rmpd v{}", env!("CARGO_PKG_VERSION"));

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

    let full_address = format!("{bind_address}:{port}");

    info!("Configuration loaded");
    info!("Music directory: {}", config.general.music_directory);
    info!("Database: {}", config.general.db_file);

    // Start the server
    app::run(full_address, config).await?;

    Ok(())
}
