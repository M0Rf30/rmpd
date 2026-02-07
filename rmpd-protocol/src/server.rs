use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

use crate::commands::utils::ACK_ERROR_UNKNOWN;
use crate::commands::{
    connection, database, fingerprint, messaging, options, outputs, partition, playback, playlists,
    queue, reflection, stickers, storage,
};
use crate::parser::{parse_command, Command};
use crate::queue_playback::QueuePlaybackManager;
use crate::response::{Response, ResponseBuilder, Stats};
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "0.24.0";

/// Convert Unix timestamp to ISO 8601 format (RFC 3339)
#[derive(Debug)]
pub struct MpdServer {
    bind_address: String,
    state: AppState,
    shutdown_rx: broadcast::Receiver<()>,
}

impl MpdServer {
    pub fn new(bind_address: String, shutdown_rx: broadcast::Receiver<()>) -> Self {
        Self {
            bind_address,
            state: AppState::new(),
            shutdown_rx,
        }
    }

    pub fn with_state(
        bind_address: String,
        state: AppState,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        Self {
            bind_address,
            state,
            shutdown_rx,
        }
    }

    pub async fn run(self) -> Result<()> {
        let listener = TcpListener::bind(&self.bind_address).await?;
        info!("mpd server listening on {}", self.bind_address);
        self.run_with_listener(listener).await
    }

    /// Run the server accept loop using a pre-bound listener.
    ///
    /// This is useful for tests that need to bind to port 0 and discover the
    /// actual port before handing the listener to the server.
    pub async fn run_with_listener(mut self, listener: TcpListener) -> Result<()> {
        // Start queue playback manager
        let mut playback_manager = QueuePlaybackManager::new(self.state.clone());
        playback_manager.start();
        info!("queue playback manager started");

        loop {
            tokio::select! {
                // Handle incoming connections
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            debug!("new connection from {}", addr);
                            let state = self.state.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(stream, state).await {
                                    error!("client error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("failed to accept connection: {}", e);
                        }
                    }
                }
                // Handle shutdown signal
                _ = self.shutdown_rx.recv() => {
                    info!("shutdown signal received, stopping server");
                    break;
                }
            }
        }

        info!("server shutdown complete");
        Ok(())
    }
}

async fn handle_client(mut stream: TcpStream, state: AppState) -> Result<()> {
    // Enable TCP_NODELAY for low-latency responses (disable Nagle's algorithm)
    stream.set_nodelay(true)?;

    // Send greeting
    stream
        .write_all(format!("OK MPD {PROTOCOL_VERSION}\n").as_bytes())
        .await?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Subscribe to event bus for idle notifications
    let mut event_rx = state.event_bus.subscribe();

    // Per-client connection state
    let mut conn_state = crate::ConnectionState::new();

    // Command batching state
    let mut batch_mode = false;
    let mut batch_ok_mode = false;
    let mut batch_commands: Vec<String> = Vec::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;

        if bytes_read == 0 {
            // Connection closed
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        debug!("received command: {}", trimmed);

        // Handle command batching
        let response = match parse_command(trimmed) {
            Ok(Command::CommandListBegin) => {
                batch_mode = true;
                batch_ok_mode = false;
                batch_commands.clear();
                continue; // Don't send response yet
            }
            Ok(Command::CommandListOkBegin) => {
                batch_mode = true;
                batch_ok_mode = true;
                batch_commands.clear();
                continue; // Don't send response yet
            }
            Ok(Command::CommandListEnd) => {
                if !batch_mode {
                    Response::Text(ResponseBuilder::error(
                        5,
                        0,
                        "command_list_end",
                        "not in command list",
                    ))
                } else {
                    let response = execute_command_list(
                        &batch_commands,
                        &state,
                        &mut conn_state,
                        batch_ok_mode,
                    )
                    .await;
                    batch_mode = false;
                    batch_ok_mode = false;
                    batch_commands.clear();
                    response
                }
            }
            Ok(Command::Idle { subsystems }) if !batch_mode => {
                Response::Text(handle_idle(&mut reader, &mut event_rx, subsystems).await)
            }
            Ok(_cmd) if batch_mode => {
                // Accumulate commands in batch
                batch_commands.push(trimmed.to_string());
                continue; // Don't send response yet
            }
            Ok(Command::Close) => {
                // Close: terminate the connection immediately (per MPD spec)
                break;
            }
            Ok(cmd) => handle_command(cmd, &state, &mut conn_state).await,
            Err(e) => Response::Text(ResponseBuilder::error(ACK_ERROR_UNKNOWN, 0, trimmed, &e)),
        };

        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?; // Flush immediately to ensure low latency
    }

    Ok(())
}

