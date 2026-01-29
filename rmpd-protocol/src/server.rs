use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error, debug};
use anyhow::Result;

use crate::parser::{parse_command, Command};
use crate::queue_playback::QueuePlaybackManager;
use crate::response::ResponseBuilder;
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "0.24.0";

pub struct MpdServer {
    bind_address: String,
    state: AppState,
}

impl MpdServer {
    pub fn new(bind_address: String) -> Self {
        Self {
            bind_address,
            state: AppState::new(),
        }
    }

    pub fn with_state(bind_address: String, state: AppState) -> Self {
        Self {
            bind_address,
            state,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(&self.bind_address).await?;
        info!("MPD server listening on {}", self.bind_address);

        // Start queue playback manager
        let mut playback_manager = QueuePlaybackManager::new(self.state.clone());
        playback_manager.start();
        info!("Queue playback manager started");

        loop {
            match listener.accept().await {
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
    }
}

async fn handle_client(mut stream: TcpStream, state: AppState) -> Result<()> {
    // Send greeting
    stream.write_all(format!("OK MPD {}\n", PROTOCOL_VERSION).as_bytes()).await?;

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
                    ResponseBuilder::error(5, 0, "command_list_end", "not in command list")
                } else {
                    let response = execute_command_list(&batch_commands, &state, batch_ok_mode).await;
                    batch_mode = false;
                    batch_ok_mode = false;
                    batch_commands.clear();
                    response
                }
            }
            Ok(Command::Idle { subsystems }) if !batch_mode => {
                handle_idle(&mut reader, &mut event_rx, subsystems).await
            }
            Ok(cmd) if batch_mode => {
                // Accumulate commands in batch
                batch_commands.push(trimmed.to_string());
                continue; // Don't send response yet
            }
            Ok(cmd) => handle_command(cmd, &state).await,
            Err(e) => ResponseBuilder::error(5, 0, trimmed, &e),
        };

        writer.write_all(response.as_bytes()).await?;
    }

    Ok(())
}

async fn execute_command_list(commands: &[String], state: &AppState, ok_mode: bool) -> String {
    let mut response = String::new();

    for (index, cmd_str) in commands.iter().enumerate() {
        match parse_command(cmd_str) {
            Ok(cmd) => {
                let cmd_response = handle_command(cmd, state).await;

                // Check for errors
                if cmd_response.starts_with("ACK") {
                    // Return error with command list index
                    return cmd_response.replace("ACK [", &format!("ACK [{}@", index));
                }

                if ok_mode {
                    // In OK mode, append list_OK after each successful command
                    response.push_str("list_OK\n");
                }
            }
            Err(e) => {
                // Parse error - return ACK with index
                return ResponseBuilder::error(5, index as i32, cmd_str, &e);
            }
        }
    }

    // All commands succeeded
    response.push_str("OK\n");
    response
}

