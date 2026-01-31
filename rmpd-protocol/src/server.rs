use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

use crate::parser::{parse_command, Command};
use crate::queue_playback::QueuePlaybackManager;
use crate::response::{Response, ResponseBuilder};
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

/// Resolve relative path to absolute path using music_directory
/// If path is already absolute, return as-is
fn resolve_path(rel_path: &str, music_dir: Option<&str>) -> String {
    // If path is already absolute, return as-is
    if rel_path.starts_with('/') {
        return rel_path.to_string();
    }

    // Otherwise, prepend music_directory
    if let Some(music_dir) = music_dir {
        let music_dir = music_dir.trim_end_matches('/');
        format!("{}/{}", music_dir, rel_path)
    } else {
        // Fallback: return as-is if no music_dir
        rel_path.to_string()
    }
}

/// Convert Unix timestamp to ISO 8601 format (RFC 3339)
fn format_iso8601_timestamp(timestamp: i64) -> String {
    const SECONDS_PER_MINUTE: i64 = 60;
    const SECONDS_PER_HOUR: i64 = 3600;
    const SECONDS_PER_DAY: i64 = 86400;

    let mut days = timestamp / SECONDS_PER_DAY;
    let remaining = timestamp % SECONDS_PER_DAY;
    let hours = remaining / SECONDS_PER_HOUR;
    let minutes = (remaining % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let seconds = remaining % SECONDS_PER_MINUTE;

    // Calculate year starting from 1970
    let mut year = 1970;
    loop {
        let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap_year { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    // Calculate month and day
    let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let days_in_month = if leap_year {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &dim in &days_in_month {
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    let day = days + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

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
            return handle_albumart_command(state, &uri, offset).await;
        }
        Command::ReadPicture { uri, offset } => {
            return handle_readpicture_command(state, &uri, offset).await;
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
        Command::Commands => handle_commands_command().await,
        Command::NotCommands => handle_notcommands_command().await,
        Command::TagTypes { subcommand } => handle_tagtypes_command(subcommand).await,
        Command::UrlHandlers => handle_urlhandlers_command().await,
        Command::Decoders => handle_decoders_command().await,
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

            let mut resp = ResponseBuilder::new();
            resp.stats(artists, albums, songs, uptime, db_playtime, db_update, 0);
            resp.ok()
        }
        Command::ClearError => {
            // Clear the error field in status
            state.status.write().await.error = None;
            ResponseBuilder::new().ok()
        }
        Command::Update { path } | Command::Rescan { path } => {
            handle_update_command(state, path.as_deref()).await
        }
        Command::Find {
            filters,
            sort,
            window,
        } => handle_find_command(state, &filters, sort.as_deref(), window).await,
        Command::Search {
            filters,
            sort,
            window,
        } => handle_search_command(state, &filters, sort.as_deref(), window).await,
        Command::List {
            tag,
            filter_tag,
            filter_value,
            group: _,
        } => handle_list_command(state, &tag, filter_tag.as_deref(), filter_value.as_deref()).await,
        Command::Count { filters, group } => {
            handle_count_command(state, &filters, group.as_deref()).await
        }
        Command::ListAll { path } => handle_listall_command(state, path.as_deref()).await,
        Command::ListAllInfo { path } => handle_listallinfo_command(state, path.as_deref()).await,
        Command::LsInfo { path } => handle_lsinfo_command(state, path.as_deref()).await,
        Command::CurrentSong => handle_currentsong_command(state).await,
        Command::PlaylistInfo { range } => handle_playlistinfo_command(state, range).await,
        Command::Playlist => {
            // Deprecated, same as playlistinfo without range
            handle_playlistinfo_command(state, None).await
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
        Command::Play { position } => handle_play_command(state, position).await,
        Command::Pause { state: pause_state } => handle_pause_command(state, pause_state).await,
        Command::Stop => handle_stop_command(state).await,
        Command::Next => handle_next_command(state).await,
        Command::Previous => handle_previous_command(state).await,
        Command::Seek { position, time } => handle_seek_command(state, position, time).await,
        Command::SeekId { id, time } => handle_seekid_command(state, id, time).await,
        Command::SeekCur { time, relative } => handle_seekcur_command(state, time, relative).await,
        Command::SetVol { volume } => handle_setvol_command(state, volume).await,
        Command::Add { uri, position } => handle_add_command(state, &uri, position).await,
        Command::Clear => handle_clear_command(state).await,
        Command::Delete { target } => handle_delete_command(state, target).await,
        Command::DeleteId { id } => handle_deleteid_command(state, id).await,
        Command::AddId { uri, position } => handle_addid_command(state, &uri, position).await,
        Command::PlayId { id } => handle_playid_command(state, id).await,
        Command::MoveId { id, to } => handle_moveid_command(state, id, to).await,
        Command::Swap { pos1, pos2 } => handle_swap_command(state, pos1, pos2).await,
        Command::SwapId { id1, id2 } => handle_swapid_command(state, id1, id2).await,
        Command::Move { from, to } => handle_move_command(state, from, to).await,
        Command::Shuffle { range } => handle_shuffle_command(state, range).await,
        Command::PlaylistId { id } => handle_playlistid_command(state, id).await,
        Command::Password { password: _ } => {
            // No password protection implemented yet
            ResponseBuilder::new().ok()
        }
        Command::AlbumArt { .. } | Command::ReadPicture { .. } => {
            // Already handled at the beginning of the function
            unreachable!()
        }
        Command::Unknown(cmd) => ResponseBuilder::error(5, 0, &cmd, "unknown command"),
        Command::Repeat { enabled } => handle_repeat_command(state, enabled).await,
        Command::Random { enabled } => handle_random_command(state, enabled).await,
        Command::Single { mode } => handle_single_command(state, &mode).await,
        Command::Consume { mode } => handle_consume_command(state, &mode).await,
        Command::Crossfade { seconds } => handle_crossfade_command(state, seconds).await,
        Command::Volume { change } => handle_volume_command(state, change).await,
        Command::GetVol => {
            let status = state.status.read().await;
            let mut resp = ResponseBuilder::new();
            resp.field("volume", status.volume.to_string());
            resp.ok()
        }
        Command::ReplayGainMode { mode } => handle_replaygain_mode_command(state, &mode).await,
        Command::ReplayGainStatus => handle_replaygain_status_command(state).await,
        Command::BinaryLimit { size } => {
            // Set binary limit (for large responses like images)
            // Store in connection state if needed, for now just acknowledge
            let _ = size;
            ResponseBuilder::new().ok()
        }
        Command::Protocol { subcommand } => handle_protocol_command(subcommand).await,
        // Stored playlists
        Command::Save { name, mode } => handle_save_command(state, &name, mode).await,
        Command::Load {
            name,
            range,
            position,
        } => handle_load_command(state, &name, range, position).await,
        Command::ListPlaylists => handle_listplaylists_command(state).await,
        Command::ListPlaylist { name, range } => {
            handle_listplaylist_command(state, &name, range).await
        }
        Command::ListPlaylistInfo { name, range } => {
            handle_listplaylistinfo_command(state, &name, range).await
        }
        Command::PlaylistAdd {
            name,
            uri,
            position,
        } => handle_playlistadd_command(state, &name, &uri, position).await,
        Command::PlaylistClear { name } => handle_playlistclear_command(state, &name).await,
        Command::PlaylistDelete { name, position } => {
            handle_playlistdelete_command(state, &name, position).await
        }
        Command::PlaylistMove { name, from, to } => {
            handle_playlistmove_command(state, &name, from, to).await
        }
        Command::Rm { name } => handle_rm_command(state, &name).await,
        Command::Rename { from, to } => handle_rename_command(state, &from, &to).await,
        Command::SearchPlaylist { name, tag, value } => {
            handle_searchplaylist_command(state, &name, &tag, &value).await
        }
        Command::PlaylistLength { name } => handle_playlistlength_command(state, &name).await,
        // Output control
        Command::Outputs => handle_outputs_command(state).await,
        Command::EnableOutput { id } => handle_enableoutput_command(state, id).await,
        Command::DisableOutput { id } => handle_disableoutput_command(state, id).await,
        Command::ToggleOutput { id } => handle_toggleoutput_command(state, id).await,
        Command::OutputSet { id, name, value } => {
            handle_outputset_command(state, id, &name, &value).await
        }
        // Advanced database
        Command::SearchAdd { tag, value } => handle_searchadd_command(state, &tag, &value).await,
        Command::SearchAddPl { name, tag, value } => {
            handle_searchaddpl_command(state, &name, &tag, &value).await
        }
        Command::FindAdd { tag, value } => handle_findadd_command(state, &tag, &value).await,
        Command::ListFiles { uri } => handle_listfiles_command(state, uri.as_deref()).await,
        Command::SearchCount { tag, value, group } => {
            handle_searchcount_command(state, &tag, &value, group.as_deref()).await
        }
        Command::GetFingerprint { uri } => handle_getfingerprint_command(state, &uri).await,
        Command::ReadComments { uri } => handle_readcomments_command(state, &uri).await,
        // Stickers
        Command::StickerGet { uri, name } => handle_sticker_get_command(state, &uri, &name).await,
        Command::StickerSet { uri, name, value } => {
            handle_sticker_set_command(state, &uri, &name, &value).await
        }
        Command::StickerDelete { uri, name } => {
            handle_sticker_delete_command(state, &uri, name.as_deref()).await
        }
        Command::StickerList { uri } => handle_sticker_list_command(state, &uri).await,
        Command::StickerFind { uri, name, value } => {
            handle_sticker_find_command(state, &uri, &name, value.as_deref()).await
        }
        Command::StickerInc { uri, name, delta } => {
            handle_sticker_inc_command(state, &uri, &name, delta).await
        }
        Command::StickerDec { uri, name, delta } => {
            handle_sticker_dec_command(state, &uri, &name, delta).await
        }
        Command::StickerNames { uri } => handle_sticker_names_command(state, uri.as_deref()).await,
        Command::StickerTypes => handle_sticker_types_command().await,
        Command::StickerNamesTypes { uri } => {
            handle_sticker_namestypes_command(state, uri.as_deref()).await
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
        Command::MixRampDb { decibels } => handle_mixrampdb_command(state, decibels).await,
        Command::MixRampDelay { seconds } => handle_mixrampdelay_command(state, seconds).await,
        _ => {
            // Unimplemented commands
            ResponseBuilder::error(5, 0, "command", "not yet implemented")
        }
    };

    Response::Text(response_str)
}

async fn handle_find_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "find", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("database error: {}", e)),
    };

    if filters.is_empty() {
        return ResponseBuilder::error(2, 0, "find", "missing arguments");
    }

    // Check if this is a filter expression (starts with '(')
    let mut songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => match db.find_songs_filter(&filter) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e))
                }
            },
            Err(e) => {
                return ResponseBuilder::error(2, 0, "find", &format!("filter parse error: {}", e))
            }
        }
    } else if filters.len() == 1 {
        // Simple single tag/value search
        match db.find_songs(&filters[0].0, &filters[0].1) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e)),
        }
    } else {
        // Multiple tag/value pairs - build filter expression with AND
        use rmpd_core::filter::{CompareOp, FilterExpression};
        let mut expr = FilterExpression::Compare {
            tag: filters[0].0.clone(),
            op: CompareOp::Equal,
            value: filters[0].1.clone(),
        };

        for filter in &filters[1..] {
            let next_expr = FilterExpression::Compare {
                tag: filter.0.clone(),
                op: CompareOp::Equal,
                value: filter.1.clone(),
            };
            expr = FilterExpression::And(Box::new(expr), Box::new(next_expr));
        }

        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e)),
        }
    };

    // Apply sorting if requested
    if let Some(sort_tag) = sort {
        songs.sort_by(|a, b| {
            let a_val = get_tag_value(a, sort_tag);
            let b_val = get_tag_value(b, sort_tag);
            a_val.cmp(&b_val)
        });
    }

    // Apply window filtering if requested
    let filtered = if let Some((start, end)) = window {
        let start_idx = start as usize;
        let end_idx = end.min(songs.len() as u32) as usize;
        if start_idx < songs.len() {
            &songs[start_idx..end_idx]
        } else {
            &[]
        }
    } else {
        &songs[..]
    };

    let mut resp = ResponseBuilder::new();
    for song in filtered {
        resp.song(song, None, None);
    }
    resp.ok()
}

