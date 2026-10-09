// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::systemd::{self, Activated};
use rmpd_core::config::Config;
use rmpd_core::error::Result;
use rmpd_core::state::PlayerState;
use rmpd_protocol::{AppState, MpdServer, StateFile};
use std::sync::Arc;
use tokio::signal;
use tracing::{error, info, warn};

/// Run the daemon. When `activated` carries sockets inherited from systemd
/// (socket activation), they are served instead of binding `bind_address` and
/// `network.unix_socket` — like MPD, which skips its own listeners whenever
/// activation fds exist.
///
/// `state_tx` hands the fully-built [`AppState`] to the macOS Now Playing
/// controller, which owns the process main thread; every other platform passes
/// `None`.
pub async fn run(
    bind_address: String,
    config: Config,
    state_tx: Option<std::sync::mpsc::Sender<rmpd_protocol::state::AppState>>,
    activated: Option<Activated>,
) -> Result<()> {
    // Create application state with database and music directory paths
    let db_path = config.general.db_file.to_string();
    let music_dir = config.general.music_directory.to_string();
    let state_file_path = config.general.state_file.to_string();
    let playlist_dir = config.general.playlist_directory.to_string();

    let mut state = AppState::with_all_paths(db_path.clone(), music_dir.clone(), playlist_dir);

    // Configure password authentication if set in config.

    // Build music-source registry from [[source]] config blocks.
    let source_registry = Arc::new(rmpd_source::SourceRegistry::from_config(&config.source));
    state.set_sources(source_registry);
    state.set_password(config.network.password.clone());
    state.set_passwords(config.network.passwords.clone());
    state.set_permission_rules(
        config.network.default_permissions.clone(),
        config.network.local_permissions.clone(),
        config.network.host_permissions.clone(),
    );
    state.set_max_command_list_size(config.network.max_command_list_size);
    state.set_max_output_buffer_size(config.network.max_output_buffer_size);
    state.set_max_playlist_length(config.general.max_playlist_length as u32);
    state.set_zeroconf_name(config.network.zeroconf_name.clone());
    state.set_symlink_policy(
        config.general.follow_inside_symlinks,
        config.general.follow_outside_symlinks,
    );
    state.set_playlist_options(
        config.playlist.embedded_cue_as_directory,
        config.database.hide_playlist_targets,
    );
    if !config
        .general
        .filesystem_charset
        .eq_ignore_ascii_case("UTF-8")
        && !config
            .general
            .filesystem_charset
            .eq_ignore_ascii_case("UTF8")
    {
        warn!(
            "filesystem_charset = {:?} is not supported; rmpd operates in UTF-8 only and ignores it",
            config.general.filesystem_charset
        );
    }

    // Apply audio settings from config to the player.
    // - resampler quality: used only when the device can't play a rate natively.
    // - DoP mode: native DSD-over-PCM policy for DSD sources.
    // - output device: select a specific (e.g. raw ALSA `hw:`) device, bypassing
    //   PipeWire/PulseAudio for bit-perfect DoP. Env vars still override.
    {
        let mut engine = state.engine.write().await;
        engine.set_resampler_quality(config.audio.resampler_quality);
        engine.set_dop_mode(config.dop_mode());
        engine.set_replay_gain(
            config.audio.replay_gain,
            config.audio.replay_gain_preamp,
            config.audio.replay_gain_missing_preamp,
        );
        engine.set_volume_normalization(config.audio.volume_normalization);
        engine.set_crossfade(config.audio.crossfade as u32);
        engine.set_mixramp(config.audio.mixramp_db, config.audio.mixramp_delay);
        engine.set_buffer_time(config.audio.buffer_time);
        engine.set_outputs({
            let enabled: Vec<rmpd_core::config::OutputConfig> = config
                .output
                .iter()
                .filter(|o| o.enabled)
                .cloned()
                .collect();
            if enabled.is_empty() {
                vec![rmpd_core::config::OutputConfig::cpal_default()]
            } else {
                enabled
            }
        });
    }
    rmpd_player::set_output_device(config.output_device());

    // Build the protocol-visible output list from the [[output]] config blocks
    // so `outputs`/`enableoutput`/`disableoutput` report the real configuration.
    state
        .set_outputs_from_config(&config.output, &config.audio.default_output)
        .await;

    // Load state from file if it exists. A failure here (corrupt file, or
    // the blocking load task itself panicking) must not prevent the daemon
    // from starting; it just means playback state is not restored. The three
    // outcomes are handled separately so a panicking load can never
    // masquerade as "no saved state was present".
    let state_file = StateFile::new(state_file_path.clone());
    match tokio::task::spawn_blocking(move || state_file.load()).await {
        Ok(Ok(Some(saved_state))) => {
            info!("restoring state from file");
            restore_state(
                &state,
                saved_state,
                &db_path,
                &music_dir,
                config.audio.restore_paused,
            )
            .await;
        }
        Ok(Ok(None)) => {}
        Ok(Err(e)) => {
            error!("failed to load state file: {}", e);
        }
        Err(e) => {
            error!("state restoration skipped: load task failed: {}", e);
        }
    }

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);

    // Set shutdown sender in state for kill command
    state.set_shutdown_sender(shutdown_tx.clone());

    // Shared handle for periodic + shutdown state saves. StateFile::save
    // remembers its own last-written content and serializes writes behind
    // an internal lock, so the ticker below and the shutdown paths can
    // safely share this one instance (see StateFile::save in
    // rmpd-protocol/src/statefile.rs).
    let state_file = Arc::new(StateFile::new(state_file_path.clone()));

    // Kept so the final save (after the server loop) can force any
    // in-flight ticker to observe shutdown before it saves, even on
    // shutdown paths that never go through the signal handler below (e.g.
    // the `kill` command).
    let final_shutdown_tx = shutdown_tx.clone();

    // Periodic state save (mpd src/StateFile.cxx ticks every
    // state_file_interval seconds, default 120, so a crash/OOM-kill/power
    // loss between clean shutdowns loses at most one interval's worth of
    // queue contents, position, volume and options). `0` disables it,
    // matching mpd.conf's documented "never" for this setting.
    let state_save_ticker = if config.general.state_file_interval > 0 {
        let interval_secs = config.general.state_file_interval;
        let ticker_state = state.clone();
        let ticker_state_file = state_file.clone();
        let mut ticker_shutdown_rx = shutdown_tx.subscribe();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            interval.tick().await; // first tick fires immediately; nothing to save yet
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        save_state(&ticker_state, &ticker_state_file).await;
                    }
                    _ = ticker_shutdown_rx.recv() => {
                        // Stop as soon as shutdown starts. Joining this
                        // handle below (before the final save) then makes it
                        // impossible for a periodic write to land after the
                        // definitive shutdown save and resurrect stale state.
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    // Expose rmpd on the session D-Bus via MPRIS so desktop environments,
    // `playerctl`, and media keys can discover and control it. Kept alive
    // (`_mpris`) for the lifetime of the server; dropping it releases the
    // D-Bus name. Failure (e.g. no session bus) is non-fatal.
    // Linux desktop integration. macOS uses the native Now Playing stack,
    // which has to run on the process main thread (see media_controls_macos),
    // so it is started from main.rs instead.
    #[cfg(not(target_os = "macos"))]
    let _media_integration = if config.network.media_controls {
        match rmpd_protocol::mpris::spawn(state.clone()).await {
            Ok(handle) => {
                info!("MPRIS interface enabled (org.mpris.MediaPlayer2.rmpd)");
                Some(handle)
            }
            Err(e) => {
                warn!("MPRIS interface disabled: {}", e);
                None
            }
        }
    } else {
        None
    };

    // A missing music_directory is not fatal (config warns about it), but the
    // scanner and the watcher both need a real directory. Skipping them here is
    // what makes that warning honest: without this the daemon would immediately
    // log a scan failure and a watch failure for a path we already reported.
    let music_dir_exists = std::path::Path::new(&music_dir).is_dir();

    // Trigger an initial library scan on startup when auto-update is enabled.
    if config.database.auto_update {
        if music_dir_exists {
            info!("auto-update enabled: scanning music directory");
            state.spawn_library_update(false).await;
        } else {
            warn!("skipping library scan: music directory {music_dir} does not exist");
        }
    }

    // Sync enabled music sources (ping first; unreachable sources are skipped).
    if !state.sources.is_empty() {
        info!("syncing music source catalogs");
        state.spawn_source_sync();
    }

    // Start enabled `[[integration]]` plugins (scrobblers, notifiers, ...).
    let (integration_shutdown, integration_signal) = rmpd_plugin::shutdown_channel();
    let integration_handles = if config.integration.iter().any(|c| c.enabled) {
        let player: Arc<dyn rmpd_plugin::PlayerHandle> =
            Arc::new(rmpd_protocol::ServerPlayerHandle::new(state.clone()));
        let integration_dir = std::path::Path::new(&state_file_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("integrations");
        rmpd_integrations::spawn_integrations(
            &config.integration,
            &state.event_bus,
            &player,
            &integration_dir,
            &integration_signal,
        )
    } else {
        Vec::new()
    };

    // Start the filesystem watcher so the database stays in sync with on-disk
    // changes. Kept alive (`_watcher`) for the lifetime of the server; dropping
    // it would stop watching.
    let _watcher = if config.database.filesystem_watch && music_dir_exists {
        match start_filesystem_watch(
            &state,
            &db_path,
            &music_dir,
            config.database.auto_update_depth,
        )
        .await
        {
            Ok(w) => Some(w),
            Err(e) => {
                warn!("filesystem watch disabled: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Clone state for shutdown handler
    let shutdown_state = state.clone();
    let shutdown_state_file = state_file.clone();

    // Spawn task to handle shutdown signals. Listens for both SIGINT and
    // SIGTERM so state is saved regardless of which signal a supervisor
    // sends (systemd's default stop signal is SIGTERM, not SIGINT) — see
    // mpd src/unix/SignalHandlers.cxx, which handles both.
    let _shutdown_handler = tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut sigterm = match signal::unix::signal(signal::unix::SignalKind::terminate()) {
                Ok(s) => Some(s),
                Err(err) => {
                    error!("unable to register SIGTERM handler: {}", err);
                    None
                }
            };
            let sig = match sigterm.as_mut() {
                Some(sigterm) => {
                    tokio::select! {
                        result = signal::ctrl_c() => result.map(|()| "SIGINT"),
                        _ = sigterm.recv() => Ok("SIGTERM"),
                    }
                }
                None => signal::ctrl_c().await.map(|()| "SIGINT"),
            };
            match sig {
                Ok(sig) => {
                    info!("received {}, saving state", sig);
                    systemd::notify_stopping();
                    save_state(&shutdown_state, &shutdown_state_file).await;
                    let _ = shutdown_tx.send(());
                }
                Err(err) => {
                    error!("unable to listen for shutdown signal: {}", err);
                }
            }
        }
        #[cfg(not(unix))]
        {
            match signal::ctrl_c().await {
                Ok(()) => {
                    info!("received SIGINT, saving state");
                    systemd::notify_stopping();
                    save_state(&shutdown_state, &shutdown_state_file).await;
                    let _ = shutdown_tx.send(());
                }
                Err(err) => {
                    error!("unable to listen for shutdown signal: {}", err);
                }
            }
        }
    });

    // macOS: pause when the device we were playing through DISAPPEARS from
    // the system's output-device list — headphone power-off reroutes to
    // speakers silently and macOS keeps playing. A short poll of cpal detects
    // the vanish without unsafe CoreAudio FFI. Deliberate switching
    // (speakers <-> headphones) never pauses: the old device stays listed.
    // Only when rmpd follows the system default: with a pinned device, the
    // watcher would pause on the loss of a device this daemon is not using.
    #[cfg(target_os = "macos")]
    if config.audio.pause_on_device_loss && config.audio.device.is_none() {
        let device_watch_state = state.clone();
        tokio::spawn(async move {
            use rmpd_core::state::PlayerState;
            use rmpd_player::cpal_utils;

            // Cleared whenever playback stops: the device a pause resumes on is
            // not necessarily the one it paused on.
            let mut active_device: Option<String> = None;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;

                // Skip the CoreAudio queries entirely unless actively playing.
                let playing = matches!(
                    PlayerState::from_atomic(
                        device_watch_state
                            .atomic_state
                            .load(std::sync::atomic::Ordering::Acquire)
                    ),
                    PlayerState::Play
                );
                if !playing {
                    active_device = None;
                    continue;
                }

                let previous = active_device.as_deref();
                let current = cpal_utils::default_output_name();

                // Ask for the device list only when the default moved away from
                // the device that was playing.
                if let Some(prev) = previous
                    && Some(prev) != current.as_deref()
                {
                    let available = cpal_utils::output_device_names();
                    if let Some(lost) = lost_device(previous, current.as_deref(), &available) {
                        info!("output device {lost:?} disappeared; pausing");
                        let _ = rmpd_protocol::commands::playback::handle_pause_command(
                            &device_watch_state,
                            Some(true),
                        )
                        .await;
                    }
                }

                active_device = current;
            }
        });
    }

    // Create and run server
    // Hand the fully-built state to the macOS Now Playing controller, which
    // owns the process main thread; a no-op elsewhere.
    if let Some(tx) = &state_tx {
        let _ = tx.send(state.clone());
    }

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = MpdServer::with_state(bind_address.clone(), state.clone(), shutdown_rx)
        .with_ready_signal(ready_tx);
    let server =
        server.with_unix_socket(config.network.unix_socket.as_ref().map(|p| p.to_string()));
    let server = server
        .with_max_connections(config.network.max_connections)
        .with_connection_timeout(std::time::Duration::from_secs(
            config.network.connection_timeout,
        ));

    // systemd `Type=notify`: report readiness once every listener is bound
    // and the accept loop is about to run (MPD sends READY=1 after startup).
    // The sender is dropped unfired if the server fails first, so a failed
    // start never reports ready. A no-op without $NOTIFY_SOCKET.
    tokio::spawn(async move {
        if ready_rx.await.is_ok() {
            systemd::notify_ready();
        }
    });

    let server_result = match activated {
        Some(activated) => {
            // Socket activation: serve the inherited sockets and bind nothing.
            let mut tcp = Vec::new();
            for l in activated.tcp {
                let l = tokio::net::TcpListener::from_std(l)?;
                match l.local_addr() {
                    Ok(addr) => info!("mpd server listening on {} (socket activation)", addr),
                    Err(_) => info!("mpd server listening on inherited TCP socket"),
                }
                tcp.push(l);
            }
            let mut unix = Vec::new();
            for l in activated.unix {
                let l = tokio::net::UnixListener::from_std(l)?;
                info!("mpd server listening on inherited unix socket (socket activation)");
                unix.push(l);
            }

            // Advertise the port systemd actually bound, and only when there
            // is a TCP listener to advertise.
            if config.network.zeroconf_enabled
                && let Some(port) = tcp
                    .iter()
                    .find_map(|l| l.local_addr().ok())
                    .map(|a| a.port())
            {
                state.advertise_mdns(port);
            }

            server.run_with_listeners(tcp, unix).await
        }
        None => {
            if let Some(sock) = &config.network.unix_socket {
                info!("unix socket: {}", sock);
            }

            // MPD's `bind_to_address` rule: a path names a socket, and naming
            // only a socket serves no TCP at all. An empty address says the same
            // thing, taking its path from `unix_socket`.
            match rmpd_protocol::server::socket_only_path(
                &bind_address,
                config.network.unix_socket.as_ref().map(|p| p.as_str()),
            )? {
                Some(path) => {
                    info!("TCP listener disabled; serving on unix socket {path} only");
                    // Evaluated as the arm's value: the shutdown cleanup below
                    // still runs when the accept loop ends.
                    server.run_unix_socket(path).await
                }
                None => {
                    let listener = tokio::net::TcpListener::bind(&bind_address).await?;
                    info!("mpd server listening on {}", bind_address);

                    // Advertise rmpd via mDNS only once the TCP listener is actually
                    // accepting connections, and only when zeroconf is enabled. Use
                    // the port actually bound (`--port` overrides the config value).
                    if config.network.zeroconf_enabled {
                        let port = listener
                            .local_addr()
                            .map_or(config.network.port, |a| a.port());
                        state.advertise_mdns(port);
                    }

                    server.run_with_listener(listener).await
                }
            }
        }
    };

    // Under systemd the service is stopping from here on (covers shutdown
    // paths that bypass the signal handler, e.g. the `kill` command).
    systemd::notify_stopping();

    // Stop the periodic ticker before the final save. This send is a no-op
    // if the signal handler above already sent it; it exists as a fallback
    // for shutdown paths that never go through that handler (e.g. the
    // `kill` command, or the listener loop returning on its own). Joining
    // the ticker task guarantees it has fully stopped — not just been
    // asked to — before the save below runs, so it cannot race a stale
    // write after the definitive shutdown save completes.
    let _ = final_shutdown_tx.send(());
    if let Some(ticker) = state_save_ticker {
        let _ = ticker.await;
    }

    // Integrations were signalled via `integration_shutdown`; give them a
    // moment to wind down so they can flush their own state.
    integration_shutdown.trigger();
    for handle in integration_handles {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    // Save state on clean shutdown
    info!("server stopped, saving state");
    save_state(&state, &state_file).await;

    server_result?;
    Ok(())
}

/// Whether the device rmpd was playing through has disappeared from the device
/// list, returning its name. A move between two listed devices (speakers and
/// headphones) is not a loss.
#[cfg(target_os = "macos")]
fn lost_device(
    previous: Option<&str>,
    current: Option<&str>,
    available: &[String],
) -> Option<String> {
    let previous = previous?;
    if current == Some(previous) {
        return None;
    }
    (!available.iter().any(|name| name == previous)).then(|| previous.to_owned())
}

/// Open a dedicated database handle and start watching the music directory for
/// changes, returning the live watcher (which must be kept alive to keep
/// watching).
async fn start_filesystem_watch(
    state: &AppState,
    db_path: &str,
    music_dir: &str,
    auto_update_depth: Option<u32>,
) -> Result<rmpd_library::FilesystemWatcher> {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let db = rmpd_library::Database::open(db_path)?;
    let mut watcher = rmpd_library::FilesystemWatcher::new(
        std::path::PathBuf::from(music_dir),
        Arc::new(Mutex::new(db)),
        state.event_bus.clone(),
    )?;
    watcher.set_max_depth(auto_update_depth);
    watcher.set_embedded_cue_as_directory(state.embedded_cue_as_directory);
    watcher.start().await?;
    info!("filesystem watcher started for {}", music_dir);
    Ok(watcher)
}

async fn restore_state(
    state: &AppState,
    saved_state: rmpd_protocol::statefile::SavedState,
    db_path: &str,
    music_dir: &str,
    restore_paused: bool,
) {
    // Restore playback options
    {
        let mut status = state.status.write().await;
        status.volume = saved_state.volume;
        status.random = saved_state.random;
        status.repeat = saved_state.repeat;
        status.single = saved_state.single;
        status.consume = saved_state.consume;
        status.crossfade = saved_state.crossfade;
        status.mixramp_db = saved_state.mixramp_db;
        status.mixramp_delay = saved_state.mixramp_delay;
        status.replay_gain_mode = saved_state.replay_gain_mode;
    }

    // Keep the engine's crossfade + MixRamp settings in sync with restored state.
    // `random` matters to the engine too: ReplayGain `auto` picks track gain when
    // random is on, album gain when it is off (mpd ReplayGainMode).
    {
        let mut engine = state.engine.write().await;
        engine.set_crossfade(saved_state.crossfade);
        engine.set_mixramp(saved_state.mixramp_db, saved_state.mixramp_delay);
        engine.set_random(saved_state.random);
    }

    // Restore per-output enabled state, then point the engine at the first
    // still-enabled output.
    if !saved_state.disabled_outputs.is_empty() {
        let mut outputs = state.outputs.write().await;
        for out in outputs.iter_mut() {
            if saved_state.disabled_outputs.iter().any(|n| n == &out.name) {
                out.enabled = false;
            }
        }
    }
    {
        let enabled: Vec<rmpd_core::config::OutputConfig> = {
            let outputs = state.outputs.read().await;
            outputs
                .iter()
                .filter(|o| o.enabled)
                .filter_map(|o| o.config.clone())
                .collect()
        };
        if !enabled.is_empty() {
            state.engine.write().await.set_outputs(enabled);
        }
    }

    // Restore the last-loaded-playlist name unconditionally (mirrors MPD's
    // PlaylistState.cxx, which sets it directly from the state file and
    // doesn't depend on whether any songs are also being restored).
    if !saved_state.last_loaded_playlist.is_empty() {
        state
            .queue
            .write()
            .await
            .set_last_loaded_playlist(saved_state.last_loaded_playlist.clone());
    }

    // Restore playlist
    // The saved current position indexes the ORIGINAL playlist; songs missing
    // from the DB are skipped below, so this is shifted left as we go to keep
    // pointing at the right song.
    let mut resume_position = saved_state.current_position;

    if !saved_state.playlist_paths.is_empty() {
        info!(
            "restoring playlist with {} songs",
            saved_state.playlist_paths.len()
        );

        if let Ok(db) = rmpd_library::Database::open(db_path) {
            let mut queue = state.queue.write().await;
            let mut missing = 0usize;

            for (orig_idx, path) in saved_state.playlist_paths.iter().enumerate() {
                // Try to find song in database
                if let Ok(Some(song)) = db.get_song_by_path(path) {
                    queue.add(song);
                } else {
                    missing += 1;
                    // A missing song shifts every later song left; shift the
                    // resume position too (or onto the next survivor if the
                    // current song itself is the one missing).
                    if let Some(pos) = resume_position.as_mut()
                        && (orig_idx as u32) < *pos
                    {
                        *pos -= 1;
                    }
                }
            }

            if missing > 0 {
                warn!(
                    "{missing} of {} restored songs not found in database (skipped)",
                    saved_state.playlist_paths.len()
                );
            }

            let playlist_len = queue.len() as u32;
            drop(queue);

            // Update playlist length in status
            let mut status = state.status.write().await;
            status.playlist_length = playlist_len;
        }
    }

    // Restore current song position and potentially resume playback
    if let Some(position) = resume_position {
        let queue = state.queue.read().await;
        if let Some(item) = queue.get(position) {
            let song = (*item.song).clone();
            let song_id = item.id;
            let range = item.range;
            drop(queue);

            // Check if we should auto-resume playback
            if !restore_paused {
                if let Some(play_state) = saved_state.state
                    && (play_state == PlayerState::Play || play_state == PlayerState::Pause)
                {
                    info!(
                        "auto-resuming playback at position {} (state: {:?})",
                        position, play_state
                    );

                    let playback_song =
                        match rmpd_protocol::commands::utils::prepare_song_for_playback(
                            &song,
                            Some(music_dir),
                            range,
                            &state.sources,
                        )
                        .await
                        {
                            Ok(ps) => ps,
                            Err(e) => {
                                warn!("failed to resolve song during state restore: {}", e);
                                return;
                            }
                        };

                    // Set current song immediately
                    let mut status = state.status.write().await;
                    status.current_song = Some(rmpd_core::state::QueuePosition {
                        position,
                        id: song_id,
                    });
                    status.duration = song.duration;
                    status.bitrate = song.bitrate;

                    // Set audio format if available
                    if let (Some(sr), Some(ch), Some(bps)) =
                        (song.sample_rate, song.channels, song.bits_per_sample)
                    {
                        status.audio_format = Some(rmpd_core::song::AudioFormat {
                            sample_rate: sr,
                            channels: ch,
                            bits_per_sample: bps as u8,
                        });
                    }
                    drop(status);

                    // Spawn background task to start playback (don't block server startup)
                    let state_clone = state.clone();
                    let elapsed = saved_state.elapsed_seconds;
                    tokio::spawn(async move {
                        // Small delay to ensure server is listening
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                        if let Err(e) =
                            resume_playback(&state_clone, playback_song, play_state, elapsed).await
                        {
                            error!("failed to resume playback: {}", e);
                        }
                    });
                } else {
                    // Saved as `state: stop` with a `current:` position: the
                    // stopped player keeps the song it stopped on (MPD
                    // `playlist_state_restore`: `playlist.current = current`).
                    let mut status = state.status.write().await;
                    status.current_song = Some(rmpd_core::state::QueuePosition {
                        position,
                        id: song_id,
                    });
                }
            } else {
                // Don't auto-resume, just set current position
                let mut status = state.status.write().await;
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position,
                    id: song_id,
                });
            }
        }
    }

    info!("state restoration complete");
}