async fn handle_idle(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    event_rx: &mut tokio::sync::broadcast::Receiver<rmpd_core::event::Event>,
    subsystems: Vec<String>,
) -> String {
    use rmpd_core::event::Subsystem;

    // Convert string subsystems to enum
    let filter_subsystems: Vec<Subsystem> = if subsystems.is_empty() {
        // If no subsystems specified, listen to all
        vec![]
    } else {
        subsystems.iter().filter_map(|s| match s.to_lowercase().as_str() {
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
        }).collect()
    };

    let mut line = String::new();

    loop {
        tokio::select! {
            // Wait for event
            event_result = event_rx.recv() => {
                if let Ok(event) = event_result {
                    let event_subsystems = event.subsystems();

                    // Check if event matches any subscribed subsystem
                    let matches = if filter_subsystems.is_empty() {
                        // No filter - return any event
                        !event_subsystems.is_empty()
                    } else {
                        // Check if event matches any filtered subsystem
                        event_subsystems.iter().any(|s| filter_subsystems.contains(s))
                    };

                    if matches {
                        // Return changed subsystem
                        let subsystem_name = subsystem_to_string(event_subsystems[0]);
                        return format!("changed: {}\nOK\n", subsystem_name);
                    }
                }
            }
            // Wait for noidle command
            line_result = reader.read_line(&mut line) => {
                if let Ok(bytes) = line_result {
                    if bytes > 0 && line.trim() == "noidle" {
                        // Cancel idle
                        return "OK\n".to_string();
                    }
                }
                // Connection closed or error
                return "OK\n".to_string();
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

async fn handle_command(cmd: Command, state: &AppState) -> String {
    match cmd {
        Command::Ping => ResponseBuilder::new().ok(),
        Command::Close => {
            // Connection will be closed by the handler
            ResponseBuilder::new().ok()
        }
        Command::Commands => {
            handle_commands_command().await
        }
        Command::NotCommands => {
            handle_notcommands_command().await
        }
        Command::TagTypes => {
            handle_tagtypes_command().await
        }
        Command::UrlHandlers => {
            handle_urlhandlers_command().await
        }
        Command::Decoders => {
            handle_decoders_command().await
        }
        Command::Status => {
            // Return status from state
            let status = state.status.read().await;
            let mut resp = ResponseBuilder::new();
            resp.status(&status);
            resp.ok()
        }
        Command::Stats => {
            // Get stats from database if available
            let (songs, artists, albums) = if let Some(ref db_path) = state.db_path {
                if let Ok(db) = rmpd_library::Database::open(db_path) {
                    (
                        db.count_songs().unwrap_or(0),
                        db.count_artists().unwrap_or(0),
                        db.count_albums().unwrap_or(0),
                    )
                } else {
                    (0, 0, 0)
                }
            } else {
                (0, 0, 0)
            };

            let mut resp = ResponseBuilder::new();
            resp.stats(artists, albums, songs, 0, 0, 0, 0);
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
        Command::Find { filters } => {
            handle_find_command(state, &filters).await
        }
        Command::Search { filters } => {
            handle_search_command(state, &filters).await
        }
        Command::List { tag, filter_tag, filter_value, group: _ } => {
            handle_list_command(state, &tag, filter_tag.as_deref(), filter_value.as_deref()).await
        }
        Command::Count { tag, value } => {
            handle_count_command(state, &tag, &value).await
        }
        Command::ListAll { path } => {
            handle_listall_command(state, path.as_deref()).await
        }
        Command::ListAllInfo { path } => {
            handle_listallinfo_command(state, path.as_deref()).await
        }
        Command::LsInfo { path } => {
            handle_lsinfo_command(state, path.as_deref()).await
        }
        Command::CurrentSong => {
            handle_currentsong_command(state).await
        }
        Command::PlaylistInfo { range: _ } => {
            handle_playlistinfo_command(state).await
        }
        Command::Playlist => {
            // Deprecated, same as playlistinfo
            handle_playlistinfo_command(state).await
        }
        Command::PlChanges { version } => {
            handle_plchanges_command(state, version).await
        }
        Command::PlChangesPosId { version } => {
            handle_plchangesposid_command(state, version).await
        }
        Command::PlaylistFind { tag, value } => {
            handle_playlistfind_command(state, &tag, &value).await
        }
        Command::PlaylistSearch { tag, value } => {
            handle_playlistsearch_command(state, &tag, &value).await
        }
        // Playback commands
        Command::Play { position } => {
            handle_play_command(state, position).await
        }
        Command::Pause { state: pause_state } => {
            handle_pause_command(state, pause_state).await
        }
        Command::Stop => {
            handle_stop_command(state).await
        }
        Command::Next => {
            handle_next_command(state).await
        }
        Command::Previous => {
            handle_previous_command(state).await
        }
        Command::Seek { position, time } => {
            handle_seek_command(state, position, time).await
        }
        Command::SeekId { id, time } => {
            handle_seekid_command(state, id, time).await
        }
        Command::SeekCur { time, relative } => {
            handle_seekcur_command(state, time, relative).await
        }
        Command::SetVol { volume } => {
            handle_setvol_command(state, volume).await
        }
        Command::Add { uri } => {
            handle_add_command(state, &uri).await
        }
        Command::Clear => {
            handle_clear_command(state).await
        }
        Command::Delete { position } => {
            handle_delete_command(state, position).await
        }
        Command::DeleteId { id } => {
            handle_deleteid_command(state, id).await
        }
        Command::AddId { uri, position } => {
            handle_addid_command(state, &uri, position).await
        }
        Command::PlayId { id } => {
            handle_playid_command(state, id).await
        }
        Command::MoveId { id, to } => {
            handle_moveid_command(state, id, to).await
        }
        Command::Swap { pos1, pos2 } => {
            handle_swap_command(state, pos1, pos2).await
        }
        Command::SwapId { id1, id2 } => {
            handle_swapid_command(state, id1, id2).await
        }
        Command::Move { from, to } => {
            handle_move_command(state, from, to).await
        }
        Command::Shuffle { range: _ } => {
            handle_shuffle_command(state).await
        }
        Command::PlaylistId { id } => {
            handle_playlistid_command(state, id).await
        }
        Command::Password { password: _ } => {
            // No password protection implemented yet
            ResponseBuilder::new().ok()
        }
        Command::AlbumArt { uri, offset } => {
            handle_albumart_command(state, &uri, offset).await
        }
        Command::ReadPicture { uri, offset } => {
            handle_readpicture_command(state, &uri, offset).await
        }
        Command::Unknown(cmd) => {
            ResponseBuilder::error(5, 0, &cmd, "unknown command")
        }
        Command::Repeat { enabled } => {
            handle_repeat_command(state, enabled).await
        }
        Command::Random { enabled } => {
            handle_random_command(state, enabled).await
        }
        Command::Single { mode } => {
            handle_single_command(state, &mode).await
        }
        Command::Consume { mode } => {
            handle_consume_command(state, &mode).await
        }
        Command::Crossfade { seconds } => {
            handle_crossfade_command(state, seconds).await
        }
        Command::Volume { change } => {
            handle_volume_command(state, change).await
        }
        Command::GetVol => {
            let status = state.status.read().await;
            let mut resp = ResponseBuilder::new();
            resp.field("volume", &status.volume.to_string());
            resp.ok()
        }
        Command::ReplayGainMode { mode } => {
            handle_replaygain_mode_command(state, &mode).await
        }
        Command::ReplayGainStatus => {
            handle_replaygain_status_command(state).await
        }
        Command::BinaryLimit { size } => {
            // Set binary limit (for large responses like images)
            // Store in connection state if needed, for now just acknowledge
            let _ = size;
            ResponseBuilder::new().ok()
        }
        Command::Protocol { min_version, max_version } => {
            // Protocol negotiation - for now just acknowledge
            let _ = (min_version, max_version);
            ResponseBuilder::new().ok()
        }
        // Stored playlists
        Command::Save { name } => {
            handle_save_command(state, &name).await
        }
        Command::Load { name } => {
            handle_load_command(state, &name).await
        }
        Command::ListPlaylists => {
            handle_listplaylists_command(state).await
        }
        Command::ListPlaylist { name } => {
            handle_listplaylist_command(state, &name).await
        }
        Command::ListPlaylistInfo { name } => {
            handle_listplaylistinfo_command(state, &name).await
        }
        Command::PlaylistAdd { name, uri } => {
            handle_playlistadd_command(state, &name, &uri).await
        }
        Command::PlaylistClear { name } => {
            handle_playlistclear_command(state, &name).await
        }
        Command::PlaylistDelete { name, position } => {
            handle_playlistdelete_command(state, &name, position).await
        }
        Command::PlaylistMove { name, from, to } => {
            handle_playlistmove_command(state, &name, from, to).await
        }
        Command::Rm { name } => {
            handle_rm_command(state, &name).await
        }
        Command::Rename { from, to } => {
            handle_rename_command(state, &from, &to).await
        }
        Command::SearchPlaylist { name, tag, value } => {
            handle_searchplaylist_command(state, &name, &tag, &value).await
        }
        Command::PlaylistLength { name } => {
            handle_playlistlength_command(state, &name).await
        }
        // Output control
        Command::Outputs => {
            handle_outputs_command(state).await
        }
        Command::EnableOutput { id } => {
            handle_enableoutput_command(state, id).await
        }
        Command::DisableOutput { id } => {
            handle_disableoutput_command(state, id).await
        }
        Command::ToggleOutput { id } => {
            handle_toggleoutput_command(state, id).await
        }
        Command::OutputSet { id, name, value } => {
            handle_outputset_command(state, id, &name, &value).await
        }
        // Advanced database
        Command::SearchAdd { tag, value } => {
            handle_searchadd_command(state, &tag, &value).await
        }
        Command::SearchAddPl { name, tag, value } => {
            handle_searchaddpl_command(state, &name, &tag, &value).await
        }
        Command::FindAdd { tag, value } => {
            handle_findadd_command(state, &tag, &value).await
        }
        Command::ListFiles { uri } => {
            handle_listfiles_command(state, uri.as_deref()).await
        }
        Command::SearchCount { tag, value, group } => {
            handle_searchcount_command(state, &tag, &value, group.as_deref()).await
        }
        Command::GetFingerprint { uri } => {
            handle_getfingerprint_command(state, &uri).await
        }
        Command::ReadComments { uri } => {
            handle_readcomments_command(state, &uri).await
        }
        // Stickers
        Command::StickerGet { uri, name } => {
            handle_sticker_get_command(state, &uri, &name).await
        }
        Command::StickerSet { uri, name, value } => {
            handle_sticker_set_command(state, &uri, &name, &value).await
        }
        Command::StickerDelete { uri, name } => {
            handle_sticker_delete_command(state, &uri, name.as_deref()).await
        }
        Command::StickerList { uri } => {
            handle_sticker_list_command(state, &uri).await
        }
        Command::StickerFind { uri, name, value } => {
            handle_sticker_find_command(state, &uri, &name, value.as_deref()).await
        }
        Command::StickerInc { uri, name, delta } => {
            handle_sticker_inc_command(state, &uri, &name, delta).await
        }
        Command::StickerDec { uri, name, delta } => {
            handle_sticker_dec_command(state, &uri, &name, delta).await
        }
        Command::StickerNames { uri } => {
            handle_sticker_names_command(state, uri.as_deref()).await
        }
        Command::StickerTypes => {
            handle_sticker_types_command().await
        }
        Command::StickerNamesTypes { uri } => {
            handle_sticker_namestypes_command(state, uri.as_deref()).await
        }
        // Partitions
        Command::Partition { name } => {
            handle_partition_command(state, &name).await
        }
        Command::ListPartitions => {
            handle_listpartitions_command().await
        }
        Command::NewPartition { name } => {
            handle_newpartition_command(&name).await
        }
        Command::DelPartition { name } => {
            handle_delpartition_command(&name).await
        }
        Command::MoveOutput { name } => {
            handle_moveoutput_command(&name).await
        }
        // Mounts
        Command::Mount { path, uri } => {
            handle_mount_command(&path, &uri).await
        }
        Command::Unmount { path } => {
            handle_unmount_command(&path).await
        }
        Command::ListMounts => {
            handle_listmounts_command().await
        }
        Command::ListNeighbors => {
            handle_listneighbors_command().await
        }
        // Client messaging
        Command::Subscribe { channel } => {
            handle_subscribe_command(&channel).await
        }
        Command::Unsubscribe { channel } => {
            handle_unsubscribe_command(&channel).await
        }
        Command::Channels => {
            handle_channels_command().await
        }
        Command::ReadMessages => {
            handle_readmessages_command().await
        }
        Command::SendMessage { channel, message } => {
            handle_sendmessage_command(&channel, &message).await
        }
        // Advanced queue
        Command::Prio { priority, range } => {
            handle_prio_command(state, priority, range).await
        }
        Command::PrioId { priority, id } => {
            handle_prioid_command(state, priority, id).await
        }
        Command::RangeId { id, range } => {
            handle_rangeid_command(state, id, range).await
        }
        Command::AddTagId { id, tag, value } => {
            handle_addtagid_command(state, id, &tag, &value).await
        }
        Command::ClearTagId { id, tag } => {
            handle_cleartagid_command(state, id, tag.as_deref()).await
        }
        // Miscellaneous
        Command::Config => {
            handle_config_command().await
        }
        Command::Kill => {
            handle_kill_command().await
        }
        Command::MixRampDb { decibels } => {
            handle_mixrampdb_command(state, decibels).await
        }
        Command::MixRampDelay { seconds } => {
            handle_mixrampdelay_command(state, seconds).await
        }
        _ => {
            // Unimplemented commands
            ResponseBuilder::error(5, 0, "command", "not yet implemented")
        }
    }
}

async fn handle_find_command(state: &AppState, filters: &[(String, String)]) -> String {
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
    let songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => {
                match db.find_songs_filter(&filter) {
                    Ok(s) => s,
                    Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e)),
                }
            }
            Err(e) => return ResponseBuilder::error(2, 0, "find", &format!("filter parse error: {}", e)),
        }
    } else if filters.len() == 1 {
        // Simple single tag/value search
        match db.find_songs(&filters[0].0, &filters[0].1) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e)),
        }
    } else {
        // Multiple tag/value pairs - build filter expression with AND
        use rmpd_core::filter::{FilterExpression, CompareOp};
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

    let mut resp = ResponseBuilder::new();
    for song in songs {
        resp.song(&song, None, None);
    }
    resp.ok()
}