// Helper function to get tag value for sorting
fn get_tag_value(song: &rmpd_core::song::Song, tag: &str) -> String {
    match tag.to_lowercase().as_str() {
        "artist" => song.artist.clone().unwrap_or_default(),
        "album" => song.album.clone().unwrap_or_default(),
        "albumartist" => song.album_artist.clone().unwrap_or_default(),
        "title" => song.title.clone().unwrap_or_default(),
        "track" => song.track.map(|t| t.to_string()).unwrap_or_default(),
        "date" => song.date.clone().unwrap_or_default(),
        "genre" => song.genre.clone().unwrap_or_default(),
        "composer" => song.composer.clone().unwrap_or_default(),
        "performer" => song.performer.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

async fn handle_search_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "search", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "search", &format!("database error: {}", e))
        }
    };

    if filters.is_empty() {
        return ResponseBuilder::error(2, 0, "search", "missing arguments");
    }

    // Check if this is a filter expression (starts with '(')
    let mut songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => match db.find_songs_filter(&filter) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e))
                }
            },
            Err(e) => {
                return ResponseBuilder::error(
                    2,
                    0,
                    "search",
                    &format!("filter parse error: {}", e),
                )
            }
        }
    } else if filters.len() == 1 {
        let tag = &filters[0].0;
        let value = &filters[0].1;

        if tag.eq_ignore_ascii_case("any") {
            // Use FTS for "any" tag
            match db.search_songs(value) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "search", &format!("search error: {}", e))
                }
            }
        } else {
            // Partial match using LIKE
            match db.find_songs(tag, value) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e))
                }
            }
        }
    } else {
        // Multiple tag/value pairs - build filter expression with AND
        use rmpd_core::filter::{CompareOp, FilterExpression};
        let mut expr = FilterExpression::Compare {
            tag: filters[0].0.clone(),
            op: CompareOp::Equal,
            value: filters[0].1.clone(),
        };

        for filter in &filters[1..] {
            let next_expr = FilterExpression::Compare {
                tag: filter.0.clone(),
                op: CompareOp::Equal,
                value: filter.1.clone(),
            };
            expr = FilterExpression::And(Box::new(expr), Box::new(next_expr));
        }

        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e))
            }
        }
    };

    // Apply sorting if requested
    if let Some(sort_tag) = sort {
        songs.sort_by(|a, b| {
            let a_val = get_tag_value(a, sort_tag);
            let b_val = get_tag_value(b, sort_tag);
            a_val.cmp(&b_val)
        });
    }

    // Apply window filtering if requested
    let filtered = if let Some((start, end)) = window {
        let start_idx = start as usize;
        let end_idx = end.min(songs.len() as u32) as usize;
        if start_idx < songs.len() {
            &songs[start_idx..end_idx]
        } else {
            &[]
        }
    } else {
        &songs[..]
    };

    let mut resp = ResponseBuilder::new();
    for song in filtered {
        resp.song(song, None, None);
    }
    resp.ok()
}

async fn handle_list_command(
    state: &AppState,
    tag: &str,
    filter_tag: Option<&str>,
    filter_value: Option<&str>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "list", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("database error: {}", e)),
    };

    // If filter is provided, get filtered results
    let values = if let (Some(ft), Some(fv)) = (filter_tag, filter_value) {
        match db.list_filtered(tag, ft, fv) {
            Ok(v) => v,
            Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("query error: {}", e)),
        }
    } else {
        // No filter, list all values
        let result = match tag.to_lowercase().as_str() {
            "artist" => db.list_artists(),
            "album" => db.list_albums(),
            "albumartist" => db.list_album_artists(),
            "genre" => db.list_genres(),
            _ => return ResponseBuilder::error(2, 0, "list", &format!("unsupported tag: {}", tag)),
        };

        match result {
            Ok(v) => v,
            Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("query error: {}", e)),
        }
    };

    let mut resp = ResponseBuilder::new();
    let tag_key = match tag.to_lowercase().as_str() {
        "artist" => "Artist",
        "album" => "Album",
        "albumartist" => "AlbumArtist",
        "genre" => "Genre",
        _ => tag,
    };

    for value in values {
        resp.field(tag_key, value);
    }
    resp.ok()
}

