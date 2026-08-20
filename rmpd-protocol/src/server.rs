use rmpd_core::error::Result;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

use crate::commands::utils::{ACK_ERROR_ARG, ACK_ERROR_PERMISSION, ACK_ERROR_UNKNOWN};
use crate::commands::{
    connection, database, fingerprint, messaging, options, outputs, partition, playback, playlists,
    queue, reflection, stickers, storage,
};
use crate::parser::{Command, parse_command};
use crate::queue_playback::QueuePlaybackManager;
use crate::response::{Response, ResponseBuilder, Stats};
use crate::state::AppState;

/// MPD protocol version we implement. This is the MPD protocol spec version,
/// not the rmpd software version.
const PROTOCOL_VERSION: &str = "0.24.0";

/// Default maximum number of concurrent client connections when not
/// overridden via `with_max_connections`.
const DEFAULT_MAX_CONNECTIONS: usize = 100;

/// Default idle timeout (seconds) for a connection waiting on its next
/// command line, when not overridden via `with_connection_timeout`.
const DEFAULT_CONNECTION_TIMEOUT_SECS: u64 = 60;

/// Hard cap on a single protocol line (MPD's own line-length limit), so a
/// client that never sends a newline can't grow the read buffer without
/// bound. Enforced by capping the reader with `AsyncReadExt::take` before
/// `read_line`, not by checking the length after the fact.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Hard cap on the total size of an in-flight `command_list_begin` /
/// `command_list_ok_begin` batch (MPD's default `max_command_list_size`),
/// so an unterminated command list can't grow `batch_commands` without
/// bound.
const MAX_COMMAND_LIST_BYTES: usize = 2 * 1024 * 1024;

/// Convert a `parse_command` error into the correct ACK response string.
/// `parse_command` returns one of three message shapes: the complete,
/// final `unknown command "X"` text (an unrecognized command name,
/// including a nested command-list token or an empty line); a raw
/// tokenizer failure mirroring MPD's `Tokenizer` exceptions ("Invalid
/// unquoted character", "Missing closing '\"'", "Space expected after
/// closing '\"'") — both of these are thrown/caught before the command name
/// is ever looked up, so they get code 5 with an empty `{}` field, matching
/// MPD's `Response::command` defaulting to ""; or anything else — an arity
/// mismatch ("wrong number / too few / too many arguments for ...") or a
/// handler-level value error a token parser recorded ("Boolean (0/1)
/// expected: X", "Malformed range: X", "Incorrect number of filter
/// arguments", ...) — which gets code 2 against the real command name.
fn parse_error_to_ack(cmd_line: &str, err: &str, index: i32) -> String {
    if err.starts_with("unknown command \"")
        || err == "Invalid unquoted character"
        || err == "Missing closing '\"'"
        || err == "Space expected after closing '\"'"
    {
        return ResponseBuilder::error(ACK_ERROR_UNKNOWN, index, "", err);
    }
    let cmd_name = cmd_line.split_whitespace().next().unwrap_or(cmd_line);
    ResponseBuilder::error(ACK_ERROR_ARG, index, cmd_name, err)
}

/// Convert Unix timestamp to ISO 8601 format (RFC 3339)
#[derive(Debug)]
pub struct MpdServer {
    bind_address: String,
    unix_socket: Option<String>,
    state: AppState,
    shutdown_rx: broadcast::Receiver<()>,
    max_connections: usize,
    connection_timeout: std::time::Duration,
}

impl MpdServer {
    pub fn new(bind_address: String, shutdown_rx: broadcast::Receiver<()>) -> Self {
        Self {
            bind_address,
            unix_socket: None,
            state: AppState::new(),
            shutdown_rx,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            connection_timeout: std::time::Duration::from_secs(DEFAULT_CONNECTION_TIMEOUT_SECS),
        }
    }

