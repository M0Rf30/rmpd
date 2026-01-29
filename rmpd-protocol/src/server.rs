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
            Ok(cmd) => handle_command(cmd, &state).await,
            Err(e) => ResponseBuilder::error(5, 0, trimmed, &e),
        };

        writer.write_all(response.as_bytes()).await?;
    }

    Ok(())
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
        Command::ListAll { path: _ } | Command::ListAllInfo { path: _ } | Command::LsInfo { path: _ } => {
            // For now, return empty list
            ResponseBuilder::new().ok()
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
        Command::Unknown(cmd) => {
            ResponseBuilder::error(5, 0, &cmd, "unknown command")
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

async fn handle_seek_command(_state: &AppState, _position: u32, _time: f64) -> String {
    // Seek not yet implemented in engine
    ResponseBuilder::error(5, 0, "seek", "not yet implemented")
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