async fn handle_count_command(
    state: &AppState,
    filters: &[(String, String)],
    group: Option<&str>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "count", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "count", &format!("database error: {}", e)),
    };

    if filters.is_empty() {
        return ResponseBuilder::error(2, 0, "count", "missing arguments");
    }

    // Get songs based on filters
    let songs = if filters.len() == 1 {
        match db.find_songs(&filters[0].0, &filters[0].1) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "count", &format!("query error: {}", e))
            }
        }
    } else {
        // Multiple filters - build AND expression
        use rmpd_core::filter::{CompareOp, FilterExpression};
        let mut expr = FilterExpression::Compare {
            tag: filters[0].0.clone(),
            op: CompareOp::Equal,
            value: filters[0].1.clone(),
        };

        for filter in &filters[1..] {
            let next_expr = FilterExpression::Compare {
                tag: filter.0.clone(),
                op: CompareOp::Equal,
                value: filter.1.clone(),
            };
            expr = FilterExpression::And(Box::new(expr), Box::new(next_expr));
        }

        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "count", &format!("query error: {}", e))
            }
        }
    };

    let mut resp = ResponseBuilder::new();

    if let Some(group_tag) = group {
        // Group by specified tag
        use std::collections::HashMap;
        let mut groups: HashMap<String, (usize, u64)> = HashMap::new();

        for song in &songs {
            let group_value = get_tag_value(song, group_tag);
            let entry = groups.entry(group_value.clone()).or_insert((0, 0));
            entry.0 += 1;
            if let Some(duration) = song.duration {
                entry.1 += duration.as_secs();
            }
        }

        for (value, (count, playtime)) in groups {
            resp.field(group_tag, &value);
            resp.field("songs", count);
            resp.field("playtime", playtime);
        }
    } else {
        // No grouping - return totals
        let total_duration: u64 = songs
            .iter()
            .filter_map(|s| s.duration)
            .map(|d| d.as_secs())
            .sum();

        resp.field("songs", songs.len());
        resp.field("playtime", total_duration);
    }

    resp.ok()
}

async fn handle_play_command(state: &AppState, position: Option<u32>) -> String {
    let queue = state.queue.read().await;

    // Get song to play and track the actual position
    let (song, actual_position) = if let Some(pos) = position {
        // Play specific position
        if let Some(item) = queue.get(pos) {
            (item.song.clone(), Some((pos, item.id)))
        } else {
            return ResponseBuilder::error(50, 0, "play", "No such song");
        }
    } else {
        // Resume or play first song
        let current_song = state.engine.read().await.get_current_song().await;
        if let Some(song) = current_song {
            // Resuming - keep existing position if set
            let pos = state.status.read().await.current_song;
            (song, pos.map(|p| (p.position, p.id)))
        } else if let Some(item) = queue.get(0) {
            // Play first song
            (item.song.clone(), Some((0, item.id)))
        } else {
            return ResponseBuilder::error(50, 0, "play", "No songs in queue");
        }
    };

    drop(queue);

    // Resolve relative path to absolute for playback
    let mut playback_song = song.clone();
    let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
    playback_song.path = absolute_path.into();

    // Start playback with resolved path
    match state.engine.write().await.play(playback_song).await {
        Ok(_) => {
            // Update status immediately (event will also update but that's idempotent)
            let mut status = state.status.write().await;
            status.state = rmpd_core::state::PlayerState::Play;
            status.elapsed = Some(std::time::Duration::ZERO);
            status.duration = song.duration;
            status.bitrate = song.bitrate;

            // Set audio format if available
            if let (Some(sr), Some(ch), Some(bps)) =
                (song.sample_rate, song.channels, song.bits_per_sample)
            {
                status.audio_format = Some(rmpd_core::song::AudioFormat {
                    sample_rate: sr,
                    channels: ch,
                    bits_per_sample: bps,
                });
            }

            if let Some((pos, id)) = actual_position {
                status.current_song = Some(rmpd_core::state::QueuePosition { position: pos, id });

                // Set next_song for UI (e.g., Cantata's next button)
                let queue = state.queue.read().await;
                if let Some(next_item) = queue.get(pos + 1) {
                    status.next_song = Some(rmpd_core::state::QueuePosition {
                        position: pos + 1,
                        id: next_item.id,
                    });
                } else {
                    status.next_song = None;
                }
            }
            drop(status);

            // Emit events to notify idle clients
            debug!("Emitting PlayerStateChanged(Play) and SongChanged events");
            state
                .event_bus
                .emit(rmpd_core::event::Event::PlayerStateChanged(
                    rmpd_core::state::PlayerState::Play,
                ));
            state
                .event_bus
                .emit(rmpd_core::event::Event::SongChanged(Some(song)));

            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "play", &format!("Playback error: {}", e)),
    }
}

async fn handle_pause_command(state: &AppState, pause_state: Option<bool>) -> String {
    info!("Pause command received: pause_state={:?}", pause_state);

    // Get current state lock-free using atomic (no engine lock needed!)
    let current_state_u8 = state.atomic_state.load(std::sync::atomic::Ordering::SeqCst);
    let current_state = match current_state_u8 {
        0 => rmpd_core::state::PlayerState::Stop,
        1 => rmpd_core::state::PlayerState::Play,
        2 => rmpd_core::state::PlayerState::Pause,
        _ => rmpd_core::state::PlayerState::Stop,
    };

    info!("Current state (atomic, no locks): {:?}", current_state);

    let should_pause =
        pause_state.unwrap_or_else(|| current_state == rmpd_core::state::PlayerState::Play);
    let is_currently_paused = current_state == rmpd_core::state::PlayerState::Pause;

    // If already in desired state, do nothing
    if should_pause == is_currently_paused {
        info!("Already in desired state, returning OK");
        return ResponseBuilder::new().ok();
    }

    info!("Acquiring engine write lock...");
    // Set pause state
    let result = if pause_state.is_some() {
        // Explicit pause state given - use set_pause
        state.engine.write().await.set_pause(should_pause).await
    } else {
        // No explicit state - toggle
        state.engine.write().await.pause().await
    };

    match result {
        Ok(_) => {
            info!("Engine pause completed, updating status...");
            // Read the actual state from atomic (engine might not have changed it)
            let actual_state_u8 = state.atomic_state.load(std::sync::atomic::Ordering::SeqCst);
            let actual_state = match actual_state_u8 {
                0 => rmpd_core::state::PlayerState::Stop,
                1 => rmpd_core::state::PlayerState::Play,
                2 => rmpd_core::state::PlayerState::Pause,
                _ => rmpd_core::state::PlayerState::Stop,
            };

            // Update status to match actual atomic state
            let mut status = state.status.write().await;
            status.state = actual_state;
            drop(status);

            // Emit event to notify idle clients
            debug!("Emitting PlayerStateChanged({:?}) event", actual_state);
            state
                .event_bus
                .emit(rmpd_core::event::Event::PlayerStateChanged(actual_state));

            info!(
                "Pause completed successfully, state is now: {:?}",
                actual_state
            );
            ResponseBuilder::new().ok()
        }
        Err(e) => {
            error!("Pause failed: {}", e);
            ResponseBuilder::error(50, 0, "pause", &format!("Pause error: {}", e))
        }
    }
}

async fn handle_stop_command(state: &AppState) -> String {
    info!("Stop command received");
    info!("Acquiring engine write lock for stop...");
    match state.engine.write().await.stop().await {
        Ok(_) => {
            // Update status after engine stops
            let mut status = state.status.write().await;
            status.state = rmpd_core::state::PlayerState::Stop;
            status.current_song = None;
            status.next_song = None;
            drop(status);

            // Emit event to notify idle clients
            debug!("Emitting PlayerStateChanged(Stop) event");
            state
                .event_bus
                .emit(rmpd_core::event::Event::PlayerStateChanged(
                    rmpd_core::state::PlayerState::Stop,
                ));

            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "stop", &format!("Stop error: {}", e)),
    }
}

async fn handle_next_command(state: &AppState) -> String {
    let queue = state.queue.read().await;
    let status = state.status.read().await;

    let next_pos = if let Some(current) = status.current_song {
        current.position + 1
    } else {
        0
    };

    if let Some(item) = queue.get(next_pos) {
        let song = item.song.clone();
        let item_id = item.id;
        drop(queue);
        drop(status);

        // Resolve relative path to absolute for playback
        let mut playback_song = song.clone();
        let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
        playback_song.path = absolute_path.into();

        match state.engine.write().await.play(playback_song).await {
            Ok(_) => {
                let mut status = state.status.write().await;
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: next_pos,
                    id: item_id,
                });

                // Set next_song for UI (e.g., Cantata's next button)
                let queue = state.queue.read().await;
                if let Some(next_item) = queue.get(next_pos + 1) {
                    status.next_song = Some(rmpd_core::state::QueuePosition {
                        position: next_pos + 1,
                        id: next_item.id,
                    });
                } else {
                    status.next_song = None;
                }

                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(50, 0, "next", &format!("Playback error: {}", e)),
        }
    } else {
        ResponseBuilder::error(50, 0, "next", "No next song")
    }
}