    pub fn with_state(
        bind_address: String,
        state: AppState,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        Self {
            bind_address,
            unix_socket: None,
            state,
            shutdown_rx,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            connection_timeout: std::time::Duration::from_secs(DEFAULT_CONNECTION_TIMEOUT_SECS),
        }
    }

    pub fn with_unix_socket(mut self, path: Option<String>) -> Self {
        self.unix_socket = path;
        self
    }

    /// Set the maximum number of concurrent client connections. Connections
    /// beyond this limit are rejected (dropped) as soon as they are accepted.
    pub fn with_max_connections(mut self, n: usize) -> Self {
        self.max_connections = n;
        self
    }

    /// Set the idle timeout for a connection waiting on its next command
    /// line. A client that connects and then sends nothing is disconnected
    /// after this duration. Does not apply while a client is in `idle` mode.
    pub fn with_connection_timeout(mut self, d: std::time::Duration) -> Self {
        self.connection_timeout = d;
        self
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

        // Optionally bind Unix socket
        let unix_listener = if let Some(path) = &self.unix_socket {
            // Remove stale socket file if present
            let _ = std::fs::remove_file(path);
            Some(tokio::net::UnixListener::bind(path)?)
        } else {
            None
        };

        // Bounds the number of concurrently active connections. Each spawned
        // connection task holds a permit for its lifetime; the accept loop
        // never blocks on permit acquisition (that would stall accepting from
        // the other listener in the `select!` below) — it just drops the
        // connection immediately if none are available.
        let connection_limiter =
            std::sync::Arc::new(tokio::sync::Semaphore::new(self.max_connections));
        let connection_timeout = self.connection_timeout;

        loop {
            tokio::select! {
                // Handle incoming connections
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            debug!("new connection from {}", addr);
                            match connection_limiter.clone().try_acquire_owned() {
                                Ok(permit) => {
                                    let state = self.state.clone();
                                    tokio::spawn(async move {
                                        let _permit = permit;
                                        if let Err(e) = handle_client(stream, state, connection_timeout).await {
                                            log_client_error("client", &e);
                                        }
                                    });
                                }
                                Err(_) => {
                                    debug!(
                                        "connection limit ({}) reached, dropping connection from {}",
                                        self.max_connections, addr
                                    );
                                    drop(stream);
                                }
                            }
                        }
                        Err(e) => {
                            error!("failed to accept connection: {}", e);
                        }
                    }
                }
                result = async {
                    if let Some(ul) = &unix_listener {
                        ul.accept().await.map(|(s, _)| s)
                    } else {
                        std::future::pending::<tokio::io::Result<tokio::net::UnixStream>>().await
                    }
                } => {
                    match result {
                        Ok(stream) => {
                            match connection_limiter.clone().try_acquire_owned() {
                                Ok(permit) => {
                                    let state = self.state.clone();
                                    tokio::spawn(async move {
                                        let _permit = permit;
                                        if let Err(e) = handle_unix_client(stream, state, connection_timeout).await {
                                            log_client_error("unix client", &e);
                                        }
                                    });
                                }
                                Err(_) => {
                                    debug!(
                                        "connection limit ({}) reached, dropping unix connection",
                                        self.max_connections
                                    );
                                    drop(stream);
                                }
                            }
                        }
                        Err(e) => {
                            error!("unix accept error: {}", e);
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

        // Clean up socket file on shutdown
        if let Some(path) = &self.unix_socket {
            let _ = std::fs::remove_file(path);
        }

        info!("server shutdown complete");
        Ok(())
    }
}

/// Log an error from a client connection. A client closing its socket
/// (connection reset / broken pipe / EOF) is routine for MPD clients, so those
/// are logged at debug; anything else is a genuine error.
fn log_client_error(kind: &str, e: &rmpd_core::error::RmpdError) {
    use std::io::ErrorKind;
    let benign = matches!(
        e,
        rmpd_core::error::RmpdError::Io(io)
            if matches!(
                io.kind(),
                ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
                    | ErrorKind::UnexpectedEof
            )
    );
    if benign {
        debug!("{kind} disconnected: {e}");
    } else {
        error!("{kind} error: {e}");
    }
}

async fn handle_client(
    mut stream: TcpStream,
    state: AppState,
    timeout: std::time::Duration,
) -> Result<()> {
    // Enable TCP_NODELAY for low-latency responses (disable Nagle's algorithm)
    stream.set_nodelay(true)?;

    // Send greeting
    stream
        .write_all(format!("OK MPD {PROTOCOL_VERSION}\n").as_bytes())
        .await?;

    let (reader, writer) = stream.into_split();
    handle_client_inner(
        tokio::io::BufReader::new(reader),
        writer,
        state,
        timeout,
        false,
    )
    .await
}

async fn handle_unix_client(
    mut stream: UnixStream,
    state: AppState,
    timeout: std::time::Duration,
) -> Result<()> {
    // Send greeting
    stream
        .write_all(format!("OK MPD {PROTOCOL_VERSION}\n").as_bytes())
        .await?;

    let (reader, writer) = stream.into_split();
    handle_client_inner(
        tokio::io::BufReader::new(reader),
        writer,
        state,
        timeout,
        true,
    )
    .await
}

async fn handle_client_inner(
    mut reader: tokio::io::BufReader<impl tokio::io::AsyncRead + Unpin>,
    mut writer: impl tokio::io::AsyncWrite + Unpin,
    state: AppState,
    timeout: std::time::Duration,
    is_local: bool,
) -> Result<()> {
    let mut line = String::new();

    // Subscribe to event bus for idle notifications
    let mut event_rx = state.event_bus.subscribe();

    // Per-client connection state
    let mut conn_state = crate::ConnectionState::new();
    // Mirrors MPD's Client::IsLocal(): true only for the Unix domain socket,
    // never for TCP (gates `config` and the `file://` line of `urlhandlers`).
    conn_state.is_local = is_local;
    // Grant full permissions immediately when no password is configured;
    // otherwise the client starts with zero permissions and must `password` in.
    if state.password.is_none() {
        conn_state.grant_all_permissions();
    } else {
        conn_state.permissions = 0;
    }

    // Command batching state
    let mut batch_mode = false;
    let mut batch_ok_mode = false;
    let mut batch_commands: Vec<String> = Vec::new();
    let mut batch_bytes: usize = 0;

    loop {
        line.clear();
        // Cap the reader with `take` so `line` can never grow past
        // `MAX_LINE_BYTES`, regardless of whether the client ever sends a
        // newline — bounding the allocation, not just checking it after.
        let bytes_read = match tokio::time::timeout(
            timeout,
            (&mut reader)
                .take(MAX_LINE_BYTES as u64)
                .read_line(&mut line),
        )
        .await
        {
            Ok(result) => result?,
            Err(_elapsed) => {
                // Idle timeout: client connected but sent nothing for
                // `timeout`. Disconnect as if it had closed the socket.
                debug!("connection idle for {:?}, closing", timeout);
                break;
            }
        };

        if bytes_read == 0 {
            // Connection closed
            break;
        }

        if bytes_read == MAX_LINE_BYTES && !line.ends_with('\n') {
            // Hit the cap without finding a terminator: reject and close
            // rather than keep waiting for a newline that would let the
            // client grow the buffer unbounded.
            let response = Response::Text(ResponseBuilder::error(
                ACK_ERROR_ARG,
                0,
                "",
                "Line too long",
            ));
            writer.write_all(response.as_bytes()).await?;
            writer.flush().await?;
            break;
        }

        let stripped = line.trim_end();
        if !stripped
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
        {
            // MPD closes the connection immediately on any line that doesn't
            // start with a lowercase ASCII letter — empty lines, leading
            // whitespace, stray HTTP probes, etc. (Client::ProcessLine's
            // IsLowerAlphaASCII check; no ACK is sent for this).
            debug!("malformed command line, closing connection: {:?}", stripped);
            break;
        }

        if batch_mode && (stripped == "idle" || stripped == "noidle") {
            // MPD: idle/noidle are async commands that can't be used inside a
            // command list; the connection is closed immediately with no ACK
            // (Client::ProcessLine's IsAsyncCommmand check).
            debug!("async command {stripped:?} not allowed inside command list, closing");
            break;
        }

        let trimmed = stripped;
        debug!("received command: {}", trimmed);

        let response = match parse_command(trimmed) {
            Ok(Command::CommandListBegin) if !batch_mode => {
                batch_mode = true;
                batch_ok_mode = false;
                batch_commands.clear();
                batch_bytes = 0;
                continue; // Don't send response yet
            }
            Ok(Command::CommandListOkBegin) if !batch_mode => {
                batch_mode = true;
                batch_ok_mode = true;
                batch_commands.clear();
                batch_bytes = 0;
                continue; // Don't send response yet
            }
            Ok(Command::CommandListEnd) => {
                if !batch_mode {
                    // Real MPD has no dedicated "not in list" error:
                    // `command_list_end` isn't in the `commands[]` table, so
                    // outside a list it's looked up like any other name and
                    // reported as unknown, same as a typo'd command.
                    Response::Text(ResponseBuilder::error(
                        ACK_ERROR_UNKNOWN,
                        0,
                        "",
                        "unknown command \"command_list_end\"",
                    ))
                } else {
                    let (response, should_close) = execute_command_list(
                        &batch_commands,
                        &state,
                        &mut conn_state,
                        batch_ok_mode,
                    )
                    .await;
                    batch_mode = false;
                    batch_ok_mode = false;
                    batch_commands.clear();
                    batch_bytes = 0;
                    if should_close {
                        // `close` ran inside the list: send whatever partial
                        // output preceded it, then drop the connection —
                        // there's no final "OK" for a list that never
                        // finished (matches MPD's `CommandResult::FINISH`).
                        writer.write_all(response.as_bytes()).await?;
                        writer.flush().await?;
                        break;
                    }
                    response
                }
            }
            Ok(Command::Idle { subsystems }) if !batch_mode => {
                // idle bypasses handle_command's generic permission check
                // because it needs raw reader/event_rx access for long-poll;
                // enforce the same PERMISSION_READ MPD requires here.
                if !conn_state.has_permission(crate::connection::PERMISSION_READ) {
                    Response::Text(ResponseBuilder::error(
                        ACK_ERROR_PERMISSION,
                        0,
                        "idle",
                        "you don't have permission for \"idle\"",
                    ))
                } else {
                    Response::Text(handle_idle(&mut reader, &mut event_rx, subsystems).await)
                }
            }
            Ok(_cmd) if batch_mode => {
                // Accumulate commands in batch, capped so an unterminated
                // command list can't grow `batch_commands` without bound.
                if batch_bytes + trimmed.len() > MAX_COMMAND_LIST_BYTES {
                    batch_mode = false;
                    batch_ok_mode = false;
                    batch_commands.clear();
                    batch_bytes = 0;
                    Response::Text(ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "command_list",
                        "command list too large",
                    ))
                } else {
                    batch_bytes += trimmed.len();
                    batch_commands.push(trimmed.to_string());
                    continue; // Don't send response yet
                }
            }
            Ok(Command::NoIdle) => {
                // `noidle` received while NOT in idle mode. This happens when an
                // idle event was already delivered (flushing `changed: …\nOK\n`)
                // before the client's racing `noidle` arrived. Per MPD's
                // Client::ProcessLine, the server writes NOTHING in this case —
                // the client already received the full idle response. Emitting an
                // extra `OK` here would desync the stream (clients like rmpc then
                // report `Expected 'OK' but got '<value>'`).
                Response::Text(String::new())
            }
            Ok(Command::Close) => {
                // Close: terminate the connection immediately (per MPD spec)
                break;
            }
            Ok(cmd) => handle_command(cmd, &state, &mut conn_state).await,
            Err(e) => Response::Text(parse_error_to_ack(trimmed, &e, 0)),
        };

        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?; // Flush immediately to ensure low latency
    }

    // Cleanup: unregister any channel subscriptions when connection closes
    for channel in conn_state.subscribed_channels() {
        state.message_broker.unregister_subscriber(channel).await;
    }
    Ok(())
}

async fn execute_command_list(
    commands: &[String],
    state: &AppState,
    conn_state: &mut crate::ConnectionState,
    ok_mode: bool,
) -> (Response, bool) {
    let mut response = String::new();

    for (index, cmd_str) in commands.iter().enumerate() {
        match parse_command(cmd_str) {
            Ok(Command::Idle { .. }) => {
                return (
                    Response::Text(ResponseBuilder::error(
                        5,
                        index as i32,
                        "idle",
                        "cannot be used inside a command list",
                    )),
                    false,
                );
            }
            Ok(Command::NoIdle) => {
                // Silently ignore noidle inside command list
                continue;
            }
            Ok(Command::Close) => {
                // MPD terminates the connection immediately when `close` runs
                // inside a command list (`CommandResult::FINISH` short-circuits
                // `ProcessCommandList`), emitting no further list output; the
                // real `handle_command` dispatch for `Close` is unreachable by
                // design, so it must never receive it here.
                return (Response::Text(response), true);
            }
            Ok(
                Command::CommandListBegin | Command::CommandListOkBegin | Command::CommandListEnd,
            ) => {
                // None of these are in MPD's `commands[]` table either — the
                // top level treats them specially only when *not* already
                // inside a list (`Client::ProcessLine` appends every other
                // line verbatim once a list is active, with no nesting
                // check). Encountered here, they're looked up like any
                // other name and reported as unknown, aborting the list.
                let name = cmd_str.split_whitespace().next().unwrap_or(cmd_str);
                return (
                    Response::Text(ResponseBuilder::error(
                        5,
                        index as i32,
                        "",
                        &format!("unknown command \"{name}\""),
                    )),
                    false,
                );
            }
            Ok(cmd) => {
                let cmd_response = handle_command(cmd, state, conn_state).await;
                // Convert response to string for batching (binary commands not allowed in batch)
                let cmd_response_str = match cmd_response {
                    Response::Text(s) => s,
                    Response::Binary(_) => {
                        return (
                            Response::Text(ResponseBuilder::error(
                                5,
                                index as i32,
                                cmd_str,
                                "binary commands not allowed in command list",
                            )),
                            false,
                        );
                    }
                };

                // Check for errors: re-emit ACK with the correct command-list index
                if cmd_response_str.starts_with("ACK [") {
                    // In ok_mode, flush accumulated response (list_OK for prior cmds) THEN the ACK.
                    // MPD emits list_OK for each successful cmd before the error.
                    // The 'response' already contains list_OKs for prior cmds, so
                    // we just append the fixed ACK to it and return.
                    //
                    // Parse: ACK [{code}@{old_idx}] {cmd} msg
                    // Rebuild with actual `index` as the command-list position.
                    let fixed = if let Some(at_pos) = cmd_response_str.find('@') {
                        if let Some(bracket_end) = cmd_response_str.find(']') {
                            // code_part is text between '[' and '@'
                            let code_part = &cmd_response_str[5..at_pos]; // after "ACK ["
                            // rest starts AFTER ']' (skip the ']' itself)
                            let rest = &cmd_response_str[bracket_end + 1..];
                            format!("ACK [{}@{}]{}", code_part, index, rest)
                        } else {
                            cmd_response_str.clone()
                        }
                    } else {
                        cmd_response_str.clone()
                    };
                    response.push_str(&fixed);
                    return (Response::Text(response), false);
                }

                // Successful command: append response body (strip trailing "OK\n") to buffer
                let body = cmd_response_str
                    .strip_suffix("OK\n")
                    .unwrap_or(&cmd_response_str);
                response.push_str(body);

                if ok_mode {
                    // In OK mode, append list_OK after each successful command
                    response.push_str("list_OK\n");
                }
            }
            Err(e) => {
                // Parse error - return ACK with index
                return (
                    Response::Text(parse_error_to_ack(cmd_str, &e, index as i32)),
                    false,
                );
            }
        }
    }

    // All commands succeeded
    response.push_str("OK\n");
    (Response::Text(response), false)
}

async fn handle_idle(
    reader: &mut tokio::io::BufReader<impl tokio::io::AsyncRead + Unpin>,
    event_rx: &mut broadcast::Receiver<rmpd_core::event::Event>,
    subsystems: Vec<String>,
) -> String {
    use rmpd_core::event::Subsystem;
    use tokio::sync::broadcast::error::RecvError;

    const ALL_SUBSYSTEMS: [Subsystem; 14] = [
        Subsystem::Database,
        Subsystem::Update,
        Subsystem::StoredPlaylist,
        Subsystem::Playlist,
        Subsystem::Player,
        Subsystem::Mixer,
        Subsystem::Output,
        Subsystem::Options,
        Subsystem::Partition,
        Subsystem::Sticker,
        Subsystem::Subscription,
        Subsystem::Message,
        Subsystem::Neighbor,
        Subsystem::Mount,
    ];

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
        let mut limited = (&mut *reader).take(MAX_LINE_BYTES as u64);
        tokio::select! {
            // Wait for event
            event_result = event_rx.recv() => {
                match event_result {
                    Ok(event) => {
                        debug!("idle received event: {:?}", event);

                        // MPD accumulates every pending change and reports
                        // them all in one reply ("lists all changed systems
                        // in a line", protocol.rst command_idle). Drain any
                        // further events already queued so simultaneous
                        // changes produce multiple `changed:` lines instead
                        // of one idle reply per event.
                        let mut raw_subsystems: Vec<Subsystem> = event.subsystems().to_vec();
                        while let Ok(next_event) = event_rx.try_recv() {
                            raw_subsystems.extend_from_slice(next_event.subsystems());
                        }

                        let mut changed: Vec<Subsystem> = Vec::new();
                        for s in raw_subsystems {
                            let included =
                                filter_subsystems.is_empty() || filter_subsystems.contains(&s);
                            if included && !changed.contains(&s) {
                                changed.push(s);
                            }
                        }

                        if !changed.is_empty() {
                            debug!("idle returning changed: {:?}", changed);
                            let mut resp = String::new();
                            for s in &changed {
                                resp.push_str("changed: ");
                                resp.push_str(subsystem_to_string(*s));
                                resp.push('\n');
                            }
                            resp.push_str("OK\n");
                            return resp;
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        // The channel overflowed; we no longer know exactly
                        // which subsystems changed. Report every subsystem
                        // the client is watching (or the full set when
                        // unfiltered) rather than lose events.
                        debug!("idle: channel lagged, skipped {} messages", skipped);
                        let reported: &[Subsystem] = if filter_subsystems.is_empty() {
                            &ALL_SUBSYSTEMS
                        } else {
                            &filter_subsystems
                        };
                        let mut resp = String::new();
                        for s in reported {
                            resp.push_str("changed: ");
                            resp.push_str(subsystem_to_string(*s));
                            resp.push('\n');
                        }
                        resp.push_str("OK\n");
                        return resp;
                    }
                    Err(RecvError::Closed) => {
                        // Channel closed - should not happen, but handle gracefully
                        debug!("idle: event channel closed");
                        return "OK\n".to_owned();
                    }
                }
            }
            // Wait for noidle command
            line_result = limited.read_line(&mut line) => {
                if let Ok(bytes) = line_result
                    && bytes > 0 && line.trim() == "noidle" {
                        // Cancel idle
                        return "OK\n".to_owned();
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
    // Enforce permissions. PERMISSION_NONE commands always pass.
    let required = cmd.command_required_permission();
    if !conn_state.has_permission(required) {
        let name = cmd.command_name();
        return Response::Text(ResponseBuilder::error(
            ACK_ERROR_PERMISSION,
            0,
            name,
            &format!("you don't have permission for \"{}\"", name),
        ));
    }

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
        Command::Commands => reflection::handle_commands_command(conn_state).await,
        Command::NotCommands => reflection::handle_notcommands_command(conn_state).await,
        Command::TagTypes { subcommand } => {
            reflection::handle_tagtypes_command(conn_state, subcommand).await
        }
        Command::UrlHandlers => reflection::handle_urlhandlers_command(conn_state).await,
        Command::Decoders => reflection::handle_decoders_command().await,
        Command::StringNormalization { subcommand } => {
            reflection::handle_stringnormalization_command(conn_state, subcommand).await
        }
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

            let last_loaded_playlist = state.queue.read().await.last_loaded_playlist().to_string();
            let mut resp = ResponseBuilder::new();
            resp.status(
                &status,
                &conn_state.current_partition,
                &last_loaded_playlist,
            );
            resp.ok()
        }
        Command::Stats => {
            // Get stats from database if available
            let (songs, artists, albums, db_playtime, db_update) =
                if let Some(ref pool) = state.db_pool {
                    match rmpd_library::Database::from_pool(pool) {
                        Ok(db) => db.get_stats().unwrap_or((0, 0, 0, 0, 0)),
                        Err(_) => (0, 0, 0, 0, 0),
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
        Command::Update { path } => {
            database::handle_update_command(state, path.as_deref(), false).await
        }
        Command::Rescan { path } => {
            database::handle_update_command(state, path.as_deref(), true).await
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
            filters,
            groups,
            window,
        } => database::handle_list_command(state, &tag, &filters, &groups, window).await,
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
        Command::PlaylistFind {
            filters,
            sort,
            window,
        } => queue::handle_playlistfind_command(state, &filters, sort.as_deref(), window).await,
        Command::PlaylistSearch {
            filters,
            sort,
            window,
        } => queue::handle_playlistsearch_command(state, &filters, sort.as_deref(), window).await,
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
        Command::Password { password } => {
            connection::handle_password_command(state, conn_state, &password).await
        }
        Command::AlbumArt { .. } | Command::ReadPicture { .. } => {
            // Already handled at the beginning of the function
            unreachable!()
        }
        Command::Unknown(cmd) => ResponseBuilder::error(
            ACK_ERROR_UNKNOWN,
            0,
            "",
            &format!("unknown command {:?}", cmd),
        ),
        Command::UnknownSubcmd(main_cmd, _sub) => ResponseBuilder::error(
            crate::commands::utils::ACK_ERROR_ARG,
            0,
            &main_cmd,
            "Unknown sub command",
        ),
        Command::ArgError(cmd, msg, _raw) => {
            ResponseBuilder::error(crate::commands::utils::ACK_ERROR_ARG, 0, &cmd, &msg)
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
            // MPD (ClientCommands.cxx handle_binary_limit): rejects sizes
            // below 64 bytes with "Value too small".
            if size < 64 {
                ResponseBuilder::error(ACK_ERROR_ARG, 0, "binarylimit", "Value too small")
            } else {
                conn_state.binary_limit = size;
                ResponseBuilder::new().ok()
            }
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
        Command::PlaylistDelete { name, range } => {
            playlists::handle_playlistdelete_command(state, &name, range).await
        }
        Command::PlaylistMove { name, from, to } => {
            playlists::handle_playlistmove_command(state, &name, from, to).await
        }
        Command::Rm { name } => playlists::handle_rm_command(state, &name).await,
        Command::Rename { from, to } => playlists::handle_rename_command(state, &from, &to).await,
        Command::SearchPlaylist {
            name,
            filters,
            window,
        } => playlists::handle_searchplaylist_command(state, &name, &filters, window).await,
        Command::PlaylistLength { name } => {
            playlists::handle_playlistlength_command(state, &name).await
        }
        // Output control
        Command::Outputs => {
            outputs::handle_outputs_command(state, &conn_state.current_partition).await
        }
        Command::EnableOutput { id } => {
            outputs::handle_enableoutput_command(state, &conn_state.current_partition, id).await
        }
        Command::DisableOutput { id } => {
            outputs::handle_disableoutput_command(state, &conn_state.current_partition, id).await
        }
        Command::ToggleOutput { id } => {
            outputs::handle_toggleoutput_command(state, &conn_state.current_partition, id).await
        }
        Command::OutputSet { id, name, value } => {
            outputs::handle_outputset_command(
                state,
                &conn_state.current_partition,
                id,
                &name,
                &value,
            )
            .await
        }
        // Advanced database
        Command::SearchAdd {
            filters,
            sort,
            window,
            position,
        } => {
            database::handle_searchadd_command(state, &filters, sort.as_deref(), window, position)
                .await
        }
        Command::SearchAddPl {
            name,
            filters,
            sort,
            window,
            position,
        } => {
            playlists::handle_searchaddpl_command(
                state,
                &name,
                &filters,
                sort.as_deref(),
                window,
                position,
            )
            .await
        }
        Command::FindAdd {
            filters,
            sort,
            window,
            position,
        } => {
            database::handle_findadd_command(state, &filters, sort.as_deref(), window, position)
                .await
        }
        Command::ListFiles { uri } => {
            database::handle_listfiles_command(state, uri.as_deref()).await
        }
        Command::SearchCount { filters, group } => {
            database::handle_searchcount_command(state, &filters, group.as_deref()).await
        }
        Command::GetFingerprint { uri } => {
            fingerprint::handle_getfingerprint_command(state, &uri).await
        }
        Command::ReadComments { uri } => database::handle_readcomments_command(state, &uri).await,
        // Stickers
        Command::StickerGet {
            sticker_type,
            uri,
            name,
        } => stickers::handle_sticker_get_command(state, &sticker_type, &uri, &name).await,
        Command::StickerSet {
            sticker_type,
            uri,
            name,
            value,
        } => stickers::handle_sticker_set_command(state, &sticker_type, &uri, &name, &value).await,
        Command::StickerDelete {
            sticker_type,
            uri,
            name,
        } => {
            stickers::handle_sticker_delete_command(state, &sticker_type, &uri, name.as_deref())
                .await
        }
        Command::StickerList { sticker_type, uri } => {
            stickers::handle_sticker_list_command(state, &sticker_type, &uri).await
        }
        Command::StickerFind {
            sticker_type,
            uri,
            name,
            value,
            sort,
            window,
        } => {
            stickers::handle_sticker_find_command(
                state,
                &sticker_type,
                &uri,
                &name,
                value.as_deref(),
                sort.as_deref(),
                window,
            )
            .await
        }
        Command::StickerInc {
            sticker_type,
            uri,
            name,
            delta,
        } => stickers::handle_sticker_inc_command(state, &sticker_type, &uri, &name, delta).await,
        Command::StickerDec {
            sticker_type,
            uri,
            name,
            delta,
        } => stickers::handle_sticker_dec_command(state, &sticker_type, &uri, &name, delta).await,
        Command::StickerInvalid { sticker_type } => {
            stickers::handle_sticker_invalid_command(&sticker_type)
        }
        Command::StickerNames => stickers::handle_sticker_names_command(state).await,
        Command::StickerTypes => stickers::handle_sticker_types_command().await,
        Command::StickerNamesTypes { sticker_type } => {
            stickers::handle_sticker_namestypes_command(state, sticker_type.as_deref()).await
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
            messaging::handle_subscribe_command(state, conn_state, &channel).await
        }
        Command::Unsubscribe { channel } => {
            messaging::handle_unsubscribe_command(state, conn_state, &channel).await
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
        Command::Config => connection::handle_config_command(state, conn_state).await,
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
