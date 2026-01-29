use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error, debug};
use anyhow::Result;

use crate::parser::{parse_command, Command};
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
        Command::Update { path: _ } | Command::Rescan { path: _ } => {
            // Return update job ID (for now, just return 1)
            let mut resp = ResponseBuilder::new();
            resp.field("updating_db", 1);
            resp.ok()
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