async fn handle_previous_command(state: &AppState) -> String {
    let queue = state.queue.read().await;
    let status = state.status.read().await;

    let prev_pos = if let Some(current) = status.current_song {
        if current.position > 0 {
            current.position - 1
        } else {
            return ResponseBuilder::error(50, 0, "previous", "Already at first song");
        }
    } else {
        0
    };

    if let Some(item) = queue.get(prev_pos) {
        let song = item.song.clone();
        let item_id = item.id;
        drop(queue);
        drop(status);

        // Resolve relative path to absolute for playback
        let mut playback_song = song.clone();
        let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
        playback_song.path = absolute_path.into();

        match state.engine.write().await.play(playback_song).await {
            Ok(_) => {
                let mut status = state.status.write().await;
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: prev_pos,
                    id: item_id,
                });

                // Set next_song for UI (e.g., Cantata's next button)
                let queue = state.queue.read().await;
                if let Some(next_item) = queue.get(prev_pos + 1) {
                    status.next_song = Some(rmpd_core::state::QueuePosition {
                        position: prev_pos + 1,
                        id: next_item.id,
                    });
                } else {
                    status.next_song = None;
                }

                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(50, 0, "previous", &format!("Playback error: {}", e)),
        }
    } else {
        ResponseBuilder::error(50, 0, "previous", "No previous song")
    }
}

async fn handle_seek_command(state: &AppState, position: u32, time: f64) -> String {
    // Get song at position
    let queue = state.queue.read().await;
    let status = state.status.read().await;

    // Check if this is the current song
    if let Some(current) = status.current_song {
        if current.position == position {
            drop(queue);
            drop(status);
            // Seek in current song
            match state.engine.read().await.seek(time).await {
                Ok(_) => {
                    // Update status elapsed time
                    state.status.write().await.elapsed =
                        Some(std::time::Duration::from_secs_f64(time));
                    ResponseBuilder::new().ok()
                }
                Err(e) => ResponseBuilder::error(50, 0, "seek", &format!("Seek failed: {}", e)),
            }
        } else {
            ResponseBuilder::error(50, 0, "seek", "Can only seek in current song")
        }
    } else {
        ResponseBuilder::error(50, 0, "seek", "Not playing")
    }
}

async fn handle_seekid_command(state: &AppState, id: u32, time: f64) -> String {
    let status = state.status.read().await;

    // Check if this is the current song
    if let Some(current) = status.current_song {
        if current.id == id {
            drop(status);
            // Seek in current song
            match state.engine.read().await.seek(time).await {
                Ok(_) => {
                    // Update status elapsed time
                    state.status.write().await.elapsed =
                        Some(std::time::Duration::from_secs_f64(time));
                    ResponseBuilder::new().ok()
                }
                Err(e) => ResponseBuilder::error(50, 0, "seekid", &format!("Seek failed: {}", e)),
            }
        } else {
            ResponseBuilder::error(50, 0, "seekid", "Can only seek in current song")
        }
    } else {
        ResponseBuilder::error(50, 0, "seekid", "Not playing")
    }
}

async fn handle_seekcur_command(state: &AppState, time: f64, relative: bool) -> String {
    let status = state.status.read().await;

    if status.current_song.is_some() {
        let current_elapsed = status
            .elapsed
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs_f64();
        drop(status);

        // Calculate actual seek position
        let seek_position = if relative {
            // Relative seek: add to current position
            (current_elapsed + time).max(0.0)
        } else {
            // Absolute seek
            time.max(0.0)
        };

        // Seek in current song
        match state.engine.read().await.seek(seek_position).await {
            Ok(_) => {
                // Update status elapsed time
                state.status.write().await.elapsed =
                    Some(std::time::Duration::from_secs_f64(seek_position));
                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(50, 0, "seekcur", &format!("Seek failed: {}", e)),
        }
    } else {
        ResponseBuilder::error(50, 0, "seekcur", "Not playing")
    }
}

async fn handle_setvol_command(state: &AppState, volume: u8) -> String {
    match state.engine.write().await.set_volume(volume).await {
        Ok(_) => {
            let mut status = state.status.write().await;
            status.volume = volume;
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "setvol", &format!("Volume error: {}", e)),
    }
}

async fn handle_add_command(state: &AppState, uri: &str, position: Option<u32>) -> String {
    debug!("Add command received with URI: [{}]", uri);
    // Get song from database
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "add", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "add", &format!("database error: {}", e)),
    };

    let song = match db.get_song_by_path(uri) {
        Ok(Some(s)) => s,
        Ok(None) => return ResponseBuilder::error(50, 0, "add", "song not found in database"),
        Err(e) => return ResponseBuilder::error(50, 0, "add", &format!("query error: {}", e)),
    };

    // Add to queue at specified position or at end
    let id = state.queue.write().await.add_at(song, position);

    // Update status to reflect playlist changes
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = state.queue.read().await.len() as u32;
    drop(status); // Release the lock

    let mut resp = ResponseBuilder::new();
    resp.field("Id", id);
    resp.ok()
}

async fn handle_clear_command(state: &AppState) -> String {
    state.queue.write().await.clear();
    state.engine.write().await.stop().await.ok();

    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = 0;
    status.current_song = None;
    status.next_song = None;

    ResponseBuilder::new().ok()
}

async fn handle_delete_command(state: &AppState, target: crate::parser::DeleteTarget) -> String {
    use crate::parser::DeleteTarget;

    match target {
        DeleteTarget::Position(position) => {
            if state.queue.write().await.delete(position).is_some() {
                let mut status = state.status.write().await;
                status.playlist_version += 1;
                status.playlist_length = state.queue.read().await.len() as u32;
                ResponseBuilder::new().ok()
            } else {
                ResponseBuilder::error(50, 0, "delete", "No such song")
            }
        }
        DeleteTarget::Range(start, end) => {
            // Delete songs in range [start, end) (exclusive end)
            let mut queue = state.queue.write().await;
            let mut deleted_count = 0;

            // Delete from highest to lowest to avoid position shifts
            for pos in (start..end).rev() {
                if queue.delete(pos).is_some() {
                    deleted_count += 1;
                }
            }

            if deleted_count > 0 {
                let mut status = state.status.write().await;
                status.playlist_version += 1;
                status.playlist_length = queue.len() as u32;
                ResponseBuilder::new().ok()
            } else {
                ResponseBuilder::error(50, 0, "delete", "No such songs in range")
            }
        }
    }
}

async fn handle_update_command(state: &AppState, _path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p.clone(),
        None => return ResponseBuilder::error(50, 0, "update", "database not configured"),
    };

    let music_dir = match &state.music_dir {
        Some(p) => p.clone(),
        None => return ResponseBuilder::error(50, 0, "update", "music directory not configured"),
    };

    let event_bus = state.event_bus.clone();

    // Spawn background scanning task (blocking task since scan is synchronous)
    tokio::task::spawn_blocking(move || {
        info!("Starting library update");

        match rmpd_library::Database::open(&db_path) {
            Ok(db) => {
                let scanner = rmpd_library::Scanner::new(event_bus.clone());
                match scanner.scan_directory(&db, std::path::Path::new(&music_dir)) {
                    Ok(stats) => {
                        info!(
                            "Library scan complete: {} scanned, {} added, {} updated, {} errors",
                            stats.scanned, stats.added, stats.updated, stats.errors
                        );
                    }
                    Err(e) => {
                        error!("Library scan error: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Failed to open database: {}", e);
            }
        }
    });

    // Return update job ID
    let mut resp = ResponseBuilder::new();
    resp.field("updating_db", 1);
    resp.ok()
}

async fn handle_albumart_command(state: &AppState, uri: &str, offset: usize) -> Response {
    info!("AlbumArt command: uri=[{}], offset={}", uri, offset);

    let db_path = match &state.db_path {
        Some(p) => p,
        None => {
            return Response::Text(ResponseBuilder::error(
                50,
                0,
                "albumart",
                "database not configured",
            ))
        }
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return Response::Text(ResponseBuilder::error(
                50,
                0,
                "albumart",
                &format!("database error: {}", e),
            ))
        }
    };

    // Resolve relative path to absolute path
    let absolute_path = if uri.starts_with('/') {
        // Already absolute
        debug!("Using absolute path: {}", uri);
        uri.to_string()
    } else {
        // Relative to music directory
        match &state.music_dir {
            Some(music_dir) => {
                let path = format!("{}/{}", music_dir, uri);
                debug!("Resolved relative path: {} -> {}", uri, path);
                path
            }
            None => {
                return Response::Text(ResponseBuilder::error(
                    50,
                    0,
                    "albumart",
                    "music directory not configured",
                ))
            }
        }
    };

    let extractor = rmpd_library::AlbumArtExtractor::new(db);

    // Pass both: relative URI for cache key, absolute path for file reading
    match extractor.get_artwork(uri, &absolute_path, offset) {
        Ok(Some(artwork)) => {
            // Binary response with proper format
            let mut resp = ResponseBuilder::new();
            resp.field("size", artwork.total_size);
            resp.field("type", &artwork.mime_type);
            resp.binary_field("binary", &artwork.data);
            Response::Binary(resp.to_binary_response())
        }
        Ok(None) => {
            // When offset is past the end of data, return OK (not an error)
            // This is the correct MPD protocol behavior for chunked transfers
            Response::Text(ResponseBuilder::new().ok())
        }
        Err(e) => Response::Text(ResponseBuilder::error(
            50,
            0,
            "albumart",
            &format!("Error: {}", e),
        )),
    }
}