async fn handle_search_command(state: &AppState, filters: &[(String, String)]) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "search", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "search", &format!("database error: {}", e)),
    };

    if filters.is_empty() {
        return ResponseBuilder::error(2, 0, "search", "missing arguments");
    }

    // Check if this is a filter expression (starts with '(')
    let songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => {
                match db.find_songs_filter(&filter) {
                    Ok(s) => s,
                    Err(e) => return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e)),
                }
            }
            Err(e) => return ResponseBuilder::error(2, 0, "search", &format!("filter parse error: {}", e)),
        }
    } else if filters.len() == 1 {
        let tag = &filters[0].0;
        let value = &filters[0].1;

        if tag.eq_ignore_ascii_case("any") {
            // Use FTS for "any" tag
            match db.search_songs(value) {
                Ok(s) => s,
                Err(e) => return ResponseBuilder::error(50, 0, "search", &format!("search error: {}", e)),
            }
        } else {
            // Partial match using LIKE
            match db.find_songs(tag, value) {
                Ok(s) => s,
                Err(e) => return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e)),
            }
        }
    } else {
        // Multiple tag/value pairs - build filter expression with AND
        use rmpd_core::filter::{FilterExpression, CompareOp};
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
            Err(e) => return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e)),
        }
    };

    let mut resp = ResponseBuilder::new();
    for song in songs {
        resp.song(&song, None, None);
    }
    resp.ok()
}

