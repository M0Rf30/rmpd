// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HTTP/WebSocket JSON-RPC server with a Mopidy-compatible `core.*` subset.
//!
//! ```toml
//! [[integration]]
//! name = "web"
//! type = "http"
//! # bind = "127.0.0.1:6680"                      # default
//! # allowed_origins = ["http://localhost:3000"]  # CORS / cross-origin allow-list ("*" = any)
//! # static_dir = "/usr/share/rmpd/web"           # serve a web client at /
//! # token = "secret"                             # require `Authorization: Bearer <token>`
//! ```
//!
//! Endpoints:
//!
//! * `POST /rmpd/rpc` (alias `/mopidy/rpc`): JSON-RPC 2.0 (single or batch).
//! * `GET /rmpd/ws` (alias `/mopidy/ws`): WebSocket carrying the same JSON-RPC
//!   plus pushed Mopidy events (`{"event": "volume_changed", "volume": 40}`).
//! * everything else: static files from `static_dir` (GET/HEAD only).
//!
//! The token protects the RPC and WebSocket endpoints (browsers cannot set
//! headers on a WebSocket, so `?token=` is accepted there too); static files
//! are public. Requests carrying an `Origin` header are rejected unless the
//! origin is listed in `allowed_origins` or is the server's own IP/localhost
//! origin, which blocks cross-site and DNS-rebinding attacks from browsers.

pub mod events;
pub mod model;
pub mod rpc;
#[cfg(test)]
mod testutil;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use rmpd_core::config::IntegrationConfig;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{
    Integration, IntegrationContext, IntegrationPlugin, PlayerHandle, ShutdownSignal,
};
use std::future::IntoFuture;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Settings accepted by this integration.
pub const SETTINGS: &[&str] = &["bind", "allowed_origins", "static_dir", "token"];

/// Registry entry.
pub const PLUGIN: IntegrationPlugin = IntegrationPlugin {
    name: "http",
    settings: SETTINGS,
    factory,
};

/// Default listen address (Mopidy-HTTP's port, loopback only).
pub const DEFAULT_BIND: &str = "127.0.0.1:6680";

/// Pending pushed events per WebSocket client before it skips ahead.
const EVENT_QUEUE: usize = 256;
/// Largest static file served.
const MAX_STATIC_BYTES: u64 = 64 * 1024 * 1024;

/// Parsed `[[integration]]` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    pub bind: SocketAddr,
    /// Normalised (lowercase, no trailing `/`) allowed origins.
    pub allowed_origins: Vec<String>,
    pub static_dir: Option<PathBuf>,
    pub token: Option<String>,
}

/// Read and validate the settings (no I/O).
///
/// # Errors
/// [`PluginError::Config`] for a bad `bind` or malformed `allowed_origins`.
pub fn parse_config(cfg: &IntegrationConfig) -> Result<HttpConfig, PluginError> {
    let bind_text = cfg
        .setting_str("bind")
        .unwrap_or_else(|| DEFAULT_BIND.to_owned());
    let bind: SocketAddr = bind_text.parse().map_err(|_| {
        PluginError::Config(
            "http: `bind` must be an IP:port such as 127.0.0.1:6680 or [::1]:6680".to_owned(),
        )
    })?;
    let allowed_origins = match cfg.settings.get("allowed_origins") {
        None => Vec::new(),
        Some(toml::Value::String(s)) => vec![normalize_origin(s)],
        Some(toml::Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                let toml::Value::String(s) = item else {
                    return Err(PluginError::Config(
                        "http: `allowed_origins` must be a list of strings".to_owned(),
                    ));
                };
                out.push(normalize_origin(s));
            }
            out
        }
        Some(_) => {
            return Err(PluginError::Config(
                "http: `allowed_origins` must be a list of strings".to_owned(),
            ));
        }
    }
    .into_iter()
    .filter(|o| !o.is_empty())
    .collect();
    Ok(HttpConfig {
        bind,
        allowed_origins,
        static_dir: cfg.setting_str("static_dir").map(PathBuf::from),
        token: cfg.setting_str("token"),
    })
}

fn factory(cfg: &IntegrationConfig) -> Result<Box<dyn Integration>, PluginError> {
    Ok(Box::new(HttpApi {
        name: cfg.name.clone(),
        config: parse_config(cfg)?,
    }))
}

struct HttpApi {
    name: String,
    config: HttpConfig,
}

// ─── Access control (pure helpers) ───────────────────────────────────────────