async fn handle_readpicture_command(state: &AppState, uri: &str, offset: usize) -> Response {
    // readpicture is similar to albumart but returns any embedded picture
    // For now, we'll use the same implementation
    handle_albumart_command(state, uri, offset).await
}

// Queue ID-based operations
async fn handle_addid_command(state: &AppState, uri: &str, position: Option<u32>) -> String {
    debug!(
        "AddId command received with URI: [{}], position: {:?}",
        uri, position
    );

    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "addid", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "addid", &format!("database error: {}", e)),
    };

    let song = match db.get_song_by_path(uri) {
        Ok(Some(s)) => s,
        Ok(None) => return ResponseBuilder::error(50, 0, "addid", "song not found in database"),
        Err(e) => return ResponseBuilder::error(50, 0, "addid", &format!("query error: {}", e)),
    };

    // Add to queue at specific position
    let id = state.queue.write().await.add_at(song, position);

    let mut resp = ResponseBuilder::new();
    resp.field("Id", id);
    resp.ok()
}

async fn handle_deleteid_command(state: &AppState, id: u32) -> String {
    if state.queue.write().await.delete_id(id).is_some() {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        status.playlist_length = state.queue.read().await.len() as u32;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "deleteid", "No such song")
    }
}

async fn handle_playid_command(state: &AppState, id: Option<u32>) -> String {
    if let Some(song_id) = id {
        // Play specific song by ID
        let queue = state.queue.read().await;
        if let Some(item) = queue.get_by_id(song_id) {
            let song = item.song.clone();
            let position = item.position;
            drop(queue);

            // Resolve relative path to absolute for playback
            let mut playback_song = song.clone();
            let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
            playback_song.path = absolute_path.into();

            match state.engine.write().await.play(playback_song).await {
                Ok(_) => {
                    let mut status = state.status.write().await;
                    status.state = rmpd_core::state::PlayerState::Play;
                    status.current_song = Some(rmpd_core::state::QueuePosition {
                        position,
                        id: song_id,
                    });

                    // Set next_song for UI (e.g., Cantata's next button)
                    let queue = state.queue.read().await;
                    if let Some(next_item) = queue.get(position + 1) {
                        status.next_song = Some(rmpd_core::state::QueuePosition {
                            position: position + 1,
                            id: next_item.id,
                        });
                    } else {
                        status.next_song = None;
                    }

                    ResponseBuilder::new().ok()
                }
                Err(e) => {
                    ResponseBuilder::error(50, 0, "playid", &format!("Playback error: {}", e))
                }
            }
        } else {
            ResponseBuilder::error(50, 0, "playid", "No such song")
        }
    } else {
        // Resume playback (same as play with no args)
        handle_play_command(state, None).await
    }
}

async fn handle_moveid_command(state: &AppState, id: u32, to: u32) -> String {
    if state.queue.write().await.move_by_id(id, to) {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "moveid", "No such song")
    }
}

async fn handle_move_command(state: &AppState, from: crate::parser::MoveFrom, to: u32) -> String {
    use crate::parser::MoveFrom;

    match from {
        MoveFrom::Position(from_pos) => {
            if state.queue.write().await.move_item(from_pos, to) {
                let mut status = state.status.write().await;
                status.playlist_version += 1;
                ResponseBuilder::new().ok()
            } else {
                ResponseBuilder::error(50, 0, "move", "Invalid position")
            }
        }
        MoveFrom::Range(start, end) => {
            // Move range of songs [start, end) to position
            // MPD semantics: move each song individually to maintain order
            let mut queue = state.queue.write().await;

            if start >= end || start >= queue.len() as u32 {
                return ResponseBuilder::error(50, 0, "move", "Invalid range");
            }

            let range_size = end.saturating_sub(start);

            // Move songs one by one
            // If moving to a position before the range, move from start to end
            // If moving to a position after the range, move from end-1 to start
            if to <= start {
                // Moving up in the queue
                for i in 0..range_size.min(queue.len() as u32 - start) {
                    if !queue.move_item(start, to + i) {
                        return ResponseBuilder::error(50, 0, "move", "Invalid position");
                    }
                }
            } else {
                // Moving down in the queue
                let actual_end = end.min(queue.len() as u32);
                for _ in 0..(actual_end - start) {
                    if !queue.move_item(start, to.saturating_sub(1)) {
                        return ResponseBuilder::error(50, 0, "move", "Invalid position");
                    }
                }
            }

            let mut status = state.status.write().await;
            status.playlist_version += 1;
            ResponseBuilder::new().ok()
        }
    }
}

async fn handle_swap_command(state: &AppState, pos1: u32, pos2: u32) -> String {
    if state.queue.write().await.swap(pos1, pos2) {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "swap", "Invalid position")
    }
}

async fn handle_swapid_command(state: &AppState, id1: u32, id2: u32) -> String {
    if state.queue.write().await.swap_by_id(id1, id2) {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "swapid", "No such song")
    }
}

async fn handle_shuffle_command(state: &AppState, range: Option<(u32, u32)>) -> String {
    if let Some((start, end)) = range {
        state.queue.write().await.shuffle_range(start, end);
    } else {
        state.queue.write().await.shuffle();
    }
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

async fn handle_playlistid_command(state: &AppState, id: Option<u32>) -> String {
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if let Some(song_id) = id {
        // Get specific song by ID
        if let Some(item) = queue.get_by_id(song_id) {
            resp.song(&item.song, Some(item.position), Some(item.id));
        } else {
            return ResponseBuilder::error(50, 0, "playlistid", "No such song");
        }
    } else {
        // Get all songs with IDs
        for item in queue.items() {
            resp.song(&item.song, Some(item.position), Some(item.id));
        }
    }

    resp.ok()
}

// Playback options
async fn handle_repeat_command(state: &AppState, enabled: bool) -> String {
    state.status.write().await.repeat = enabled;
    ResponseBuilder::new().ok()
}

async fn handle_random_command(state: &AppState, enabled: bool) -> String {
    state.status.write().await.random = enabled;
    ResponseBuilder::new().ok()
}

async fn handle_single_command(state: &AppState, mode: &str) -> String {
    let single_mode = match mode {
        "0" => rmpd_core::state::SingleMode::Off,
        "1" => rmpd_core::state::SingleMode::On,
        "oneshot" => rmpd_core::state::SingleMode::Oneshot,
        _ => return ResponseBuilder::error(2, 0, "single", "Invalid mode"),
    };
    state.status.write().await.single = single_mode;
    ResponseBuilder::new().ok()
}

async fn handle_consume_command(state: &AppState, mode: &str) -> String {
    let consume_mode = match mode {
        "0" => rmpd_core::state::ConsumeMode::Off,
        "1" => rmpd_core::state::ConsumeMode::On,
        "oneshot" => rmpd_core::state::ConsumeMode::Oneshot,
        _ => return ResponseBuilder::error(2, 0, "consume", "Invalid mode"),
    };
    state.status.write().await.consume = consume_mode;
    ResponseBuilder::new().ok()
}

async fn handle_crossfade_command(state: &AppState, seconds: u32) -> String {
    state.status.write().await.crossfade = seconds;
    ResponseBuilder::new().ok()
}

async fn handle_volume_command(state: &AppState, change: i8) -> String {
    let current_vol = state.status.read().await.volume;
    let new_vol = (current_vol as i16 + change as i16).clamp(0, 100) as u8;

    match state.engine.write().await.set_volume(new_vol).await {
        Ok(_) => {
            state.status.write().await.volume = new_vol;
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "volume", &format!("Volume error: {}", e)),
    }
}

// Queue inspection
async fn handle_currentsong_command(state: &AppState) -> String {
    let status = state.status.read().await;
    let queue = state.queue.read().await;

    if let Some(current) = status.current_song {
        if let Some(item) = queue.get(current.position) {
            let mut resp = ResponseBuilder::new();
            resp.song(&item.song, Some(current.position), Some(current.id));
            return resp.ok();
        }
    }

    // No current song
    ResponseBuilder::new().ok()
}

async fn handle_playlistinfo_command(state: &AppState, range: Option<(u32, u32)>) -> String {
    let queue = state.queue.read().await;
    let items = queue.items();
    let mut resp = ResponseBuilder::new();

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
        resp.song(&item.song, Some(item.position), Some(item.id));
    }

    resp.ok()
}

