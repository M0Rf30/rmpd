// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

use anyhow::{Result, anyhow};
use clap::{ArgAction, Parser, Subcommand};
use rmpd_core::config::{Config, ConfigSource, DiagLevel, DiscoverOptions};
use std::path::PathBuf;
use tracing::{info, warn};

mod app;
mod cli;
mod systemd;

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
    /// Configuration file(s). Repeatable or `a.toml:b.toml`; a directory
    /// loads every `*.toml` inside. Later files override earlier ones.
    #[arg(short, long, action = ArgAction::Append)]
    config: Vec<PathBuf>,

    /// Override a config value, e.g. `-o network.port=6601` (repeatable)
    #[arg(short = 'o', long = "option", value_name = "SECTION.KEY=VALUE")]
    option: Vec<String>,

    /// Bind address
    #[arg(short, long)]
    bind: Option<String>,

    /// Port number
    #[arg(short, long)]
    port: Option<u16>,

    /// More logging: -v debug, -vv trace
    #[arg(short, long, action = ArgAction::Count, conflicts_with = "quiet")]
    verbose: u8,

    /// Only log warnings and errors
    #[arg(short, long)]
    quiet: bool,

    /// Run as a background daemon
    #[arg(short = 'd', long)]
    daemonize: bool,

    /// Log to syslog/journald instead of stdout (useful when running as a daemon)
    #[arg(long)]
    syslog: bool,

    /// Log to stdout, ignoring `log_file`
    #[arg(long, conflicts_with_all = ["stderr", "syslog"])]
    stdout: bool,

    /// Log to stderr, ignoring `log_file`
    #[arg(long, conflicts_with = "syslog")]
    stderr: bool,

    /// Ask the running rmpd instance to shut down, then exit
    #[arg(long)]
    kill: bool,

    /// Skip configuration file discovery entirely and use built-in defaults
    #[arg(long)]
    no_config: bool,

    /// Write a starter configuration file to the default search location and exit
    #[arg(long)]
    generate_config: bool,

    /// Print the configuration file path that would be used and exit
    #[arg(long)]
    print_config_path: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the effective configuration (after files and -o), secrets masked
    Config,
    /// Print version, enabled features and compiled-in plugins
    Deps,
}

/// Build the tracing filter. Honors `RUST_LOG` when set; otherwise applies
/// `level` to rmpd's own crates while pinning noisy third-party crates down so
/// the default (non-debug) output stays readable.
fn default_env_filter(level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!(
            "{level},\
             symphonia=error,symphonia_core=error,symphonia_bundle_mp3=error,\
             symphonia_format_isomp4=error,symphonia_format_ogg=error,\
             symphonia_codec_vorbis=error,symphonia_metadata=error,\
             cpal=warn,zbus=warn"
        ))
    })
}

