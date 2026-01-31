use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

use crate::commands::{database, options, outputs, playback, playlists, queue, reflection, stickers};
use crate::parser::{parse_command, Command};
use crate::queue_playback::QueuePlaybackManager;
use crate::response::{Response, ResponseBuilder, Stats};
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "0.24.0";

/// Strip music directory prefix from absolute path
fn strip_music_dir_prefix<'a>(path: &'a str, music_dir: Option<&str>) -> &'a str {
    if let Some(music_dir) = music_dir {
        // Normalize music_dir to end with /
        let music_dir_with_slash = if music_dir.ends_with('/') {
            music_dir
        } else {
            // Need to handle this case by checking both variants
            if let Some(stripped) = path.strip_prefix(music_dir) {
                return stripped.trim_start_matches('/');
            }
            music_dir
        };

        if let Some(stripped) = path.strip_prefix(music_dir_with_slash) {
            return stripped;
        }
    }
    path
}

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

    pub async fn run(mut self) -> Result<()> {
        let listener = TcpListener::bind(&self.bind_address).await?;
        info!("MPD server listening on {}", self.bind_address);

        // Start queue playback manager
        let mut playback_manager = QueuePlaybackManager::new(self.state.clone());
        playback_manager.start();
        info!("Queue playback manager started");

        loop {
            tokio::select! {
                // Handle incoming connections
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            debug!("New connection from {}", addr);
                            let state = self.state.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(stream, state).await {
                                    error!("Client error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("Failed to accept connection: {}", e);
                        }
                    }
                }
                // Handle shutdown signal
                _ = self.shutdown_rx.recv() => {
                    info!("Shutdown signal received, stopping server...");
                    break;
                }
            }
        }

        info!("Server shutdown complete");
        Ok(())
    }
}

async fn handle_client(mut stream: TcpStream, state: AppState) -> Result<()> {
    // Enable TCP_NODELAY for low-latency responses (disable Nagle's algorithm)
    stream.set_nodelay(true)?;

    // Send greeting
    stream
        .write_all(format!("OK MPD {}\n", PROTOCOL_VERSION).as_bytes())
        .await?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Subscribe to event bus for idle notifications
    let mut event_rx = state.event_bus.subscribe();

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

        debug!("Received command: {}", trimmed);

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
                    let response =
                        execute_command_list(&batch_commands, &state, batch_ok_mode).await;
                    batch_mode = false;
                    batch_ok_mode = false;
                    batch_commands.clear();
                    response
                }
            }
            Ok(Command::Idle { subsystems }) if !batch_mode => {
                Response::Text(handle_idle(&mut reader, &mut event_rx, subsystems).await)
            }
            Ok(cmd) if batch_mode => {
                // Accumulate commands in batch
                batch_commands.push(trimmed.to_string());
                continue; // Don't send response yet
            }
            Ok(cmd) => handle_command(cmd, &state).await,
            Err(e) => Response::Text(ResponseBuilder::error(5, 0, trimmed, &e)),
        };

        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?; // Flush immediately to ensure low latency
    }

    Ok(())
}