async fn handle_list_command(state: &AppState, tag: &str, filter_tag: Option<&str>, filter_value: Option<&str>) -> String {
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

async fn handle_count_command(state: &AppState, tag: &str, value: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "count", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "count", &format!("database error: {}", e)),
    };

    let songs = match db.find_songs(tag, value) {
        Ok(s) => s,
        Err(e) => return ResponseBuilder::error(50, 0, "count", &format!("query error: {}", e)),
    };

    let total_duration: u64 = songs.iter()
        .filter_map(|s| s.duration)
        .map(|d| d.as_secs())
        .sum();

    let mut resp = ResponseBuilder::new();
    resp.field("songs", songs.len());
    resp.field("playtime", total_duration);
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

    // Start playback
    match state.engine.write().await.play(song).await {
        Ok(_) => {
            // Update status
            let mut status = state.status.write().await;
            status.state = rmpd_core::state::PlayerState::Play;
            if let Some((pos, id)) = actual_position {
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: pos,
                    id,
                });
            }
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "play", &format!("Playback error: {}", e)),
    }
}

async fn handle_pause_command(state: &AppState, pause_state: Option<bool>) -> String {
    let mut engine = state.engine.write().await;
    let current_state = engine.get_state().await;

    let should_pause = pause_state.unwrap_or_else(|| current_state == rmpd_core::state::PlayerState::Play);

    match engine.pause().await {
        Ok(_) => {
            let mut status = state.status.write().await;
            status.state = if should_pause {
                rmpd_core::state::PlayerState::Pause
            } else {
                rmpd_core::state::PlayerState::Play
            };
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "pause", &format!("Pause error: {}", e)),
    }
}