/// Serialize and save current player state. Shared by the periodic ticker
/// and the shutdown paths in `run()`; `state_file` is expected to be one
/// `Arc<StateFile>` shared across all callers so StateFile::save's internal
/// last-written-content check and lock apply across all of them.
async fn save_state(state: &AppState, state_file: &StateFile) {
    let status = state.status.read().await;
    let queue = state.queue.read().await;
    let disabled_outputs: Vec<String> = state
        .outputs
        .read()
        .await
        .iter()
        .filter(|o| !o.enabled)
        .map(|o| o.name.clone())
        .collect();

    if let Err(e) = state_file.save(&status, &queue, &disabled_outputs).await {
        error!("failed to save state: {}", e);
    }
}

async fn resume_playback(
    state: &AppState,
    playback_song: rmpd_core::playback::PlaybackSong,
    target_state: PlayerState,
    elapsed: Option<f64>,
) -> Result<()> {
    state.engine.write().await.play(playback_song).await?;

    {
        let mut status = state.status.write().await;
        status.state = if target_state == PlayerState::Pause {
            PlayerState::Pause
        } else {
            PlayerState::Play
        };
    }

    if let Some(elapsed_time) = elapsed
        && elapsed_time > 0.0
    {
        // Queue the seek under the engine lock, wait for the verdict without
        // it (see `handle_seek_command`). A failed seek (an unseekable
        // source such as a radio stream) must not abort the restore: the
        // saved pause below still has to be applied, or audio would start
        // playing on a daemon that was left paused.
        let pending = state.engine.read().await.begin_seek(elapsed_time);
        let outcome = match pending {
            Ok(pending) => pending.verdict().await,
            Err(e) => Err(e),
        };
        if let Err(e) = outcome {
            warn!("could not seek to the saved position {elapsed_time}s: {e}");
        }
    }

    if target_state == PlayerState::Pause {
        state.engine.write().await.pause().await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::song::Song;

    fn song(path: &str) -> Song {
        Song {
            id: 0,
            path: path.into(),
            duration: None,
            sample_rate: None,
            channels: None,
            bits_per_sample: None,
            bitrate: None,
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
            range: None,
            tags: vec![],
        }
    }

    /// A saved `state: pause` with a `time:` must survive a failing seek (an
    /// unseekable source such as a radio stream, or a song that cannot be
    /// decoded): the restore carries on instead of bailing out before the
    /// pause is applied, which would leave the daemon playing.
    #[tokio::test]
    async fn resume_does_not_abort_when_the_saved_position_cannot_be_seeked() {
        let dir = std::env::temp_dir().join(format!(
            "rmpd-resume-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.wav");
        std::fs::write(&bad, b"this is not audio").unwrap();

        let state = AppState::new();
        let playback_song = rmpd_core::playback::PlaybackSong {
            song: Arc::new(song("bad.wav")),
            resolved_path: bad.to_str().unwrap().into(),
            range: None,
        };

        let result = resume_playback(&state, playback_song, PlayerState::Pause, Some(5.0)).await;
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            result.is_ok(),
            "a failed seek must not abort the restore: {result:?}"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod device_loss_tests {
    use super::lost_device;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_device_still_in_the_list_is_not_lost() {
        let listed = names(&["Headphones", "Speakers"]);
        assert_eq!(
            lost_device(Some("Headphones"), Some("Headphones"), &listed),
            None
        );
    }

    #[test]
    fn a_move_to_another_listed_device_is_not_a_loss() {
        let listed = names(&["Headphones", "Speakers"]);
        assert_eq!(
            lost_device(Some("Headphones"), Some("Speakers"), &listed),
            None
        );
    }

    #[test]
    fn a_device_that_left_the_list_is_reported() {
        let listed = names(&["Speakers"]);
        assert_eq!(
            lost_device(Some("Headphones"), Some("Speakers"), &listed).as_deref(),
            Some("Headphones")
        );
    }

    #[test]
    fn nothing_is_reported_without_a_previous_device() {
        let listed = names(&["Speakers"]);
        assert_eq!(lost_device(None, Some("Speakers"), &listed), None);
    }
}
