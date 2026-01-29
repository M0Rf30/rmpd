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

        let response = match parse_command(trimmed) {
            Ok(Command::Idle { subsystems }) => {
                handle_idle(&mut reader, &mut event_rx, subsystems).await
            }
            Ok(cmd) => handle_command(cmd, &state).await,
            Err(e) => ResponseBuilder::error(5, 0, trimmed, &e),
        };

        writer.write_all(response.as_bytes()).await?;
    }

    Ok(())
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
            let mut resp = ResponseBuilder::new();
            // List all supported commands
            resp.field("command", "play");
            resp.field("command", "pause");
            resp.field("command", "stop");
            resp.field("command", "next");
            resp.field("command", "previous");
            resp.field("command", "status");
            resp.field("command", "currentsong");
            resp.field("command", "stats");
            resp.field("command", "add");
            resp.field("command", "clear");
            resp.field("command", "delete");
            resp.field("command", "ping");
            resp.field("command", "close");
            resp.field("command", "update");
            resp.field("command", "rescan");
            resp.field("command", "find");
            resp.field("command", "search");
            resp.field("command", "list");
            resp.field("command", "listall");
            resp.field("command", "listallinfo");
            resp.field("command", "lsinfo");
            resp.field("command", "count");
            resp.ok()
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
        Command::Update { path } | Command::Rescan { path } => {
            handle_update_command(state, path.as_deref()).await
        }
        Command::Find { tag, value } => {
            handle_find_command(state, &tag, &value).await
        }
        Command::Search { tag, value } => {
            handle_search_command(state, &tag, &value).await
        }
        Command::List { tag, group: _ } => {
            handle_list_command(state, &tag).await
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
        _ => {
            // Unimplemented commands
            ResponseBuilder::error(5, 0, "command", "not yet implemented")
        }
    }
}

async fn handle_find_command(state: &AppState, tag: &str, value: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "find", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("database error: {}", e)),
    };

    let songs = match db.find_songs(tag, value) {
        Ok(s) => s,
        Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e)),
    };

    let mut resp = ResponseBuilder::new();
    for song in songs {
        resp.song(&song, None, None);
    }
    resp.ok()
}

async fn handle_search_command(state: &AppState, tag: &str, value: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "search", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "search", &format!("database error: {}", e)),
    };

    // For search, use FTS if tag is "any", otherwise use find
    let songs = if tag.eq_ignore_ascii_case("any") {
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
    };

    let mut resp = ResponseBuilder::new();
    for song in songs {
        resp.song(&song, None, None);
    }
    resp.ok()
}

async fn handle_list_command(state: &AppState, tag: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "list", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("database error: {}", e)),
    };

    let values = match tag.to_lowercase().as_str() {
        "artist" => db.list_artists(),
        "album" => db.list_albums(),
        "genre" => db.list_genres(),
        _ => return ResponseBuilder::error(2, 0, "list", &format!("unsupported tag: {}", tag)),
    };

    let values = match values {
        Ok(v) => v,
        Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("query error: {}", e)),
    };

    let mut resp = ResponseBuilder::new();
    let tag_key = match tag.to_lowercase().as_str() {
        "artist" => "Artist",
        "album" => "Album",
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

    // Get song to play
    let song = if let Some(pos) = position {
        // Play specific position
        if let Some(item) = queue.get(pos) {
            item.song.clone()
        } else {
            return ResponseBuilder::error(50, 0, "play", "No such song");
        }
    } else {
        // Resume or play first song
        let current_song = state.engine.read().await.get_current_song().await;
        if let Some(song) = current_song {
            song
        } else if let Some(item) = queue.get(0) {
            item.song.clone()
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
            if let Some(pos) = position {
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: pos,
                    id: state.queue.read().await.get(pos).map(|i| i.id).unwrap_or(0),
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