async fn handle_stop_command(state: &AppState) -> String {
    match state.engine.write().await.stop().await {
        Ok(_) => {
            let mut status = state.status.write().await;
            status.state = rmpd_core::state::PlayerState::Stop;
            status.current_song = None;
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

        match state.engine.write().await.play(song).await {
            Ok(_) => {
                let mut status = state.status.write().await;
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: next_pos,
                    id: item_id,
                });
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

        match state.engine.write().await.play(song).await {
            Ok(_) => {
                let mut status = state.status.write().await;
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: prev_pos,
                    id: item_id,
                });
                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(50, 0, "previous", &format!("Playback error: {}", e)),
        }
    } else {
        ResponseBuilder::error(50, 0, "previous", "No previous song")
    }
}

async fn handle_seek_command(state: &AppState, position: u32, _time: f64) -> String {
    // Get song at position
    let queue = state.queue.read().await;
    let status = state.status.read().await;

    // Check if this is the current song
    if let Some(current) = status.current_song {
        if current.position == position {
            drop(queue);
            drop(status);
            // Seek in current song
            // TODO: Implement actual seeking in engine
            // For now, just acknowledge
            ResponseBuilder::new().ok()
        } else {
            ResponseBuilder::error(50, 0, "seek", "Can only seek in current song")
        }
    } else {
        ResponseBuilder::error(50, 0, "seek", "Not playing")
    }
}