async fn execute_command_list(
    commands: &[String],
    state: &AppState,
    conn_state: &mut crate::ConnectionState,
    ok_mode: bool,
) -> Response {
    let mut response = String::new();

    for (index, cmd_str) in commands.iter().enumerate() {
        match parse_command(cmd_str) {
            Ok(cmd) => {
                let cmd_response = handle_command(cmd, state, conn_state).await;

                // Convert response to string for batching (binary commands not allowed in batch)
                let cmd_response_str = match cmd_response {
                    Response::Text(s) => s,
                    Response::Binary(_) => {
                        return Response::Text(ResponseBuilder::error(
                            5,
                            index as i32,
                            cmd_str,
                            "binary commands not allowed in command list",
                        ));
                    }
                };

                // Check for errors
                if cmd_response_str.starts_with("ACK") {
                    // Return error with command list index
                    return Response::Text(
                        cmd_response_str.replace("ACK [", &format!("ACK [{index}@")),
                    );
                }

                if ok_mode {
                    // In OK mode, append list_OK after each successful command
                    response.push_str("list_OK\n");
                }
            }
            Err(e) => {
                // Parse error - return ACK with index
                return Response::Text(ResponseBuilder::error(
                    ACK_ERROR_UNKNOWN,
                    index as i32,
                    cmd_str,
                    &e,
                ));
            }
        }
    }

    // All commands succeeded
    response.push_str("OK\n");
    Response::Text(response)
}

async fn handle_idle(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    event_rx: &mut broadcast::Receiver<rmpd_core::event::Event>,
    subsystems: Vec<String>,
) -> String {
    use rmpd_core::event::Subsystem;
    use tokio::sync::broadcast::error::RecvError;

    // Convert string subsystems to enum
    let filter_subsystems: Vec<Subsystem> = if subsystems.is_empty() {
        // If no subsystems specified, listen to all
        vec![]
    } else {
        subsystems
            .iter()
            .filter_map(|s| match s.to_lowercase().as_str() {
                "database" => Some(Subsystem::Database),
                "update" => Some(Subsystem::Update),
                "stored_playlist" => Some(Subsystem::StoredPlaylist),
                "playlist" => Some(Subsystem::Playlist),
                "player" => Some(Subsystem::Player),
                "mixer" => Some(Subsystem::Mixer),
                "output" => Some(Subsystem::Output),
                "options" => Some(Subsystem::Options),
                "partition" => Some(Subsystem::Partition),
                "sticker" => Some(Subsystem::Sticker),
                "subscription" => Some(Subsystem::Subscription),
                "message" => Some(Subsystem::Message),
                "neighbor" => Some(Subsystem::Neighbor),
                "mount" => Some(Subsystem::Mount),
                _ => None,
            })
            .collect()
    };

    let mut line = String::new();

    loop {
        tokio::select! {
            // Wait for event
            event_result = event_rx.recv() => {
                match event_result {
                    Ok(event) => {
                        debug!("idle received event: {:?}", event);
                        let event_subsystems = event.subsystems();

                        // Check if event matches any subscribed subsystem
                        let matches = if filter_subsystems.is_empty() {
                            // No filter - return any event
                            !event_subsystems.is_empty()
                        } else {
                            // Check if event matches any filtered subsystem
                            event_subsystems.iter().any(|s| filter_subsystems.contains(s))
                        };

                        debug!("event matches filter: {}, subsystems: {:?}", matches, event_subsystems);

                        if matches {
                            // Return changed subsystem
                            let subsystem_name = subsystem_to_string(event_subsystems[0]);
                            debug!("idle returning: changed: {}", subsystem_name);
                            return format!("changed: {subsystem_name}\nOK\n");
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        // Channel lagged - messages were dropped
                        // Return immediately to notify client of changes
                        debug!("idle: channel lagged, skipped {} messages", skipped);
                        return "changed: player\nOK\n".to_owned();
                    }
                    Err(RecvError::Closed) => {
                        // Channel closed - should not happen, but handle gracefully
                        debug!("idle: event channel closed");
                        return "OK\n".to_owned();
                    }
                }
            }
            // Wait for noidle command
            line_result = reader.read_line(&mut line) => {
                if let Ok(bytes) = line_result {
                    if bytes > 0 && line.trim() == "noidle" {
                        // Cancel idle
                        return "OK\n".to_owned();
                    }
                }
                // Connection closed or error
                return "OK\n".to_owned();
            }
        }
    }
}

fn subsystem_to_string(subsystem: rmpd_core::event::Subsystem) -> &'static str {
    use rmpd_core::event::Subsystem;
    match subsystem {
        Subsystem::Database => "database",
        Subsystem::Update => "update",
        Subsystem::StoredPlaylist => "stored_playlist",
        Subsystem::Playlist => "playlist",
        Subsystem::Player => "player",
        Subsystem::Mixer => "mixer",
        Subsystem::Output => "output",
        Subsystem::Options => "options",
        Subsystem::Partition => "partition",
        Subsystem::Sticker => "sticker",
        Subsystem::Subscription => "subscription",
        Subsystem::Message => "message",
        Subsystem::Neighbor => "neighbor",
        Subsystem::Mount => "mount",
    }
}