fn main() -> Result<()> {
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

    if matches!(args.command, Some(Command::Deps)) {
        cli::print_deps();
        return Ok(());
    }

    // One-shot actions read the config but never write a starter file.
    let one_shot = args.kill || args.command.is_some();

    // Load configuration before any logging is set up, so the effective log
    // level (from config or --verbose) can drive the tracing filter from the
    // very first line of output.
    let discover_opts = DiscoverOptions {
        generate_if_missing: !args.no_config && !one_shot,
        no_config: args.no_config,
    };
    // anyhow prints the error to stderr on exit, which is the only channel
    // available here: the tracing subscriber is not up yet, by design.
    let load = Config::discover_layered(&args.config, &args.option, discover_opts)?;

    if matches!(args.command, Some(Command::Config)) {
        for d in &load.diagnostics {
            if d.level == DiagLevel::Warn {
                eprintln!("warning: {}", d.message);
            }
        }
        print!("{}", load.config.to_masked_toml()?);
        return Ok(());
    }
    if args.kill {
        return cli::kill_running(&load.config.network);
    }
    let config = load.config;

    // Initialize logging
    let log_level = if args.quiet {
        "warn".to_owned()
    } else {
        match args.verbose {
            0 => config.general.log_level.clone(),
            1 => "debug".to_owned(),
            _ => "trace".to_owned(),
        }
    };

    // Open the configured log file up front, if any, so every branch below
    // that doesn't use journald can write to it instead of stdout/stderr.
    // Opening failure falls back to the default destination rather than
    // aborting startup — losing the preferred log destination is not worth
    // refusing to start the daemon over. Log rotation on SIGHUP is not
    // implemented here (mpd's LogInit.cxx reopens the log file on SIGHUP);
    // the file is opened once and kept for the process lifetime.
    let log_file_writer = (!args.stdout && !args.stderr)
        .then_some(config.general.log_file.as_ref())
        .flatten()
        .and_then(|path| {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path.as_std_path())
            {
                Ok(file) => Some(file),
                Err(e) => {
                    eprintln!("warning: unable to open log file {path} ({e}), logging to stdout");
                    None
                }
            }
        });

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
                    match log_file_writer {
                        Some(file) => tracing_subscriber::fmt()
                            .with_ansi(false)
                            .with_writer(std::sync::Mutex::new(file))
                            .with_env_filter(env_filter)
                            .init(),
                        None => tracing_subscriber::fmt()
                            .with_ansi(false)
                            .with_writer(std::io::stderr)
                            .with_env_filter(env_filter)
                            .init(),
                    }
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let env_filter = default_env_filter(&log_level);
            match log_file_writer {
                Some(file) => tracing_subscriber::fmt()
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_env_filter(env_filter)
                    .init(),
                None => tracing_subscriber::fmt()
                    .with_ansi(false)
                    .with_writer(std::io::stderr)
                    .with_env_filter(env_filter)
                    .init(),
            }
        }
    } else {
        let env_filter = default_env_filter(&log_level);
        match log_file_writer {
            Some(file) => tracing_subscriber::fmt()
                .with_writer(std::sync::Mutex::new(file))
                .with_env_filter(env_filter)
                .init(),
            None if args.stderr => tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(env_filter)
                .init(),
            None => tracing_subscriber::fmt().with_env_filter(env_filter).init(),
        }
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
    let cli_listen_override = args.bind.is_some() || args.port.is_some();
    let bind_address = args
        .bind
        .unwrap_or_else(|| config.network.bind_address.clone());
    let port = args.port.unwrap_or(config.network.port);

    // A socket path or an empty address ("no TCP") passes through untouched;
    // appending a port would make it unboundable.
    let full_address = rmpd_protocol::server::resolve_bind_address(&bind_address, port);

    info!("music directory: {}", config.general.music_directory);
    info!("database: {}", config.general.db_file);

    // systemd integration (see systemd.rs). Take any socket-activation fds
    // now: this clears LISTEN_* from the environment, which is only sound
    // while the process is still single-threaded, and LISTEN_PID is only
    // valid for this exact PID, so it must happen before any fork.
    let activated = systemd::take_activated_listeners()
        .map_err(|e| anyhow!("systemd socket activation failed: {e}"))?;
    if activated.is_some() {
        info!("socket activation: serving sockets passed by systemd");
        if cli_listen_override {
            warn!("--bind/--port are ignored: listening on the sockets passed by systemd");
        }
    }

    // Daemonize (double-fork) BEFORE the tokio runtime is built: forking a
    // live multi-threaded runtime loses every worker/reactor thread except
    // the calling one in the child, which then hangs or corrupts state the
    // instant it touches an async primitive.
    //
    // Never daemonize under a supervisor that tracks the main PID:
    // `Type=notify` only accepts READY=1 from that PID (the forked child
    // would be rejected and the unit would time out), and LISTEN_PID only
    // matches the original process. Foreground is what systemd wants anyway.
    if args.daemonize {
        if activated.is_some() || systemd::notify_socket_present() {
            warn!(
                "--daemonize ignored: running under systemd (NOTIFY_SOCKET/socket activation); \
                 use Type=notify without --daemonize"
            );
        } else {
            daemonize()?;
        }
    }

    // macOS desktop integration needs the process main thread for AppKit, so
    // the server moves to a background thread there; everywhere else the
    // runtime drives it directly.
    #[cfg(target_os = "macos")]
    let want_media_controls = config.network.media_controls && !args.daemonize;

    #[cfg(target_os = "macos")]
    if !want_media_controls {
        if config.network.media_controls {
            info!("media controls unavailable: --daemonize leaves no AppKit session");
        } else {
            info!("media controls disabled: set media_controls = true to enable Now Playing");
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    #[cfg(target_os = "macos")]
    if want_media_controls {
        use rmpd_protocol::media_controls_macos;

        let (state_tx, state_rx) = std::sync::mpsc::channel();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let handle = runtime.handle().clone();
        let addr = full_address.clone();
        std::thread::Builder::new()
            .name("rmpd-server".into())
            .spawn(move || {
                if let Err(e) = handle.block_on(app::run(addr, config, Some(state_tx), activated)) {
                    eprintln!("rmpd: server error: {e}");
                    // Fatal startup failure: the AppKit loop on the main thread
                    // would keep the process alive, so exit here. Same exemption
                    // the daemonize path above uses.
                    #[allow(clippy::disallowed_methods)]
                    std::process::exit(1);
                }
                // Clean shutdown: let the AppKit run loop terminate.
                let _ = exit_tx.send(());
            })?;

        media_controls_macos::run_blocking(state_rx, exit_rx, runtime.handle().clone());
        return Ok(());
    }

    runtime.block_on(app::run(full_address, config, None, activated))?;
    Ok(())
}