async fn handle_seekid_command(state: &AppState, id: u32, _time: f64) -> String {
    let status = state.status.read().await;

    // Check if this is the current song
    if let Some(current) = status.current_song {
        if current.id == id {
            drop(status);
            // Seek in current song
            // TODO: Implement actual seeking in engine
            ResponseBuilder::new().ok()
        } else {
            ResponseBuilder::error(50, 0, "seekid", "Can only seek in current song")
        }
    } else {
        ResponseBuilder::error(50, 0, "seekid", "Not playing")
    }
}

async fn handle_seekcur_command(state: &AppState, _time: f64, _relative: bool) -> String {
    let status = state.status.read().await;

    if status.current_song.is_some() {
        drop(status);
        // Seek in current song
        // TODO: Implement actual seeking in engine with relative support
        ResponseBuilder::new().ok()
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

async fn handle_add_command(state: &AppState, uri: &str) -> String {
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

    // Add to queue
    let id = state.queue.write().await.add(song);

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

    ResponseBuilder::new().ok()
}

async fn handle_delete_command(state: &AppState, position: u32) -> String {
    if state.queue.write().await.delete(position).is_some() {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        status.playlist_length = state.queue.read().await.len() as u32;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "delete", "No such song")
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

async fn handle_albumart_command(state: &AppState, uri: &str, offset: usize) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "albumart", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "albumart", &format!("database error: {}", e)),
    };

    let extractor = rmpd_library::AlbumArtExtractor::new(db);

    match extractor.get_artwork(uri, offset) {
        Ok(Some(artwork)) => {
            // Binary response - for now just return metadata
            // TODO: Implement full binary response support with actual binary data
            let mut resp = ResponseBuilder::new();
            resp.field("size", artwork.total_size);
            resp.field("type", &artwork.mime_type);
            resp.binary_field("binary", &artwork.data);
            // Convert to bytes which includes the binary data
            // For now, we'll just return text response until binary protocol is fully implemented
            String::from_utf8(resp.to_bytes()).unwrap_or_else(|_| {
                ResponseBuilder::error(50, 0, "albumart", "Encoding error")
            })
        }
        Ok(None) => ResponseBuilder::error(50, 0, "albumart", "No album art found"),
        Err(e) => ResponseBuilder::error(50, 0, "albumart", &format!("Error: {}", e)),
    }
}

async fn handle_readpicture_command(state: &AppState, uri: &str, offset: usize) -> String {
    // readpicture is similar to albumart but returns any embedded picture
    // For now, we'll use the same implementation
    handle_albumart_command(state, uri, offset).await
}