async fn handle_command(
    cmd: Command,
    state: &AppState,
    conn_state: &mut crate::ConnectionState,
) -> Response {
    // Special handling for binary commands
    match cmd {
        Command::AlbumArt { uri, offset } => {
            return database::handle_albumart_command(state, &uri, offset).await;
        }
        Command::ReadPicture { uri, offset } => {
            return database::handle_readpicture_command(state, &uri, offset).await;
        }
        _ => {}
    }

    // All other commands return text responses
    let response_str = match cmd {
        Command::Ping => ResponseBuilder::new().ok(),
        Command::Close => {
            // Close is handled in the accept loop; this branch is unreachable
            // but kept so the match is exhaustive.
            unreachable!("Close is handled before dispatch")
        }
        Command::Commands => reflection::handle_commands_command().await,
        Command::NotCommands => reflection::handle_notcommands_command().await,
        Command::TagTypes { subcommand } => {
            reflection::handle_tagtypes_command(conn_state, subcommand).await
        }
        Command::UrlHandlers => reflection::handle_urlhandlers_command().await,
        Command::Decoders => reflection::handle_decoders_command().await,
        Command::Status => {
            let status = {
                let mut guard = state.status.write().await;
                // Sync status.state with atomic_state WHILE holding the lock
                // This prevents race conditions between reading atomic_state and writing to status
                guard.state = rmpd_core::state::PlayerState::from_atomic(
                    state
                        .atomic_state
                        .load(std::sync::atomic::Ordering::Acquire),
                );
                guard.clone()
            };

            let mut resp = ResponseBuilder::new();
            resp.status(&status);
            resp.ok()
        }
        Command::Stats => {
            // Get stats from database if available
            let (songs, artists, albums, db_playtime, db_update) =
                if let Some(ref db_path) = state.db_path {
                    if let Ok(db) = rmpd_library::Database::open(db_path) {
                        db.get_stats().unwrap_or((0, 0, 0, 0, 0))
                    } else {
                        (0, 0, 0, 0, 0)
                    }
                } else {
                    (0, 0, 0, 0, 0)
                };

            // Calculate uptime in seconds
            let uptime = state.start_time.elapsed().as_secs();

            let stats = Stats {
                artists,
                albums,
                songs,
                uptime,
                db_playtime,
                db_update,
                playtime: 0,
            };

            let mut resp = ResponseBuilder::new();
            resp.stats(&stats);
            resp.ok()
        }
        Command::ClearError => {
            // Clear the error field in status
            state.status.write().await.error = None;
            ResponseBuilder::new().ok()
        }
        Command::Update { path } | Command::Rescan { path } => {
            database::handle_update_command(state, path.as_deref()).await
        }
        Command::Find {
            filters,
            sort,
            window,
        } => database::handle_find_command(state, &filters, sort.as_deref(), window).await,
        Command::Search {
            filters,
            sort,
            window,
        } => database::handle_search_command(state, &filters, sort.as_deref(), window).await,
        Command::List {
            tag,
            filter_tag,
            filter_value,
            group: _,
        } => {
            database::handle_list_command(
                state,
                &tag,
                filter_tag.as_deref(),
                filter_value.as_deref(),
            )
            .await
        }
        Command::Count { filters, group } => {
            database::handle_count_command(state, &filters, group.as_deref()).await
        }
        Command::ListAll { path } => database::handle_listall_command(state, path.as_deref()).await,
        Command::ListAllInfo { path } => {
            database::handle_listallinfo_command(state, path.as_deref()).await
        }
        Command::LsInfo { path } => database::handle_lsinfo_command(state, path.as_deref()).await,
        Command::CurrentSong => database::handle_currentsong_command(state).await,
        Command::PlaylistInfo { range } => queue::handle_playlistinfo_command(state, range).await,
        Command::Playlist => {
            // Deprecated, same as playlistinfo without range
            queue::handle_playlistinfo_command(state, None).await
        }
        Command::PlChanges { version, range } => {
            queue::handle_plchanges_command(state, version, range).await
        }
        Command::PlChangesPosId { version, range } => {
            queue::handle_plchangesposid_command(state, version, range).await
        }
        Command::PlaylistFind { tag, value } => {
            queue::handle_playlistfind_command(state, &tag, &value).await
        }
        Command::PlaylistSearch { tag, value } => {
            queue::handle_playlistsearch_command(state, &tag, &value).await
        }
        // Playback commands
        Command::Play { position } => playback::handle_play_command(state, position).await,
        Command::Pause { state: pause_state } => {
            playback::handle_pause_command(state, pause_state).await
        }
        Command::Stop => playback::handle_stop_command(state).await,
        Command::Next => playback::handle_next_command(state).await,
        Command::Previous => playback::handle_previous_command(state).await,
        Command::Seek { position, time } => {
            playback::handle_seek_command(state, position, time).await
        }
        Command::SeekId { id, time } => playback::handle_seekid_command(state, id, time).await,
        Command::SeekCur { time, relative } => {
            playback::handle_seekcur_command(state, time, relative).await
        }
        Command::SetVol { volume } => options::handle_setvol_command(state, volume).await,
        Command::Add { uri, position } => queue::handle_add_command(state, &uri, position).await,
        Command::Clear => queue::handle_clear_command(state).await,
        Command::Delete { target } => queue::handle_delete_command(state, target).await,
        Command::DeleteId { id } => queue::handle_deleteid_command(state, id).await,
        Command::AddId { uri, position } => {
            queue::handle_addid_command(state, &uri, position).await
        }
        Command::PlayId { id } => queue::handle_playid_command(state, id).await,
        Command::MoveId { id, to } => queue::handle_moveid_command(state, id, to).await,
        Command::Swap { pos1, pos2 } => queue::handle_swap_command(state, pos1, pos2).await,
        Command::SwapId { id1, id2 } => queue::handle_swapid_command(state, id1, id2).await,
        Command::Move { from, to } => queue::handle_move_command(state, from, to).await,
        Command::Shuffle { range } => queue::handle_shuffle_command(state, range).await,
        Command::PlaylistId { id } => queue::handle_playlistid_command(state, id).await,
        Command::Password { password: _ } => {
            // No password protection implemented yet
            ResponseBuilder::new().ok()
        }
        Command::AlbumArt { .. } | Command::ReadPicture { .. } => {
            // Already handled at the beginning of the function
            unreachable!()
        }
        Command::Unknown(cmd) => {
            ResponseBuilder::error(ACK_ERROR_UNKNOWN, 0, &cmd, "unknown command")
        }
        Command::Repeat { enabled } => options::handle_repeat_command(state, enabled).await,
        Command::Random { enabled } => options::handle_random_command(state, enabled).await,
        Command::Single { mode } => options::handle_single_command(state, &mode).await,
        Command::Consume { mode } => options::handle_consume_command(state, &mode).await,
        Command::Crossfade { seconds } => options::handle_crossfade_command(state, seconds).await,
        Command::Volume { change } => options::handle_volume_command(state, change).await,
        Command::GetVol => {
            let status = state.status.read().await;
            let mut resp = ResponseBuilder::new();
            resp.field("volume", status.volume.to_string());
            resp.ok()
        }
        Command::ReplayGainMode { mode } => {
            options::handle_replaygain_mode_command(state, &mode).await
        }
        Command::ReplayGainStatus => options::handle_replaygain_status_command(state).await,
        Command::BinaryLimit { size } => {
            // Set binary limit (for large responses like images)
            // Store in connection state if needed, for now just acknowledge
            let _ = size;
            ResponseBuilder::new().ok()
        }
        Command::Protocol { subcommand } => {
            reflection::handle_protocol_command(conn_state, subcommand).await
        }
        // Stored playlists
        Command::Save { name, mode } => playlists::handle_save_command(state, &name, mode).await,
        Command::Load {
            name,
            range,
            position,
        } => playlists::handle_load_command(state, &name, range, position).await,
        Command::ListPlaylists => playlists::handle_listplaylists_command(state).await,
        Command::ListPlaylist { name, range } => {
            playlists::handle_listplaylist_command(state, &name, range).await
        }
        Command::ListPlaylistInfo { name, range } => {
            playlists::handle_listplaylistinfo_command(state, &name, range).await
        }
        Command::PlaylistAdd {
            name,
            uri,
            position,
        } => playlists::handle_playlistadd_command(state, &name, &uri, position).await,
        Command::PlaylistClear { name } => {
            playlists::handle_playlistclear_command(state, &name).await
        }
        Command::PlaylistDelete { name, position } => {
            playlists::handle_playlistdelete_command(state, &name, position).await
        }
        Command::PlaylistMove { name, from, to } => {
            playlists::handle_playlistmove_command(state, &name, from, to).await
        }
        Command::Rm { name } => playlists::handle_rm_command(state, &name).await,
        Command::Rename { from, to } => playlists::handle_rename_command(state, &from, &to).await,
        Command::SearchPlaylist { name, tag, value } => {
            playlists::handle_searchplaylist_command(state, &name, &tag, &value).await
        }
        Command::PlaylistLength { name } => {
            playlists::handle_playlistlength_command(state, &name).await
        }
        // Output control
        Command::Outputs => outputs::handle_outputs_command(state).await,
        Command::EnableOutput { id } => outputs::handle_enableoutput_command(state, id).await,
        Command::DisableOutput { id } => outputs::handle_disableoutput_command(state, id).await,
        Command::ToggleOutput { id } => outputs::handle_toggleoutput_command(state, id).await,
        Command::OutputSet { id, name, value } => {
            outputs::handle_outputset_command(state, id, &name, &value).await
        }
        // Advanced database
        Command::SearchAdd { tag, value } => {
            database::handle_searchadd_command(state, &tag, &value).await
        }
        Command::SearchAddPl { name, tag, value } => {
            playlists::handle_searchaddpl_command(state, &name, &tag, &value).await
        }
        Command::FindAdd { tag, value } => {
            database::handle_findadd_command(state, &tag, &value).await
        }
        Command::ListFiles { uri } => {
            database::handle_listfiles_command(state, uri.as_deref()).await
        }
        Command::SearchCount { tag, value, group } => {
            database::handle_searchcount_command(state, &tag, &value, group.as_deref()).await
        }
        Command::GetFingerprint { uri } => {
            fingerprint::handle_getfingerprint_command(state, &uri).await
        }
        Command::ReadComments { uri } => database::handle_readcomments_command(state, &uri).await,
        // Stickers
        Command::StickerGet { uri, name } => {
            stickers::handle_sticker_get_command(state, &uri, &name).await
        }
        Command::StickerSet { uri, name, value } => {
            stickers::handle_sticker_set_command(state, &uri, &name, &value).await
        }
        Command::StickerDelete { uri, name } => {
            stickers::handle_sticker_delete_command(state, &uri, name.as_deref()).await
        }
        Command::StickerList { uri } => stickers::handle_sticker_list_command(state, &uri).await,
        Command::StickerFind { uri, name, value } => {
            stickers::handle_sticker_find_command(state, &uri, &name, value.as_deref()).await
        }
        Command::StickerInc { uri, name, delta } => {
            stickers::handle_sticker_inc_command(state, &uri, &name, delta).await
        }
        Command::StickerDec { uri, name, delta } => {
            stickers::handle_sticker_dec_command(state, &uri, &name, delta).await
        }
        Command::StickerNames { uri } => {
            stickers::handle_sticker_names_command(state, uri.as_deref()).await
        }
        Command::StickerTypes => stickers::handle_sticker_types_command().await,
        Command::StickerNamesTypes { uri } => {
            stickers::handle_sticker_namestypes_command(state, uri.as_deref()).await
        }
        // Partitions
        Command::Partition { name } => {
            partition::handle_partition_command(state, conn_state, &name).await
        }
        Command::ListPartitions => partition::handle_listpartitions_command(state).await,
        Command::NewPartition { name } => {
            partition::handle_newpartition_command(state, &name).await
        }
        Command::DelPartition { name } => {
            partition::handle_delpartition_command(state, &name).await
        }
        Command::MoveOutput { name } => {
            partition::handle_moveoutput_command(state, conn_state, &name).await
        }
        // Mounts
        Command::Mount { path, uri } => storage::handle_mount_command(state, &path, &uri).await,
        Command::Unmount { path } => storage::handle_unmount_command(state, &path).await,
        Command::ListMounts => storage::handle_listmounts_command(state).await,
        Command::ListNeighbors => storage::handle_listneighbors_command(state).await,
        // Client messaging
        Command::Subscribe { channel } => {
            messaging::handle_subscribe_command(conn_state, &channel).await
        }
        Command::Unsubscribe { channel } => {
            messaging::handle_unsubscribe_command(conn_state, &channel).await
        }
        Command::Channels => messaging::handle_channels_command(state).await,
        Command::ReadMessages => messaging::handle_readmessages_command(state, conn_state).await,
        Command::SendMessage { channel, message } => {
            messaging::handle_sendmessage_command(state, &channel, &message).await
        }
        // Advanced queue
        Command::Prio { priority, ranges } => {
            queue::handle_prio_command(state, priority, &ranges).await
        }
        Command::PrioId { priority, ids } => {
            queue::handle_prioid_command(state, priority, &ids).await
        }
        Command::RangeId { id, range } => queue::handle_rangeid_command(state, id, range).await,
        Command::AddTagId { id, tag, value } => {
            queue::handle_addtagid_command(state, id, &tag, &value).await
        }
        Command::ClearTagId { id, tag } => {
            queue::handle_cleartagid_command(state, id, tag.as_deref()).await
        }
        // Miscellaneous
        Command::Config => connection::handle_config_command(state).await,
        Command::Kill => connection::handle_kill_command(state).await,
        Command::MixRampDb { decibels } => options::handle_mixrampdb_command(state, decibels).await,
        Command::MixRampDelay { seconds } => {
            options::handle_mixrampdelay_command(state, seconds).await
        }
        _ => {
            // Unimplemented commands
            ResponseBuilder::error(ACK_ERROR_UNKNOWN, 0, "command", "not yet implemented")
        }
    };

    Response::Text(response_str)
}