// Browsing commands
async fn handle_lsinfo_command(state: &AppState, path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "lsinfo", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "lsinfo", &format!("database error: {}", e))
        }
    };

    let path_str = path.unwrap_or("");

    // Get directory listing
    match db.list_directory(path_str) {
        Ok(listing) => {
            let mut resp = ResponseBuilder::new();
            let music_dir = state.music_dir.as_deref();

            // List subdirectories first
            for dir in &listing.directories {
                let display_dir = strip_music_dir_prefix(dir, music_dir);
                resp.field("directory", display_dir);
            }

            // Then list songs
            for song in &listing.songs {
                // Create a modified song with stripped path for display
                let display_path = strip_music_dir_prefix(song.path.as_str(), music_dir);
                let mut display_song = song.clone();
                display_song.path = display_path.into();
                resp.song(&display_song, None, None);
            }

            // For root directory, also list playlists
            if path_str.is_empty() || path_str == "/" {
                if let Ok(playlists) = db.list_playlists() {
                    for playlist in &playlists {
                        resp.field("playlist", &playlist.name);
                        let timestamp_str = format_iso8601_timestamp(playlist.last_modified);
                        resp.field("Last-Modified", &timestamp_str);
                    }
                }
            }

            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "lsinfo", &format!("Error: {}", e)),
    }
}

async fn handle_listall_command(state: &AppState, path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listall", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listall", &format!("database error: {}", e))
        }
    };

    let path_str = path.unwrap_or("");

    match db.list_directory_recursive(path_str) {
        Ok(songs) => {
            let mut resp = ResponseBuilder::new();
            for song in &songs {
                resp.field("file", &song.path);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listall", &format!("Error: {}", e)),
    }
}

async fn handle_listallinfo_command(state: &AppState, path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listallinfo", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listallinfo", &format!("database error: {}", e))
        }
    };

    let path_str = path.unwrap_or("");

    match db.list_directory_recursive(path_str) {
        Ok(songs) => {
            let mut resp = ResponseBuilder::new();
            for song in &songs {
                resp.song(song, None, None);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listallinfo", &format!("Error: {}", e)),
    }
}

// Stored playlist commands
async fn handle_save_command(
    state: &AppState,
    name: &str,
    mode: Option<crate::parser::SaveMode>,
) -> String {
    use crate::parser::SaveMode;

    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "save", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "save", &format!("database error: {}", e)),
    };

    // Get current queue
    let queue = state.queue.read().await;
    let songs: Vec<_> = queue.items().iter().map(|item| item.song.clone()).collect();
    drop(queue);

    // Handle different save modes
    let mode = mode.unwrap_or(SaveMode::Create);

    match mode {
        SaveMode::Create => {
            // Default: create new playlist or fail if exists
            // Check if playlist already exists
            match db.load_playlist(name) {
                Ok(_) => {
                    // Playlist exists, fail
                    ResponseBuilder::error(50, 0, "save", "Playlist already exists")
                }
                Err(_) => {
                    // Playlist doesn't exist, create it
                    match db.save_playlist(name, &songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {}", e)),
                    }
                }
            }
        }
        SaveMode::Replace => {
            // Replace existing playlist or create if doesn't exist
            // Delete existing playlist if it exists (ignore errors)
            let _ = db.delete_playlist(name);

            // Save new playlist
            match db.save_playlist(name, &songs) {
                Ok(_) => ResponseBuilder::new().ok(),
                Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {}", e)),
            }
        }
        SaveMode::Append => {
            // Append to existing playlist or create if doesn't exist
            match db.load_playlist(name) {
                Ok(mut existing_songs) => {
                    // Playlist exists, append to it
                    existing_songs.extend(songs);

                    // Save updated playlist
                    match db.save_playlist(name, &existing_songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {}", e)),
                    }
                }
                Err(_) => {
                    // Playlist doesn't exist, create it
                    match db.save_playlist(name, &songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {}", e)),
                    }
                }
            }
        }
    }
}

async fn handle_load_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
    position: Option<u32>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "load", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "load", &format!("database error: {}", e)),
    };

    match db.load_playlist(name) {
        Ok(mut songs) => {
            // Apply range filter if specified
            if let Some((start, end)) = range {
                let start = start as usize;
                let end = end.min(songs.len() as u32) as usize;
                if start < songs.len() {
                    songs = songs[start..end].to_vec();
                } else {
                    return ResponseBuilder::error(50, 0, "load", "Invalid range");
                }
            }

            let mut queue = state.queue.write().await;

            // If position is specified, add at that position
            // Otherwise, clear queue and add all songs
            if let Some(pos) = position {
                for (i, song) in songs.into_iter().enumerate() {
                    queue.add_at(song, Some(pos + i as u32));
                }
            } else {
                queue.clear();
                for song in songs {
                    queue.add(song);
                }
            }
            drop(queue);

            // Update status
            let mut status = state.status.write().await;
            status.playlist_version += 1;
            status.playlist_length = state.queue.read().await.len() as u32;

            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "load", &format!("Error: {}", e)),
    }
}

async fn handle_listplaylists_command(state: &AppState) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listplaylists", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(
                50,
                0,
                "listplaylists",
                &format!("database error: {}", e),
            )
        }
    };

    match db.list_playlists() {
        Ok(playlists) => {
            let mut resp = ResponseBuilder::new();
            for playlist in &playlists {
                resp.field("playlist", &playlist.name);
                let timestamp_str = format_iso8601_timestamp(playlist.last_modified);
                resp.field("Last-Modified", &timestamp_str);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylists", &format!("Error: {}", e)),
    }
}

async fn handle_listplaylist_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listplaylist", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listplaylist", &format!("database error: {}", e))
        }
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            // Apply range filter
            let filtered = if let Some((start, end)) = range {
                let start_idx = start as usize;
                let end_idx = end.min(songs.len() as u32) as usize;
                if start_idx < songs.len() {
                    &songs[start_idx..end_idx]
                } else {
                    &[]
                }
            } else {
                &songs[..]
            };

            let mut resp = ResponseBuilder::new();
            for song in filtered {
                resp.field("file", &song.path);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylist", &format!("Error: {}", e)),
    }
}

async fn handle_listplaylistinfo_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => {
            return ResponseBuilder::error(50, 0, "listplaylistinfo", "database not configured")
        }
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(
                50,
                0,
                "listplaylistinfo",
                &format!("database error: {}", e),
            )
        }
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            // Apply range filter
            let filtered = if let Some((start, end)) = range {
                let start_idx = start as usize;
                let end_idx = end.min(songs.len() as u32) as usize;
                if start_idx < songs.len() {
                    &songs[start_idx..end_idx]
                } else {
                    &[]
                }
            } else {
                &songs[..]
            };

            let mut resp = ResponseBuilder::new();
            for song in filtered {
                resp.song(song, None, None);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylistinfo", &format!("Error: {}", e)),
    }
}

async fn handle_playlistadd_command(
    state: &AppState,
    name: &str,
    uri: &str,
    position: Option<u32>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistadd", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "playlistadd", &format!("database error: {}", e))
        }
    };

    // TODO: Implement position support in database layer
    // For now, position parameter is parsed but not used
    let _ = position;

    match db.playlist_add(name, uri) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistadd", &format!("Error: {}", e)),
    }
}

async fn handle_playlistclear_command(state: &AppState, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistclear", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(
                50,
                0,
                "playlistclear",
                &format!("database error: {}", e),
            )
        }
    };

    match db.playlist_clear(name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistclear", &format!("Error: {}", e)),
    }
}

async fn handle_playlistdelete_command(state: &AppState, name: &str, position: u32) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistdelete", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(
                50,
                0,
                "playlistdelete",
                &format!("database error: {}", e),
            )
        }
    };

    match db.playlist_delete_pos(name, position) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistdelete", &format!("Error: {}", e)),
    }
}

async fn handle_playlistmove_command(state: &AppState, name: &str, from: u32, to: u32) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistmove", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "playlistmove", &format!("database error: {}", e))
        }
    };

    match db.playlist_move(name, from, to) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistmove", &format!("Error: {}", e)),
    }
}

async fn handle_rm_command(state: &AppState, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "rm", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "rm", &format!("database error: {}", e)),
    };

    match db.delete_playlist(name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "rm", &format!("Error: {}", e)),
    }
}