/// Lowercase, trim, and drop a trailing `/`.
#[must_use]
pub fn normalize_origin(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// `localhost` or an IP literal (with optional port): hosts a DNS-rebinding
/// attacker cannot claim.
fn is_local_authority(authority: &str) -> bool {
    let hostname = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    hostname == "localhost" || hostname.parse::<IpAddr>().is_ok()
}

/// Whether a request with this `Origin` header may proceed.
///
/// Allowed when listed (full origin or bare `host:port`; `*` allows any) or
/// when it equals the request's own `Host` and that host is `localhost`/an IP
/// literal.
#[must_use]
pub fn origin_allowed(origin: &str, host: Option<&str>, allowed: &[String]) -> bool {
    let origin = normalize_origin(origin);
    let authority = origin
        .split_once("://")
        .map_or(origin.as_str(), |(_, rest)| rest);
    if allowed
        .iter()
        .any(|a| a == "*" || *a == origin || a == authority)
    {
        return true;
    }
    host.is_some_and(|h| {
        let h = h.trim().to_ascii_lowercase();
        h == authority && is_local_authority(&h)
    })
}

/// Constant-time-ish string equality (does not short-circuit on content).
#[must_use]
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

/// Token from an `Authorization: Bearer <token>` header value.
#[must_use]
pub fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, rest) = value.trim().split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = rest.trim();
        (!token.is_empty()).then_some(token)
    } else {
        None
    }
}

fn percent_decode(input: &str, plus_is_space: bool) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(if plus_is_space && bytes[i] == b'+' {
                b' '
            } else {
                bytes[i]
            });
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `token` query parameter (for WebSocket clients that cannot set headers).
#[must_use]
pub fn query_token(query: Option<&str>) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token").then(|| percent_decode(value, true))
    })
}

fn is_api_path(path: &str) -> bool {
    path.starts_with("/rmpd/") || path.starts_with("/mopidy/")
}

/// Map a URL path to a path relative to the static root, refusing anything
/// that could leave it (`..`, backslashes, NUL, drive prefixes).
#[must_use]
pub fn sanitize_static_path(url_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(url_path, false);
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            other => {
                // Reject Windows-style prefixes and anything non-normal.
                let mut comps = Path::new(other).components();
                match (comps.next(), comps.next()) {
                    (Some(Component::Normal(_)), None) => out.push(other),
                    _ => return None,
                }
            }
        }
    }
    Some(out)
}

/// `Content-Type` for a file extension.
#[must_use]
pub fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

// ─── Server ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Shared {
    player: Arc<dyn PlayerHandle>,
    token: Option<Arc<str>>,
    allowed_origins: Arc<[String]>,
    /// Canonicalised static root, when configured and present.
    static_root: Option<Arc<PathBuf>>,
    events: broadcast::Sender<Arc<str>>,
    shutdown: ShutdownSignal,
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn status(code: StatusCode, message: &'static str) -> Response {
    (code, message).into_response()
}

fn add_cors_headers(resp: &mut Response, origin: &str) {
    if let Ok(value) = HeaderValue::from_str(origin) {
        let headers = resp.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        headers.append(header::VARY, HeaderValue::from_static("Origin"));
    }
}

/// Origin allow-list, CORS preflight, and bearer-token check.
async fn guard(State(shared): State<Shared>, req: Request, next: Next) -> Response {
    let origin = header_str(req.headers(), header::ORIGIN).map(str::to_owned);
    if let Some(origin) = &origin {
        let host = header_str(req.headers(), header::HOST);
        if !origin_allowed(origin, host, &shared.allowed_origins) {
            return status(StatusCode::FORBIDDEN, "origin not allowed");
        }
    }
    if req.method() == Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        if let Some(origin) = &origin {
            add_cors_headers(&mut resp, origin);
            let h = resp.headers_mut();
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, POST, OPTIONS"),
            );
            h.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("authorization, content-type"),
            );
            h.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_static("600"),
            );
        }
        return resp;
    }
    if let Some(expected) = &shared.token
        && is_api_path(req.uri().path())
    {
        let provided = header_str(req.headers(), header::AUTHORIZATION)
            .and_then(bearer_token)
            .map(str::to_owned)
            .or_else(|| query_token(req.uri().query()));
        if !provided.is_some_and(|t| token_eq(&t, expected)) {
            let mut resp = status(StatusCode::UNAUTHORIZED, "unauthorized");
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            if let Some(origin) = &origin {
                add_cors_headers(&mut resp, origin);
            }
            return resp;
        }
    }
    let mut resp = next.run(req).await;
    if let Some(origin) = &origin {
        add_cors_headers(&mut resp, origin);
    }
    resp
}