// Queue ID-based operations
async fn handle_addid_command(state: &AppState, uri: &str, position: Option<u32>) -> String {
    debug!("AddId command received with URI: [{}], position: {:?}", uri, position);

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

            match state.engine.write().await.play(song).await {
                Ok(_) => {
                    let mut status = state.status.write().await;
                    status.state = rmpd_core::state::PlayerState::Play;
                    status.current_song = Some(rmpd_core::state::QueuePosition {
                        position,
                        id: song_id,
                    });
                    ResponseBuilder::new().ok()
                }
                Err(e) => ResponseBuilder::error(50, 0, "playid", &format!("Playback error: {}", e)),
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

async fn handle_move_command(state: &AppState, from: u32, to: u32) -> String {
    if state.queue.write().await.move_item(from, to) {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "move", "Invalid position")
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

async fn handle_shuffle_command(state: &AppState) -> String {
    state.queue.write().await.shuffle();
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

async fn handle_playlistinfo_command(state: &AppState) -> String {
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    for item in queue.items() {
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
        Err(e) => return ResponseBuilder::error(50, 0, "lsinfo", &format!("database error: {}", e)),
    };

    let path_str = path.unwrap_or("");

    // Get directory listing
    match db.list_directory(path_str) {
        Ok(listing) => {
            let mut resp = ResponseBuilder::new();

            // List subdirectories first
            for dir in &listing.directories {
                resp.field("directory", dir);
            }

            // Then list songs
            for song in &listing.songs {
                resp.song(song, None, None);
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
        Err(e) => return ResponseBuilder::error(50, 0, "listall", &format!("database error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "listallinfo", &format!("database error: {}", e)),
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
async fn handle_save_command(state: &AppState, name: &str) -> String {
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

    match db.save_playlist(name, &songs) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {}", e)),
    }
}

async fn handle_load_command(state: &AppState, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "load", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "load", &format!("database error: {}", e)),
    };

    match db.load_playlist(name) {
        Ok(songs) => {
            // Clear current queue and add all songs from playlist
            let mut queue = state.queue.write().await;
            queue.clear();
            for song in songs {
                queue.add(song);
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
        Err(e) => return ResponseBuilder::error(50, 0, "listplaylists", &format!("database error: {}", e)),
    };

    match db.list_playlists() {
        Ok(playlists) => {
            let mut resp = ResponseBuilder::new();
            for playlist in &playlists {
                resp.field("playlist", &playlist.name);
                resp.field("Last-Modified", &playlist.last_modified);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylists", &format!("Error: {}", e)),
    }
}

async fn handle_listplaylist_command(state: &AppState, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listplaylist", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "listplaylist", &format!("database error: {}", e)),
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            let mut resp = ResponseBuilder::new();
            for song in &songs {
                resp.field("file", &song.path);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylist", &format!("Error: {}", e)),
    }
}

async fn handle_listplaylistinfo_command(state: &AppState, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listplaylistinfo", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "listplaylistinfo", &format!("database error: {}", e)),
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            let mut resp = ResponseBuilder::new();
            for song in &songs {
                resp.song(song, None, None);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylistinfo", &format!("Error: {}", e)),
    }
}

async fn handle_playlistadd_command(state: &AppState, name: &str, uri: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistadd", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "playlistadd", &format!("database error: {}", e)),
    };

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
        Err(e) => return ResponseBuilder::error(50, 0, "playlistclear", &format!("database error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "playlistdelete", &format!("database error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "playlistmove", &format!("database error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "rename", &format!("database error: {}", e)),
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

    for output in outputs.iter() {
        resp.field("outputid", output.id);
        resp.field("outputname", &output.name);
        resp.field("plugin", &output.plugin);
        resp.field("outputenabled", if output.enabled { "1" } else { "0" });
    }

    resp.ok()
}

async fn handle_enableoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = true;
        state.event_bus.emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "enableoutput", "No such output")
    }
}

async fn handle_disableoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = false;
        state.event_bus.emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "disableoutput", "No such output")
    }
}

async fn handle_toggleoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = !output.enabled;
        state.event_bus.emit(rmpd_core::event::Event::OutputsChanged);
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

async fn handle_tagtypes_command() -> String {
    let mut resp = ResponseBuilder::new();

    // All supported metadata tags
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
        Err(e) => return ResponseBuilder::error(50, 0, "searchadd", &format!("database error: {}", e)),
    };

    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "searchadd", &format!("search error: {}", e)),
        }
    } else {
        match db.find_songs(tag, value) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "searchadd", &format!("query error: {}", e)),
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

async fn handle_searchaddpl_command(state: &AppState, name: &str, tag: &str, value: &str) -> String {
    // Search and add results to stored playlist
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "searchaddpl", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "searchaddpl", &format!("database error: {}", e)),
    };

    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "searchaddpl", &format!("search error: {}", e)),
        }
    } else {
        match db.find_songs(tag, value) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "searchaddpl", &format!("query error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "listfiles", &format!("database error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "sticker get", &format!("database error: {}", e)),
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

async fn handle_sticker_set_command(state: &AppState, uri: &str, name: &str, value: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "sticker set", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "sticker set", &format!("database error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "sticker delete", &format!("database error: {}", e)),
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
        Err(e) => return ResponseBuilder::error(50, 0, "sticker list", &format!("database error: {}", e)),
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

async fn handle_sticker_find_command(state: &AppState, uri: &str, name: &str, _value: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "sticker find", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "sticker find", &format!("database error: {}", e)),
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
async fn handle_prio_command(state: &AppState, _priority: u8, _range: (u32, u32)) -> String {
    // Set priority for range (stub - would need priority field in QueueItem)
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

async fn handle_prioid_command(state: &AppState, _priority: u8, _id: u32) -> String {
    // Set priority for ID (stub)
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
async fn handle_plchanges_command(state: &AppState, version: u32) -> String {
    // Return changes in queue since version
    // For now, return all queue items if version differs from current
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if queue.version() != version {
        for item in queue.items() {
            resp.field("file", item.song.path.as_str());
            resp.field("Pos", &item.position.to_string());
            resp.field("Id", &item.id.to_string());
            if let Some(ref title) = item.song.title {
                resp.field("Title", title);
            }
        }
    }
    resp.ok()
}

async fn handle_plchangesposid_command(state: &AppState, version: u32) -> String {
    // Return position/id changes since version
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if queue.version() != version {
        for item in queue.items() {
            resp.field("cpos", &item.position.to_string());
            resp.field("Id", &item.id.to_string());
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
            "artist" => item.song.artist.as_ref().map(|s| s.to_lowercase().contains(&value_lower)).unwrap_or(false),
            "album" => item.song.album.as_ref().map(|s| s.to_lowercase().contains(&value_lower)).unwrap_or(false),
            "title" => item.song.title.as_ref().map(|s| s.to_lowercase().contains(&value_lower)).unwrap_or(false),
            "genre" => item.song.genre.as_ref().map(|s| s.to_lowercase().contains(&value_lower)).unwrap_or(false),
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
async fn handle_searchcount_command(state: &AppState, tag: &str, value: &str, _group: Option<&str>) -> String {
    // Count search results with optional grouping
    // For now, same as count but could support grouping in future
    handle_count_command(state, tag, value).await
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
async fn handle_searchplaylist_command(state: &AppState, name: &str, tag: &str, value: &str) -> String {
    // Search stored playlist for songs matching tag/value
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            if let Ok(songs) = db.load_playlist(name) {
                let mut resp = ResponseBuilder::new();
                let value_lower = value.to_lowercase();

                for song in songs {
                    let matches = match tag.to_lowercase().as_str() {
                        "artist" => song.artist.as_ref().map(|s| s.to_lowercase().contains(&value_lower)).unwrap_or(false),
                        "album" => song.album.as_ref().map(|s| s.to_lowercase().contains(&value_lower)).unwrap_or(false),
                        "title" => song.title.as_ref().map(|s| s.to_lowercase().contains(&value_lower)).unwrap_or(false),
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
                let total_duration: f64 = songs.iter()
                    .filter_map(|s| s.duration)
                    .map(|d| d.as_secs_f64())
                    .sum();

                let mut resp = ResponseBuilder::new();
                resp.field("songs", &songs.len().to_string());
                resp.field("playtime", &format!("{:.3}", total_duration));
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "playlistlength", "Playlist not found")
}

// Sticker commands
async fn handle_sticker_inc_command(state: &AppState, uri: &str, name: &str, delta: Option<i32>) -> String {
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
                resp.field("sticker", &format!("{}={}", name, new_value));
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "sticker inc", "Failed to increment sticker")
}

async fn handle_sticker_dec_command(state: &AppState, uri: &str, name: &str, delta: Option<i32>) -> String {
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
                resp.field("sticker", &format!("{}={}", name, new_value));
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
                        resp.field("sticker", &format!("{} song", name));
                    }
                    return resp.ok();
                }
            }
        }
    }
    ResponseBuilder::new().ok()
}