async fn execute_command_list(commands: &[String], state: &AppState, ok_mode: bool) -> Response {
    let mut response = String::new();

    for (index, cmd_str) in commands.iter().enumerate() {
        match parse_command(cmd_str) {
            Ok(cmd) => {
                let cmd_response = handle_command(cmd, state).await;

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
                        cmd_response_str.replace("ACK [", &format!("ACK [{}@", index)),
                    );
                }

                if ok_mode {
                    // In OK mode, append list_OK after each successful command
                    response.push_str("list_OK\n");
                }
            }
            Err(e) => {
                // Parse error - return ACK with index
                return Response::Text(ResponseBuilder::error(5, index as i32, cmd_str, &e));
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
                        debug!("Idle received event: {:?}", event);
                        let event_subsystems = event.subsystems();

                        // Check if event matches any subscribed subsystem
                        let matches = if filter_subsystems.is_empty() {
                            // No filter - return any event
                            !event_subsystems.is_empty()
                        } else {
                            // Check if event matches any filtered subsystem
                            event_subsystems.iter().any(|s| filter_subsystems.contains(s))
                        };

                        debug!("Event matches filter: {}, subsystems: {:?}", matches, event_subsystems);

                        if matches {
                            // Return changed subsystem
                            let subsystem_name = subsystem_to_string(event_subsystems[0]);
                            debug!("Idle returning: changed: {}", subsystem_name);
                            return format!("changed: {}\nOK\n", subsystem_name);
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        // Channel lagged - messages were dropped
                        // Return immediately to notify client of changes
                        debug!("Idle: channel lagged, skipped {} messages", skipped);
                        return "changed: player\nOK\n".to_owned();
                    }
                    Err(RecvError::Closed) => {
                        // Channel closed - should not happen, but handle gracefully
                        debug!("Idle: event channel closed");
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

async fn handle_command(cmd: Command, state: &AppState) -> Response {
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
            // Connection will be closed by the handler
            ResponseBuilder::new().ok()
        }
        Command::Commands => reflection::handle_commands_command().await,
        Command::NotCommands => reflection::handle_notcommands_command().await,
        Command::TagTypes { subcommand } => reflection::handle_tagtypes_command(subcommand).await,
        Command::UrlHandlers => reflection::handle_urlhandlers_command().await,
        Command::Decoders => reflection::handle_decoders_command().await,
        Command::Status => {
            // Read status with lock held
            let mut status_guard = state.status.write().await;

            // Sync status.state with atomic_state WHILE holding the lock
            // This prevents race conditions between reading atomic_state and writing to status
            let atomic_state_val = state.atomic_state.load(std::sync::atomic::Ordering::SeqCst);
            let atomic_player_state = match atomic_state_val {
                0 => rmpd_core::state::PlayerState::Stop,
                1 => rmpd_core::state::PlayerState::Play,
                2 => rmpd_core::state::PlayerState::Pause,
                _ => rmpd_core::state::PlayerState::Stop,
            };
            status_guard.state = atomic_player_state;

            // Clone status and release lock
            let status = status_guard.clone();
            drop(status_guard);

            let mut resp = ResponseBuilder::new();
            resp.status(&status);
            resp.ok()
        }
        Command::Stats => {
            // Get stats from database if available
            let (songs, artists, albums, db_playtime, db_update) =
                if let Some(ref db_path) = state.db_path {
                    if let Ok(db) = rmpd_library::Database::open(db_path) {
                        let songs = db.count_songs().unwrap_or(0);
                        let artists = db.count_artists().unwrap_or(0);
                        let albums = db.count_albums().unwrap_or(0);
                        let db_playtime = db.get_db_playtime().unwrap_or(0);
                        let db_update = db.get_db_update().unwrap_or(0);

                        (songs, artists, albums, db_playtime, db_update)
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
        } => database::handle_list_command(state, &tag, filter_tag.as_deref(), filter_value.as_deref()).await,
        Command::Count { filters, group } => {
            database::handle_count_command(state, &filters, group.as_deref()).await
        }
        Command::ListAll { path } => database::handle_listall_command(state, path.as_deref()).await,
        Command::ListAllInfo { path } => database::handle_listallinfo_command(state, path.as_deref()).await,
        Command::LsInfo { path } => database::handle_lsinfo_command(state, path.as_deref()).await,
        Command::CurrentSong => database::handle_currentsong_command(state).await,
        Command::PlaylistInfo { range } => queue::handle_playlistinfo_command(state, range).await,
        Command::Playlist => {
            // Deprecated, same as playlistinfo without range
            queue::handle_playlistinfo_command(state, None).await
        }
        Command::PlChanges { version, range } => {
            handle_plchanges_command(state, version, range).await
        }
        Command::PlChangesPosId { version, range } => {
            handle_plchangesposid_command(state, version, range).await
        }
        Command::PlaylistFind { tag, value } => {
            handle_playlistfind_command(state, &tag, &value).await
        }
        Command::PlaylistSearch { tag, value } => {
            handle_playlistsearch_command(state, &tag, &value).await
        }
        // Playback commands
        Command::Play { position } => playback::handle_play_command(state, position).await,
        Command::Pause { state: pause_state } => playback::handle_pause_command(state, pause_state).await,
        Command::Stop => playback::handle_stop_command(state).await,
        Command::Next => playback::handle_next_command(state).await,
        Command::Previous => playback::handle_previous_command(state).await,
        Command::Seek { position, time } => playback::handle_seek_command(state, position, time).await,
        Command::SeekId { id, time } => playback::handle_seekid_command(state, id, time).await,
        Command::SeekCur { time, relative } => playback::handle_seekcur_command(state, time, relative).await,
        Command::SetVol { volume } => options::handle_setvol_command(state, volume).await,
        Command::Add { uri, position } => queue::handle_add_command(state, &uri, position).await,
        Command::Clear => queue::handle_clear_command(state).await,
        Command::Delete { target } => queue::handle_delete_command(state, target).await,
        Command::DeleteId { id } => queue::handle_deleteid_command(state, id).await,
        Command::AddId { uri, position } => queue::handle_addid_command(state, &uri, position).await,
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
        Command::Unknown(cmd) => ResponseBuilder::error(5, 0, &cmd, "unknown command"),
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
        Command::ReplayGainMode { mode } => options::handle_replaygain_mode_command(state, &mode).await,
        Command::ReplayGainStatus => options::handle_replaygain_status_command(state).await,
        Command::BinaryLimit { size } => {
            // Set binary limit (for large responses like images)
            // Store in connection state if needed, for now just acknowledge
            let _ = size;
            ResponseBuilder::new().ok()
        }
        Command::Protocol { subcommand } => reflection::handle_protocol_command(subcommand).await,
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
        Command::PlaylistClear { name } => playlists::handle_playlistclear_command(state, &name).await,
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
        Command::PlaylistLength { name } => playlists::handle_playlistlength_command(state, &name).await,
        // Output control
        Command::Outputs => outputs::handle_outputs_command(state).await,
        Command::EnableOutput { id } => outputs::handle_enableoutput_command(state, id).await,
        Command::DisableOutput { id } => outputs::handle_disableoutput_command(state, id).await,
        Command::ToggleOutput { id } => outputs::handle_toggleoutput_command(state, id).await,
        Command::OutputSet { id, name, value } => {
            outputs::handle_outputset_command(state, id, &name, &value).await
        }
        // Advanced database
        Command::SearchAdd { tag, value } => database::handle_searchadd_command(state, &tag, &value).await,
        Command::SearchAddPl { name, tag, value } => {
            playlists::handle_searchaddpl_command(state, &name, &tag, &value).await
        }
        Command::FindAdd { tag, value } => database::handle_findadd_command(state, &tag, &value).await,
        Command::ListFiles { uri } => database::handle_listfiles_command(state, uri.as_deref()).await,
        Command::SearchCount { tag, value, group } => {
            handle_searchcount_command(state, &tag, &value, group.as_deref()).await
        }
        Command::GetFingerprint { uri } => handle_getfingerprint_command(state, &uri).await,
        Command::ReadComments { uri } => handle_readcomments_command(state, &uri).await,
        // Stickers
        Command::StickerGet { uri, name } => stickers::handle_sticker_get_command(state, &uri, &name).await,
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
        Command::StickerNames { uri } => stickers::handle_sticker_names_command(state, uri.as_deref()).await,
        Command::StickerTypes => stickers::handle_sticker_types_command().await,
        Command::StickerNamesTypes { uri } => {
            stickers::handle_sticker_namestypes_command(state, uri.as_deref()).await
        }
        // Partitions
        Command::Partition { name } => handle_partition_command(state, &name).await,
        Command::ListPartitions => handle_listpartitions_command().await,
        Command::NewPartition { name } => handle_newpartition_command(&name).await,
        Command::DelPartition { name } => handle_delpartition_command(&name).await,
        Command::MoveOutput { name } => handle_moveoutput_command(&name).await,
        // Mounts
        Command::Mount { path, uri } => handle_mount_command(&path, &uri).await,
        Command::Unmount { path } => handle_unmount_command(&path).await,
        Command::ListMounts => handle_listmounts_command().await,
        Command::ListNeighbors => handle_listneighbors_command().await,
        // Client messaging
        Command::Subscribe { channel } => handle_subscribe_command(&channel).await,
        Command::Unsubscribe { channel } => handle_unsubscribe_command(&channel).await,
        Command::Channels => handle_channels_command().await,
        Command::ReadMessages => handle_readmessages_command().await,
        Command::SendMessage { channel, message } => {
            handle_sendmessage_command(&channel, &message).await
        }
        // Advanced queue
        Command::Prio { priority, ranges } => handle_prio_command(state, priority, &ranges).await,
        Command::PrioId { priority, ids } => handle_prioid_command(state, priority, &ids).await,
        Command::RangeId { id, range } => handle_rangeid_command(state, id, range).await,
        Command::AddTagId { id, tag, value } => {
            handle_addtagid_command(state, id, &tag, &value).await
        }
        Command::ClearTagId { id, tag } => {
            handle_cleartagid_command(state, id, tag.as_deref()).await
        }
        // Miscellaneous
        Command::Config => handle_config_command().await,
        Command::Kill => handle_kill_command().await,
        Command::MixRampDb { decibels } => options::handle_mixrampdb_command(state, decibels).await,
        Command::MixRampDelay { seconds } => options::handle_mixrampdelay_command(state, seconds).await,
        _ => {
            // Unimplemented commands
            ResponseBuilder::error(5, 0, "command", "not yet implemented")
        }
    };

    Response::Text(response_str)
}


// Partition commands (multi-queue support)
async fn handle_partition_command(_state: &AppState, _name: &str) -> String {
    // Switch to partition (not fully implemented)
    ResponseBuilder::new().ok()
}

async fn handle_listpartitions_command() -> String {
    // List partitions - only default for now
    let mut resp = ResponseBuilder::new();
    resp.field("partition", "default");
    resp.ok()
}

async fn handle_newpartition_command(_name: &str) -> String {
    // Create new partition (not fully implemented)
    ResponseBuilder::new().ok()
}

async fn handle_delpartition_command(_name: &str) -> String {
    // Delete partition (not fully implemented)
    ResponseBuilder::new().ok()
}

async fn handle_moveoutput_command(_name: &str) -> String {
    // Move output to current partition (not fully implemented)
    ResponseBuilder::new().ok()
}

// Mount commands (virtual filesystem)
async fn handle_mount_command(_path: &str, _uri: &str) -> String {
    // Mount storage (not implemented - would require virtual FS)
    ResponseBuilder::new().ok()
}

async fn handle_unmount_command(_path: &str) -> String {
    // Unmount storage (not implemented)
    ResponseBuilder::new().ok()
}

async fn handle_listmounts_command() -> String {
    // List mounts - return empty
    ResponseBuilder::new().ok()
}

async fn handle_listneighbors_command() -> String {
    // List network neighbors - return empty
    ResponseBuilder::new().ok()
}

// Client-to-client messaging
async fn handle_subscribe_command(_channel: &str) -> String {
    // Subscribe to channel (stub)
    ResponseBuilder::new().ok()
}

async fn handle_unsubscribe_command(_channel: &str) -> String {
    // Unsubscribe from channel (stub)
    ResponseBuilder::new().ok()
}

async fn handle_channels_command() -> String {
    // List channels - return empty
    ResponseBuilder::new().ok()
}

async fn handle_readmessages_command() -> String {
    // Read messages - return empty
    ResponseBuilder::new().ok()
}

async fn handle_sendmessage_command(_channel: &str, _message: &str) -> String {
    // Send message (stub)
    ResponseBuilder::new().ok()
}

// Advanced queue operations
async fn handle_prio_command(state: &AppState, _priority: u8, _ranges: &[(u32, u32)]) -> String {
    // Set priority for multiple ranges (stub - would need priority field in QueueItem)
    // TODO: Implement priority support in QueueItem
    // For each range in ranges, set priority for items in that range
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

async fn handle_prioid_command(state: &AppState, _priority: u8, _ids: &[u32]) -> String {
    // Set priority for multiple IDs (stub - would need priority field in QueueItem)
    // TODO: Implement priority support in QueueItem
    // For each ID in ids, set priority for that queue item
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

async fn handle_rangeid_command(state: &AppState, _id: u32, _range: (f64, f64)) -> String {
    // Set playback range for song (stub)
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

async fn handle_addtagid_command(state: &AppState, _id: u32, _tag: &str, _value: &str) -> String {
    // Add tag to queue item (stub)
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

async fn handle_cleartagid_command(state: &AppState, _id: u32, _tag: Option<&str>) -> String {
    // Clear tags from queue item (stub)
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

// Miscellaneous commands
async fn handle_config_command() -> String {
    // Return configuration - minimal for now
    let mut resp = ResponseBuilder::new();
    resp.field("music_directory", "/var/lib/mpd/music");
    resp.ok()
}

async fn handle_kill_command() -> String {
    // Kill server (stub - should trigger graceful shutdown)
    ResponseBuilder::new().ok()
}

// Queue inspection commands
async fn handle_plchanges_command(
    state: &AppState,
    version: u32,
    range: Option<(u32, u32)>,
) -> String {
    // Return changes in queue since version
    // MPD protocol: version 0 means "give me current playlist"
    // Otherwise, return items if playlist has changed since given version
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if version == 0 || queue.version() > version {
        let items = queue.items();

        // Apply range filter
        let filtered = if let Some((start, end)) = range {
            let start_idx = start as usize;
            let end_idx = end.min(items.len() as u32) as usize;
            if start_idx < items.len() {
                &items[start_idx..end_idx]
            } else {
                &[]
            }
        } else {
            items
        };

        for item in filtered {
            resp.field("file", item.song.path.as_str());
            resp.field("Pos", item.position.to_string());
            resp.field("Id", item.id.to_string());
            if let Some(ref title) = item.song.title {
                resp.field("Title", title);
            }
        }
    }
    resp.ok()
}

async fn handle_plchangesposid_command(
    state: &AppState,
    version: u32,
    range: Option<(u32, u32)>,
) -> String {
    // Return position/id changes since version
    // MPD protocol: version 0 means "give me current playlist"
    // Otherwise, return items if playlist has changed since given version
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if version == 0 || queue.version() > version {
        let items = queue.items();

        // Apply range filter
        let filtered = if let Some((start, end)) = range {
            let start_idx = start as usize;
            let end_idx = end.min(items.len() as u32) as usize;
            if start_idx < items.len() {
                &items[start_idx..end_idx]
            } else {
                &[]
            }
        } else {
            items
        };

        for item in filtered {
            resp.field("cpos", item.position.to_string());
            resp.field("Id", item.id.to_string());
        }
    }
    resp.ok()
}

async fn handle_playlistfind_command(state: &AppState, tag: &str, value: &str) -> String {
    // Search queue for exact matches
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    for item in queue.items() {
        let matches = match tag.to_lowercase().as_str() {
            "artist" => item.song.artist.as_deref() == Some(value),
            "album" => item.song.album.as_deref() == Some(value),
            "title" => item.song.title.as_deref() == Some(value),
            "genre" => item.song.genre.as_deref() == Some(value),
            _ => false,
        };

        if matches {
            resp.song(&item.song, Some(item.position), Some(item.id));
        }
    }
    resp.ok()
}

async fn handle_playlistsearch_command(state: &AppState, tag: &str, value: &str) -> String {
    // Case-insensitive search in queue
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();
    let value_lower = value.to_lowercase();

    for item in queue.items() {
        let matches = match tag.to_lowercase().as_str() {
            "artist" => item
                .song
                .artist
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            "album" => item
                .song
                .album
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            "title" => item
                .song
                .title
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            "genre" => item
                .song
                .genre
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            _ => false,
        };

        if matches {
            resp.song(&item.song, Some(item.position), Some(item.id));
        }
    }
    resp.ok()
}

// Database commands
async fn handle_searchcount_command(
    state: &AppState,
    tag: &str,
    value: &str,
    group: Option<&str>,
) -> String {
    // Count search results with optional grouping
    let filters = vec![(tag.to_string(), value.to_string())];
    database::handle_count_command(state, &filters, group).await
}

async fn handle_getfingerprint_command(state: &AppState, uri: &str) -> String {
    // Generate chromaprint fingerprint for audio file
    // Requires chromaprint library - stub for now
    let _ = (state, uri);
    ResponseBuilder::error(50, 0, "getfingerprint", "chromaprint not available")
}

async fn handle_readcomments_command(state: &AppState, uri: &str) -> String {
    // Read file metadata comments
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            if let Ok(Some(song)) = db.get_song_by_path(uri) {
                let mut resp = ResponseBuilder::new();
                if let Some(ref comment) = song.comment {
                    resp.field("comment", comment);
                }
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "readcomments", "No such file")
}

// Playlist queue search commands