async fn rpc_http(State(shared): State<Shared>, body: String) -> Response {
    match rpc::handle_body(shared.player.as_ref(), &body).await {
        Some(reply) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            reply,
        )
            .into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn ws_upgrade(State(shared): State<Shared>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, shared))
}

async fn ws_session(mut socket: WebSocket, shared: Shared) {
    let mut events = shared.events.subscribe();
    let mut shutdown = shared.shutdown.clone();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Some(reply) = rpc::handle_body(shared.player.as_ref(), text.as_str()).await
                        && socket.send(Message::Text(reply.into())).await.is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            pushed = events.recv() => match pushed {
                Ok(json) => {
                    if socket.send(Message::Text(json.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

async fn static_files(State(shared): State<Shared>, req: Request) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return status(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    }
    let Some(root) = &shared.static_root else {
        return status(StatusCode::NOT_FOUND, "not found");
    };
    let Some(rel) = sanitize_static_path(req.uri().path()) else {
        return status(StatusCode::NOT_FOUND, "not found");
    };
    let mut candidate = root.join(rel);
    if tokio::fs::metadata(&candidate)
        .await
        .is_ok_and(|m| m.is_dir())
    {
        candidate.push("index.html");
    }
    // Resolve symlinks and make sure the file is still inside the root.
    let Ok(real) = tokio::fs::canonicalize(&candidate).await else {
        return status(StatusCode::NOT_FOUND, "not found");
    };
    if !real.starts_with(root.as_path()) {
        return status(StatusCode::NOT_FOUND, "not found");
    }
    match tokio::fs::metadata(&real).await {
        Ok(m) if m.is_file() && m.len() <= MAX_STATIC_BYTES => {}
        Ok(m) if m.is_file() => {
            return status(StatusCode::PAYLOAD_TOO_LARGE, "file too large");
        }
        _ => return status(StatusCode::NOT_FOUND, "not found"),
    }
    match tokio::fs::read(&real).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(&real))
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .body(Body::from(bytes))
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "error")),
        Err(_) => status(StatusCode::NOT_FOUND, "not found"),
    }
}

fn router(shared: Shared) -> Router {
    Router::new()
        .route("/rmpd/rpc", post(rpc_http))
        .route("/mopidy/rpc", post(rpc_http))
        .route("/rmpd/ws", get(ws_upgrade))
        .route("/mopidy/ws", get(ws_upgrade))
        .fallback(static_files)
        .layer(middleware::from_fn_with_state(shared.clone(), guard))
        .with_state(shared)
}

#[async_trait]
impl Integration for HttpApi {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError> {
        let HttpApi { name, config } = *self;
        let listener = TcpListener::bind(config.bind)
            .await
            .map_err(|e| PluginError::Runtime(format!("http: cannot bind {}: {e}", config.bind)))?;
        let addr = listener.local_addr().unwrap_or(config.bind);
        if !addr.ip().is_loopback() && config.token.is_none() {
            tracing::warn!(
                %name,
                %addr,
                "http api is reachable from the network without a `token`; anyone can control playback"
            );
        }
        let static_root = match &config.static_dir {
            Some(dir) => match tokio::fs::canonicalize(dir).await {
                Ok(p) => Some(Arc::new(p)),
                Err(e) => {
                    tracing::warn!(%name, "http static_dir unusable, not serving files: {e}");
                    None
                }
            },
            None => None,
        };
        let (events_tx, _) = broadcast::channel(EVENT_QUEUE);
        let shared = Shared {
            player: Arc::clone(&ctx.player),
            token: config.token.as_deref().map(Arc::from),
            allowed_origins: config.allowed_origins.into(),
            static_root,
            events: events_tx.clone(),
            shutdown: ctx.shutdown.clone(),
        };
        tracing::info!(%name, %addr, "http api listening");