async fn handle_rename_command(state: &AppState, from: &str, to: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "rename", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "rename", &format!("database error: {}", e))
        }
    };

    match db.rename_playlist(from, to) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "rename", &format!("Error: {}", e)),
    }
}

// Output control commands
async fn handle_outputs_command(state: &AppState) -> String {
    let outputs = state.outputs.read().await;
    let mut resp = ResponseBuilder::new();

    for (i, output) in outputs.iter().enumerate() {
        resp.field("outputid", output.id);
        resp.field("outputname", &output.name);
        resp.field("plugin", &output.plugin);
        resp.field("outputenabled", if output.enabled { "1" } else { "0" });
        // Add blank line between outputs, but not after the last one
        if i < outputs.len() - 1 {
            resp.blank_line();
        }
    }

    resp.ok()
}

async fn handle_enableoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = true;
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "enableoutput", "No such output")
    }
}

async fn handle_disableoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = false;
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "disableoutput", "No such output")
    }
}

async fn handle_toggleoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = !output.enabled;
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "toggleoutput", "No such output")
    }
}

async fn handle_outputset_command(state: &AppState, id: u32, _name: &str, _value: &str) -> String {
    // Verify output exists
    let outputs = state.outputs.read().await;
    if outputs.iter().any(|o| o.id == id) {
        // For now, just acknowledge - actual attribute setting would be implemented
        // when we have configurable output properties
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "outputset", "No such output")
    }
}

// Reflection commands
async fn handle_commands_command() -> String {
    let mut resp = ResponseBuilder::new();

    // Playback control
    resp.field("command", "play");
    resp.field("command", "playid");
    resp.field("command", "pause");
    resp.field("command", "stop");
    resp.field("command", "next");
    resp.field("command", "previous");
    resp.field("command", "seek");
    resp.field("command", "seekid");
    resp.field("command", "seekcur");

    // Queue management
    resp.field("command", "add");
    resp.field("command", "addid");
    resp.field("command", "delete");
    resp.field("command", "deleteid");
    resp.field("command", "clear");
    resp.field("command", "move");
    resp.field("command", "moveid");
    resp.field("command", "swap");
    resp.field("command", "swapid");
    resp.field("command", "shuffle");
    resp.field("command", "playlistid");

    // Status & inspection
    resp.field("command", "status");
    resp.field("command", "currentsong");
    resp.field("command", "stats");
    resp.field("command", "playlistinfo");
    resp.field("command", "playlistid");

    // Volume
    resp.field("command", "setvol");
    resp.field("command", "volume");

    // Options
    resp.field("command", "repeat");
    resp.field("command", "random");
    resp.field("command", "single");
    resp.field("command", "consume");
    resp.field("command", "crossfade");

    // Connection
    resp.field("command", "close");
    resp.field("command", "ping");
    resp.field("command", "password");

    // Reflection
    resp.field("command", "commands");
    resp.field("command", "notcommands");
    resp.field("command", "tagtypes");
    resp.field("command", "urlhandlers");
    resp.field("command", "decoders");

    // Database
    resp.field("command", "update");
    resp.field("command", "rescan");
    resp.field("command", "find");
    resp.field("command", "search");
    resp.field("command", "list");
    resp.field("command", "listall");
    resp.field("command", "listallinfo");
    resp.field("command", "lsinfo");
    resp.field("command", "count");

    // Album art
    resp.field("command", "albumart");
    resp.field("command", "readpicture");

    // Stored playlists
    resp.field("command", "save");
    resp.field("command", "load");
    resp.field("command", "listplaylists");
    resp.field("command", "listplaylist");
    resp.field("command", "listplaylistinfo");
    resp.field("command", "playlistadd");
    resp.field("command", "playlistclear");
    resp.field("command", "playlistdelete");
    resp.field("command", "playlistmove");
    resp.field("command", "rm");
    resp.field("command", "rename");

    // Idle
    resp.field("command", "idle");
    resp.field("command", "noidle");

    // Outputs
    resp.field("command", "outputs");
    resp.field("command", "enableoutput");
    resp.field("command", "disableoutput");
    resp.field("command", "toggleoutput");
    resp.field("command", "outputset");

    // Command batching
    resp.field("command", "command_list_begin");
    resp.field("command", "command_list_ok_begin");
    resp.field("command", "command_list_end");

    resp.ok()
}

async fn handle_notcommands_command() -> String {
    // Return empty list - no password-protected commands yet
    ResponseBuilder::new().ok()
}

async fn handle_tagtypes_command(subcommand: Option<crate::parser::TagTypesSubcommand>) -> String {
    use crate::parser::TagTypesSubcommand;

    let mut resp = ResponseBuilder::new();

    match subcommand {
        None | Some(TagTypesSubcommand::Available) => {
            // List all supported metadata tags
            resp.field("tagtype", "Artist");
            resp.field("tagtype", "ArtistSort");
            resp.field("tagtype", "Album");
            resp.field("tagtype", "AlbumSort");
            resp.field("tagtype", "AlbumArtist");
            resp.field("tagtype", "AlbumArtistSort");
            resp.field("tagtype", "Title");
            resp.field("tagtype", "Track");
            resp.field("tagtype", "Name");
            resp.field("tagtype", "Genre");
            resp.field("tagtype", "Date");
            resp.field("tagtype", "Composer");
            resp.field("tagtype", "Performer");
            resp.field("tagtype", "Comment");
            resp.field("tagtype", "Disc");
        }
        Some(TagTypesSubcommand::All) => {
            // Enable all tag types for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK as all tags are enabled by default
        }
        Some(TagTypesSubcommand::Clear) => {
            // Disable all tag types for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK
        }
        Some(TagTypesSubcommand::Enable { tags: _ }) => {
            // Enable specific tags for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK as all tags are enabled by default
        }
        Some(TagTypesSubcommand::Disable { tags: _ }) => {
            // Disable specific tags for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK
        }
        Some(TagTypesSubcommand::Reset { tags: _ }) => {
            // Reset specific tags to default state for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK
        }
    }

    resp.ok()
}

async fn handle_protocol_command(subcommand: Option<crate::parser::ProtocolSubcommand>) -> String {
    use crate::parser::ProtocolSubcommand;

    let mut resp = ResponseBuilder::new();

    match subcommand {
        None | Some(ProtocolSubcommand::Available) => {
            // List all available protocol features
            // Based on MPD 0.24.x protocol features
            resp.field("feature", "binary"); // Binary responses
            resp.field("feature", "command_list_ok"); // Command lists with OK markers
            resp.field("feature", "idle"); // Idle notifications
            resp.field("feature", "ranges"); // Range syntax (START:END)
            resp.field("feature", "tags"); // Tag type negotiation
        }
        Some(ProtocolSubcommand::All) => {
            // Enable all protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK as all features are enabled by default
        }
        Some(ProtocolSubcommand::Clear) => {
            // Disable all protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK
        }
        Some(ProtocolSubcommand::Enable { features: _ }) => {
            // Enable specific protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK as all features are enabled by default
        }
        Some(ProtocolSubcommand::Disable { features: _ }) => {
            // Disable specific protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK
        }
    }

    resp.ok()
}

async fn handle_urlhandlers_command() -> String {
    let mut resp = ResponseBuilder::new();

    // Supported URL schemes
    resp.field("handler", "file://");
    // Future: http://, https://, etc.

    resp.ok()
}

async fn handle_decoders_command() -> String {
    let mut resp = ResponseBuilder::new();

    // All decoders provided by Symphonia
    // Note: Unlike outputs, decoders are NOT separate entities - no blank lines between them
    resp.field("plugin", "flac");
    resp.field("suffix", "flac");
    resp.field("mime_type", "audio/flac");

    resp.field("plugin", "mp3");
    resp.field("suffix", "mp3");
    resp.field("mime_type", "audio/mpeg");

    resp.field("plugin", "vorbis");
    resp.field("suffix", "ogg");
    resp.field("suffix", "oga");
    resp.field("mime_type", "audio/ogg");
    resp.field("mime_type", "audio/vorbis");

    resp.field("plugin", "opus");
    resp.field("suffix", "opus");
    resp.field("mime_type", "audio/opus");

    resp.field("plugin", "aac");
    resp.field("suffix", "aac");
    resp.field("suffix", "m4a");
    resp.field("mime_type", "audio/aac");
    resp.field("mime_type", "audio/mp4");

    resp.field("plugin", "wav");
    resp.field("suffix", "wav");
    resp.field("mime_type", "audio/wav");

    resp.ok()
}

// Advanced database commands
async fn handle_searchadd_command(state: &AppState, tag: &str, value: &str) -> String {
    // Search and add results to queue
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "searchadd", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "searchadd", &format!("database error: {}", e))
        }
    };

    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "searchadd", &format!("search error: {}", e))
            }
        }
    } else {
        match db.find_songs(tag, value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "searchadd", &format!("query error: {}", e))
            }
        }
    };

    let mut queue = state.queue.write().await;
    for song in songs {
        queue.add(song);
    }
    drop(queue);

    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = state.queue.read().await.len() as u32;

    ResponseBuilder::new().ok()
}

async fn handle_searchaddpl_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    // Search and add results to stored playlist
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "searchaddpl", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "searchaddpl", &format!("database error: {}", e))
        }
    };

    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    50,
                    0,
                    "searchaddpl",
                    &format!("search error: {}", e),
                )
            }
        }
    } else {
        match db.find_songs(tag, value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "searchaddpl", &format!("query error: {}", e))
            }
        }
    };

    for song in songs {
        if let Err(e) = db.playlist_add(name, song.path.as_str()) {
            return ResponseBuilder::error(50, 0, "searchaddpl", &format!("Error: {}", e));
        }
    }

    ResponseBuilder::new().ok()
}

async fn handle_findadd_command(state: &AppState, tag: &str, value: &str) -> String {
    // Find (exact match) and add to queue
    handle_searchadd_command(state, tag, value).await
}

async fn handle_listfiles_command(state: &AppState, uri: Option<&str>) -> String {
    // List all files (songs and playlists)
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listfiles", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listfiles", &format!("database error: {}", e))
        }
    };

    let path = uri.unwrap_or("");
    match db.list_directory(path) {
        Ok(listing) => {
            let mut resp = ResponseBuilder::new();
            for dir in &listing.directories {
                resp.field("directory", dir);
            }
            for song in &listing.songs {
                resp.field("file", &song.path);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listfiles", &format!("Error: {}", e)),
    }
}

// Sticker commands (metadata storage)
async fn handle_sticker_get_command(state: &AppState, uri: &str, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "sticker get", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "sticker get", &format!("database error: {}", e))
        }
    };

    match db.get_sticker(uri, name) {
        Ok(Some(value)) => {
            let mut resp = ResponseBuilder::new();
            resp.field("sticker", format!("{}={}", name, value));
            resp.ok()
        }
        Ok(None) => ResponseBuilder::error(50, 0, "sticker get", "no such sticker"),
        Err(e) => ResponseBuilder::error(50, 0, "sticker get", &format!("Error: {}", e)),
    }
}

async fn handle_sticker_set_command(
    state: &AppState,
    uri: &str,
    name: &str,
    value: &str,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "sticker set", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "sticker set", &format!("database error: {}", e))
        }
    };

    match db.set_sticker(uri, name, value) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "sticker set", &format!("Error: {}", e)),
    }
}

async fn handle_sticker_delete_command(state: &AppState, uri: &str, name: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "sticker delete", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(
                50,
                0,
                "sticker delete",
                &format!("database error: {}", e),
            )
        }
    };

    match db.delete_sticker(uri, name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "sticker delete", &format!("Error: {}", e)),
    }
}

async fn handle_sticker_list_command(state: &AppState, uri: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "sticker list", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "sticker list", &format!("database error: {}", e))
        }
    };

    match db.list_stickers(uri) {
        Ok(stickers) => {
            let mut resp = ResponseBuilder::new();
            for (name, value) in stickers {
                resp.field("sticker", format!("{}={}", name, value));
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "sticker list", &format!("Error: {}", e)),
    }
}

async fn handle_sticker_find_command(
    state: &AppState,
    uri: &str,
    name: &str,
    _value: Option<&str>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "sticker find", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "sticker find", &format!("database error: {}", e))
        }
    };

    match db.find_stickers(uri, name) {
        Ok(results) => {
            let mut resp = ResponseBuilder::new();
            for (file_uri, sticker_value) in results {
                resp.field("file", file_uri);
                resp.field("sticker", format!("{}={}", name, sticker_value));
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "sticker find", &format!("Error: {}", e)),
    }
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

async fn handle_mixrampdb_command(state: &AppState, decibels: f32) -> String {
    let mut status = state.status.write().await;
    status.mixramp_db = decibels;
    ResponseBuilder::new().ok()
}

async fn handle_mixrampdelay_command(state: &AppState, seconds: f32) -> String {
    let mut status = state.status.write().await;
    status.mixramp_delay = seconds;
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

// ReplayGain commands
async fn handle_replaygain_mode_command(state: &AppState, mode: &str) -> String {
    // Set ReplayGain mode (off, track, album, auto)
    // Store in player status or engine config
    let _ = (state, mode);
    ResponseBuilder::new().ok()
}

async fn handle_replaygain_status_command(state: &AppState) -> String {
    // Return current ReplayGain status
    let _ = state;
    let mut resp = ResponseBuilder::new();
    resp.field("replay_gain_mode", "off");
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
    handle_count_command(state, &filters, group).await
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

// Playlist commands
async fn handle_searchplaylist_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    // Search stored playlist for songs matching tag/value
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            if let Ok(songs) = db.load_playlist(name) {
                let mut resp = ResponseBuilder::new();
                let value_lower = value.to_lowercase();

                for song in songs {
                    let matches = match tag.to_lowercase().as_str() {
                        "artist" => song
                            .artist
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&value_lower))
                            .unwrap_or(false),
                        "album" => song
                            .album
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&value_lower))
                            .unwrap_or(false),
                        "title" => song
                            .title
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&value_lower))
                            .unwrap_or(false),
                        _ => false,
                    };

                    if matches {
                        resp.song(&song, None, None);
                    }
                }
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "searchplaylist", "Playlist not found")
}

async fn handle_playlistlength_command(state: &AppState, name: &str) -> String {
    // Get playlist length and total duration
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            if let Ok(songs) = db.load_playlist(name) {
                let total_duration: f64 = songs
                    .iter()
                    .filter_map(|s| s.duration)
                    .map(|d| d.as_secs_f64())
                    .sum();

                let mut resp = ResponseBuilder::new();
                resp.field("songs", songs.len().to_string());
                resp.field("playtime", format!("{:.3}", total_duration));
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "playlistlength", "Playlist not found")
}

// Sticker commands
async fn handle_sticker_inc_command(
    state: &AppState,
    uri: &str,
    name: &str,
    delta: Option<i32>,
) -> String {
    // Increment numeric sticker value
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            let increment = delta.unwrap_or(1);

            // Get current value
            let current = if let Ok(Some(val)) = db.get_sticker(uri, name) {
                val.parse::<i32>().unwrap_or(0)
            } else {
                0
            };

            let new_value = current + increment;
            if db.set_sticker(uri, name, &new_value.to_string()).is_ok() {
                let mut resp = ResponseBuilder::new();
                resp.field("sticker", format!("{}={}", name, new_value));
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "sticker inc", "Failed to increment sticker")
}

async fn handle_sticker_dec_command(
    state: &AppState,
    uri: &str,
    name: &str,
    delta: Option<i32>,
) -> String {
    // Decrement numeric sticker value
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            let decrement = delta.unwrap_or(1);

            // Get current value
            let current = if let Ok(Some(val)) = db.get_sticker(uri, name) {
                val.parse::<i32>().unwrap_or(0)
            } else {
                0
            };

            let new_value = current - decrement;
            if db.set_sticker(uri, name, &new_value.to_string()).is_ok() {
                let mut resp = ResponseBuilder::new();
                resp.field("sticker", format!("{}={}", name, new_value));
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "sticker dec", "Failed to decrement sticker")
}

async fn handle_sticker_names_command(state: &AppState, uri: Option<&str>) -> String {
    // List unique sticker names (optionally for specific URI)
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            // For now, just return stickers for the given URI if provided
            // Full implementation would need a new database query
            if let Some(uri_str) = uri {
                if let Ok(stickers) = db.list_stickers(uri_str) {
                    let mut resp = ResponseBuilder::new();
                    for (name, _) in stickers {
                        resp.field("sticker", &name);
                    }
                    return resp.ok();
                }
            }
        }
    }
    ResponseBuilder::new().ok()
}

async fn handle_sticker_types_command() -> String {
    // List available sticker types (song is the primary type)
    let mut resp = ResponseBuilder::new();
    resp.field("sticker", "song");
    resp.ok()
}

async fn handle_sticker_namestypes_command(state: &AppState, uri: Option<&str>) -> String {
    // List sticker names and types
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            if let Some(uri_str) = uri {
                if let Ok(stickers) = db.list_stickers(uri_str) {
                    let mut resp = ResponseBuilder::new();
                    for (name, _) in stickers {
                        resp.field("sticker", format!("{} song", name));
                    }
                    return resp.ok();
                }
            }
        }
    }
    ResponseBuilder::new().ok()
}