        let mut stop = ctx.shutdown.clone();
        let server = axum::serve(listener, router(shared))
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .into_future();
        let pump = events::run_pump(ctx.events, ctx.player, events_tx, ctx.shutdown);
        tokio::select! {
            result = server => result
                .map_err(|e| PluginError::Runtime(format!("http: server error: {e}"))),
            () = pump => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::MockPlayer;
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn cfg_with(settings: &str) -> IntegrationConfig {
        IntegrationConfig {
            name: "web".to_owned(),
            integration_type: "http".to_owned(),
            enabled: true,
            settings: settings.parse::<toml::Table>().unwrap(),
        }
    }

    #[test]
    fn defaults() {
        let c = parse_config(&cfg_with("")).unwrap();
        assert_eq!(c.bind, DEFAULT_BIND.parse::<SocketAddr>().unwrap());
        assert!(c.allowed_origins.is_empty());
        assert!(c.static_dir.is_none());
        assert!(c.token.is_none());
    }

    #[test]
    fn parses_all_settings() {
        let c = parse_config(&cfg_with(
            r#"
            bind = "0.0.0.0:7000"
            allowed_origins = ["HTTP://Example.org/", "localhost:3000", ""]
            static_dir = "/srv/web"
            token = "s3cret"
            "#,
        ))
        .unwrap();
        assert_eq!(c.bind.port(), 7000);
        assert_eq!(c.allowed_origins, ["http://example.org", "localhost:3000"]);
        assert_eq!(c.static_dir, Some(PathBuf::from("/srv/web")));
        assert_eq!(c.token.as_deref(), Some("s3cret"));
    }

    #[test]
    fn rejects_bad_settings() {
        assert!(parse_config(&cfg_with(r#"bind = "not-an-address""#)).is_err());
        assert!(parse_config(&cfg_with("allowed_origins = [1]")).is_err());
        assert!(parse_config(&cfg_with("allowed_origins = 5")).is_err());
    }

    #[test]
    fn error_messages_do_not_echo_the_token() {
        let err = parse_config(&cfg_with("token = \"topsecret\"\nbind = \"x\"")).unwrap_err();
        assert!(!err.to_string().contains("topsecret"));
    }

    #[test]
    fn origin_rules() {
        let allowed = vec![
            "http://app.example".to_owned(),
            "other.example:81".to_owned(),
        ];
        assert!(origin_allowed("http://app.example", None, &allowed));
        assert!(origin_allowed("HTTP://APP.example/", None, &allowed));
        assert!(origin_allowed("http://other.example:81", None, &allowed));
        assert!(!origin_allowed("http://evil.example", None, &allowed));
        assert!(origin_allowed(
            "http://evil.example",
            None,
            &["*".to_owned()]
        ));
        // Same origin as a loopback/IP Host is fine ...
        assert!(origin_allowed(
            "http://127.0.0.1:6680",
            Some("127.0.0.1:6680"),
            &[]
        ));
        assert!(origin_allowed(
            "http://localhost:6680",
            Some("localhost:6680"),
            &[]
        ));
        assert!(origin_allowed("http://[::1]:6680", Some("[::1]:6680"), &[]));
        // ... but a name that merely matches Host is not (DNS rebinding).
        assert!(!origin_allowed(
            "http://attacker.example:6680",
            Some("attacker.example:6680"),
            &[]
        ));
        assert!(!origin_allowed(
            "http://127.0.0.1:1",
            Some("127.0.0.1:6680"),
            &[]
        ));
    }

    #[test]
    fn token_helpers() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
        assert!(!token_eq("", "a"));
        assert_eq!(bearer_token("Bearer xyz"), Some("xyz"));
        assert_eq!(bearer_token("bearer  xyz "), Some("xyz"));
        assert_eq!(bearer_token("Basic xyz"), None);
        assert_eq!(bearer_token("Bearer"), None);
        assert_eq!(query_token(Some("a=1&token=x%2By")), Some("x+y".to_owned()));
        assert_eq!(query_token(Some("a=1")), None);
        assert_eq!(query_token(None), None);
    }

    #[test]
    fn static_path_sanitising() {
        assert_eq!(sanitize_static_path("/"), Some(PathBuf::new()));
        assert_eq!(
            sanitize_static_path("/js/app.js"),
            Some(PathBuf::from("js/app.js"))
        );
        assert_eq!(sanitize_static_path("//a/./b"), Some(PathBuf::from("a/b")));
        assert_eq!(sanitize_static_path("/../etc/passwd"), None);
        assert_eq!(sanitize_static_path("/a/%2e%2e/b"), None);
        assert_eq!(sanitize_static_path("/a\\b"), None);
        assert_eq!(sanitize_static_path("/a%00b"), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("a/index.HTML")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("x.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("x.bin")), "application/octet-stream");
        assert_eq!(content_type(Path::new("noext")), "application/octet-stream");
    }

    #[test]
    fn registry_entry() {
        assert_eq!(PLUGIN.name, "http");
        assert!(PLUGIN.settings.contains(&"token"));
    }

    // ── Loopback integration tests ──

    async fn start(
        token: Option<&str>,
        origins: &[&str],
        static_root: Option<PathBuf>,
    ) -> SocketAddr {
        let (events, _) = broadcast::channel(8);
        let (trigger, shutdown) = rmpd_plugin::shutdown_channel();
        // Leak the trigger so the signal stays live for the whole test.
        let _leaked = Box::leak(Box::new(trigger));
        let shared = Shared {
            player: Arc::new(MockPlayer::with_queue()),
            token: token.map(Arc::from),
            allowed_origins: origins.iter().map(|s| normalize_origin(s)).collect(),
            static_root: static_root.map(Arc::new),
            events,
            shutdown,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router(shared)).await;
        });
        addr
    }

    async fn raw(addr: SocketAddr, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn post_rpc(path: &str, extra_headers: &str, body: &str) -> String {
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n{extra_headers}\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn rpc_over_http_and_alias() {
        let addr = start(None, &[], None).await;
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"core.mixer.get_volume"}"#;
        for path in ["/rmpd/rpc", "/mopidy/rpc"] {
            let resp = raw(addr, &post_rpc(path, "", body)).await;
            assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
            assert!(resp.contains(r#""result":40"#), "{resp}");
        }
    }

    #[tokio::test]
    async fn token_is_enforced_on_api_only() {
        let addr = start(Some("tok"), &[], None).await;
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"core.playback.get_state"}"#;
        let denied = raw(addr, &post_rpc("/rmpd/rpc", "", body)).await;
        assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
        let wrong = raw(
            addr,
            &post_rpc("/rmpd/rpc", "Authorization: Bearer nope\r\n", body),
        )
        .await;
        assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong}");
        let ok = raw(
            addr,
            &post_rpc("/rmpd/rpc", "Authorization: Bearer tok\r\n", body),
        )
        .await;
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("stopped"), "{ok}");
        // Static side (no static_dir configured) is not behind the token.
        let page = raw(
            addr,
            "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(page.starts_with("HTTP/1.1 404"), "{page}");
    }

    #[tokio::test]
    async fn foreign_origin_is_forbidden_and_allowed_origin_gets_cors() {
        let addr = start(None, &["http://app.example"], None).await;
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"core.get_uri_schemes"}"#;
        let evil = raw(
            addr,
            &post_rpc("/rmpd/rpc", "Origin: http://evil.example\r\n", body),
        )
        .await;
        assert!(evil.starts_with("HTTP/1.1 403"), "{evil}");
        let good = raw(
            addr,
            &post_rpc("/rmpd/rpc", "Origin: http://app.example\r\n", body),
        )
        .await;
        assert!(good.starts_with("HTTP/1.1 200"), "{good}");
        assert!(
            good.to_ascii_lowercase()
                .contains("access-control-allow-origin: http://app.example"),
            "{good}"
        );
        let preflight = raw(
            addr,
            "OPTIONS /rmpd/rpc HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Origin: http://app.example\r\n\r\n",
        )
        .await;
        assert!(preflight.starts_with("HTTP/1.1 204"), "{preflight}");
        assert!(
            preflight
                .to_ascii_lowercase()
                .contains("access-control-allow-methods"),
            "{preflight}"
        );
    }

    #[tokio::test]
    async fn static_files_are_served_and_confined() {
        let dir = std::env::temp_dir().join(format!("rmpd-http-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.join("sub/app.js"), "1").unwrap();
        let outside = dir.with_extension("secret");
        std::fs::write(&outside, "nope").unwrap();
        let root = std::fs::canonicalize(&dir).unwrap();
        let addr = start(None, &[], Some(root)).await;

        let get = |path: &str| {
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        };
        let index = raw(addr, &get("/")).await;
        assert!(index.starts_with("HTTP/1.1 200"), "{index}");
        assert!(index.contains("<h1>hi</h1>"), "{index}");
        assert!(index.to_ascii_lowercase().contains("text/html"), "{index}");
        let js = raw(addr, &get("/sub/app.js")).await;
        assert!(js.to_ascii_lowercase().contains("text/javascript"), "{js}");
        let missing = raw(addr, &get("/nope.txt")).await;
        assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
        let traversal = raw(addr, &get("/../rmpd-http-test.secret")).await;
        assert!(traversal.starts_with("HTTP/1.1 404"), "{traversal}");
        assert!(!traversal.contains("nope\n"), "{traversal}");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
    }
}
