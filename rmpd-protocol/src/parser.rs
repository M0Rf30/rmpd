use winnow::ascii::space0;
use winnow::combinator::opt;
use winnow::error::{ContextError, ErrMode};
use winnow::prelude::*;
use winnow::token::{take_till, take_while};

// Type alias for parser results (winnow 0.7 compatibility)
type PResult<O> = Result<O, ErrMode<ContextError>>;

#[derive(Debug, Clone, PartialEq, rmpd_macros::CommandMetadata)]
pub enum Command {
    // Playback control
    #[command(name = "play", permission = 16)]
    Play { position: Option<u32> },
    #[command(name = "playid", permission = 16)]
    PlayId { id: Option<u32> },
    #[command(name = "pause", permission = 16)]
    Pause { state: Option<bool> },
    #[command(name = "stop", permission = 16)]
    Stop,
    #[command(name = "next", permission = 16)]
    Next,
    #[command(name = "previous", permission = 16)]
    Previous,
    #[command(name = "seek", permission = 16)]
    Seek { position: u32, time: f64 },
    #[command(name = "seekid", permission = 16)]
    SeekId { id: u32, time: f64 },
    #[command(name = "seekcur", permission = 16)]
    SeekCur { time: f64, relative: bool },

    // Queue management
    #[command(name = "add", permission = 2)]
    Add {
        uri: String,
        position: Option<InsertPosition>,
    },
    #[command(name = "addid", permission = 2)]
    AddId {
        uri: String,
        position: Option<InsertPosition>,
    },
    #[command(name = "delete", permission = 16)]
    Delete { target: DeleteTarget },
    #[command(name = "deleteid", permission = 16)]
    DeleteId { id: u32 },
    #[command(name = "clear", permission = 16)]
    Clear,
    #[command(name = "move", permission = 16)]
    Move { from: MoveFrom, to: InsertPosition },
    #[command(name = "moveid", permission = 16)]
    MoveId { id: u32, to: InsertPosition },
    #[command(name = "shuffle", permission = 16)]
    Shuffle { range: Option<(u32, u32)> },
    #[command(name = "swap", permission = 16)]
    Swap { pos1: u32, pos2: u32 },
    #[command(name = "swapid", permission = 16)]
    SwapId { id1: u32, id2: u32 },

    // Status
    #[command(name = "status", permission = 1)]
    Status,
    #[command(name = "currentsong", permission = 1)]
    CurrentSong,
    #[command(name = "stats", permission = 1)]
    Stats,
    #[command(name = "clearerror", permission = 16)]
    ClearError,

    // Queue inspection
    #[command(name = "playlistinfo", permission = 1)]
    PlaylistInfo { range: Option<(u32, u32)> },
    #[command(name = "playlistid", permission = 1)]
    PlaylistId { id: Option<u32> },
    #[command(name = "playlist", permission = 1)]
    Playlist,
    #[command(name = "plchanges", permission = 1)]
    PlChanges {
        version: u32,
        range: Option<(u32, u32)>,
    },
    #[command(name = "plchangesposid", permission = 1)]
    PlChangesPosId {
        version: u32,
        range: Option<(u32, u32)>,
    },
    #[command(name = "playlistfind", permission = 1)]
    PlaylistFind {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
    },
    #[command(name = "playlistsearch", permission = 1)]
    PlaylistSearch {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
    },

    // Volume
    #[command(name = "setvol", permission = 16)]
    SetVol { volume: u8 },
    #[command(name = "volume", permission = 16)]
    Volume { change: i32 },
    #[command(name = "getvol", permission = 1)]
    GetVol,

    // Options
    #[command(name = "repeat", permission = 16)]
    Repeat { enabled: bool },
    #[command(name = "random", permission = 16)]
    Random { enabled: bool },
    #[command(name = "single", permission = 16)]
    Single { mode: String },
    #[command(name = "consume", permission = 16)]
    Consume { mode: String },
    #[command(name = "crossfade", permission = 16)]
    Crossfade { seconds: u32 },
    #[command(name = "replay_gain_mode", permission = 16)]
    ReplayGainMode { mode: String },
    #[command(name = "replay_gain_status", permission = 1)]
    ReplayGainStatus,

    // Connection
    #[command(name = "close")]
    Close,
    #[command(name = "ping")]
    Ping,
    #[command(name = "password")]
    Password { password: String },
    #[command(name = "binarylimit")]
    BinaryLimit { size: u32 },
    #[command(name = "protocol")]
    Protocol {
        subcommand: Option<ProtocolSubcommand>,
    },

    // Reflection
    #[command(name = "commands")]
    Commands,
    #[command(name = "notcommands")]
    NotCommands,
    #[command(name = "tagtypes")]
    TagTypes {
        subcommand: Option<TagTypesSubcommand>,
    },
    #[command(name = "urlhandlers", permission = 1)]
    UrlHandlers,
    #[command(name = "decoders", permission = 1)]
    Decoders,
    #[command(name = "stringnormalization")]
    StringNormalization {
        subcommand: Option<StringNormalizationSubcommand>,
    },

    // Database
    #[command(name = "update", permission = 4)]
    Update { path: Option<String> },
    #[command(name = "rescan", permission = 4)]
    Rescan { path: Option<String> },
    #[command(name = "find", permission = 1)]
    Find {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
    },
    #[command(name = "search", permission = 1)]
    Search {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
    },
    #[command(name = "list", permission = 1)]
    List {
        tag: String,
        filters: Vec<(String, String)>,
        groups: Vec<String>,
        window: Option<(u32, u32)>,
    },
    #[command(name = "listall", permission = 1)]
    ListAll { path: Option<String> },
    #[command(name = "listallinfo", permission = 1)]
    ListAllInfo { path: Option<String> },
    #[command(name = "lsinfo", permission = 1)]
    LsInfo { path: Option<String> },
    #[command(name = "count", permission = 1)]
    Count {
        filters: Vec<(String, String)>,
        group: Option<String>,
    },
    #[command(name = "searchcount", permission = 1)]
    SearchCount {
        filters: Vec<(String, String)>,
        group: Option<String>,
    },
    #[command(name = "getfingerprint", permission = 1)]
    GetFingerprint { uri: String },
    #[command(name = "readcomments", permission = 1)]
    ReadComments { uri: String },

    // Album art
    #[command(name = "albumart", permission = 1)]
    AlbumArt { uri: String, offset: usize },
    #[command(name = "readpicture", permission = 1)]
    ReadPicture { uri: String, offset: usize },

    // Stored playlists
    #[command(name = "save", permission = 4)]
    Save { name: String, mode: Option<String> },
    #[command(name = "load", permission = 2)]
    Load {
        name: String,
        range: Option<(u32, u32)>,
        position: Option<InsertPosition>,
    },
    #[command(name = "listplaylists", permission = 1)]
    ListPlaylists,
    #[command(name = "listplaylist", permission = 1)]
    ListPlaylist {
        name: String,
        range: Option<(u32, u32)>,
    },
    #[command(name = "listplaylistinfo", permission = 1)]
    ListPlaylistInfo {
        name: String,
        range: Option<(u32, u32)>,
    },
    #[command(name = "playlistadd", permission = 4)]
    PlaylistAdd {
        name: String,
        uri: String,
        position: Option<u32>,
    },
    #[command(name = "playlistclear", permission = 4)]
    PlaylistClear { name: String },
    #[command(name = "playlistdelete", permission = 4)]
    PlaylistDelete { name: String, range: (u32, u32) },
    #[command(name = "playlistmove", permission = 4)]
    PlaylistMove {
        name: String,
        from: (u32, u32),
        to: u32,
    },
    #[command(name = "rm", permission = 4)]
    Rm { name: String },
    #[command(name = "rename", permission = 4)]
    Rename { from: String, to: String },
    #[command(name = "searchplaylist", permission = 1)]
    SearchPlaylist {
        name: String,
        filters: Vec<(String, String)>,
        window: Option<(u32, u32)>,
    },
    #[command(name = "playlistlength", permission = 1)]
    PlaylistLength { name: String },

    // Idle notifications
    #[command(name = "idle", permission = 1)]
    Idle { subsystems: Vec<String> },
    #[command(name = "noidle")]
    NoIdle,

    // Output control
    #[command(name = "outputs", permission = 1)]
    Outputs,
    #[command(name = "enableoutput", permission = 8)]
    EnableOutput { id: u32 },
    #[command(name = "disableoutput", permission = 8)]
    DisableOutput { id: u32 },
    #[command(name = "toggleoutput", permission = 8)]
    ToggleOutput { id: u32 },
    #[command(name = "outputset", permission = 8)]
    OutputSet {
        id: u32,
        name: String,
        value: String,
    },

    // Command batching
    #[command(name = "command_list")]
    CommandListBegin,
    #[command(name = "command_list")]
    CommandListOkBegin,
    #[command(name = "command_list")]
    CommandListEnd,

    // Advanced database
    #[command(name = "searchadd", permission = 2)]
    SearchAdd {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
        position: Option<InsertPosition>,
    },
    #[command(name = "searchaddpl", permission = 4)]
    SearchAddPl {
        name: String,
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
        position: Option<u32>,
    },
    #[command(name = "findadd", permission = 2)]
    FindAdd {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
        position: Option<InsertPosition>,
    },
    #[command(name = "listfiles", permission = 1)]
    ListFiles { uri: Option<String> },

    // Sticker database
    #[command(name = "sticker", permission = 8)]
    StickerGet {
        sticker_type: String,
        uri: String,
        name: String,
    },
    #[command(name = "sticker", permission = 8)]
    StickerSet {
        sticker_type: String,
        uri: String,
        name: String,
        value: String,
    },
    #[command(name = "sticker", permission = 8)]
    StickerDelete {
        sticker_type: String,
        uri: String,
        name: Option<String>,
    },
    #[command(name = "sticker", permission = 8)]
    StickerList { sticker_type: String, uri: String },
    #[command(name = "sticker", permission = 8)]
    StickerFind {
        sticker_type: String,
        uri: String,
        name: String,
        value: Option<String>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
    },
    #[command(name = "sticker", permission = 8)]
    StickerInc {
        sticker_type: String,
        uri: String,
        name: String,
        delta: i32,
    },
    #[command(name = "sticker", permission = 8)]
    StickerDec {
        sticker_type: String,
        uri: String,
        name: String,
        delta: i32,
    },
    /// A `sticker` line whose subcommand isn't recognized. Carries
    /// `sticker_type` through so the handler validates the domain first
    /// (MPD's `handle_sticker` resolves `args[1]` into a domain handler
    /// before ever checking `args[0]`) and only reports "bad request" once
    /// the domain itself is valid.
    #[command(name = "sticker", permission = 8)]
    StickerInvalid { sticker_type: String },
    #[command(name = "stickernames", permission = 8)]
    StickerNames,
    #[command(name = "stickertypes", permission = 8)]
    StickerTypes,
    #[command(name = "stickernamestypes", permission = 8)]
    StickerNamesTypes { sticker_type: Option<String> },

    // Partitions
    #[command(name = "partition", permission = 1)]
    Partition { name: String },
    #[command(name = "listpartitions", permission = 1)]
    ListPartitions,
    #[command(name = "newpartition", permission = 8)]
    NewPartition { name: String },
    #[command(name = "delpartition", permission = 8)]
    DelPartition { name: String },
    #[command(name = "moveoutput", permission = 8)]
    MoveOutput { name: String },

    // Mounts
    #[command(name = "mount", permission = 8)]
    Mount { path: String, uri: String },
    #[command(name = "unmount", permission = 8)]
    Unmount { path: String },
    #[command(name = "listmounts", permission = 1)]
    ListMounts,
    #[command(name = "listneighbors", permission = 1)]
    ListNeighbors,

    // Client-to-client messaging
    #[command(name = "subscribe", permission = 1)]
    Subscribe { channel: String },
    #[command(name = "unsubscribe", permission = 1)]
    Unsubscribe { channel: String },
    #[command(name = "channels", permission = 1)]
    Channels,
    #[command(name = "readmessages", permission = 1)]
    ReadMessages,
    #[command(name = "sendmessage", permission = 4)]
    SendMessage { channel: String, message: String },

    // Advanced queue operations
    #[command(name = "prio", permission = 16)]
    Prio {
        priority: u8,
        ranges: Vec<(u32, u32)>,
    },
    #[command(name = "prioid", permission = 16)]
    PrioId { priority: u8, ids: Vec<u32> },
    #[command(name = "rangeid", permission = 2)]
    RangeId { id: u32, range: Option<(f64, f64)> },
    #[command(name = "addtagid", permission = 2)]
    AddTagId { id: u32, tag: String, value: String },
    #[command(name = "cleartagid", permission = 2)]
    ClearTagId { id: u32, tag: Option<String> },

    // Miscellaneous
    #[command(name = "config", permission = 8)]
    Config,
    #[command(name = "kill", permission = 8)]
    Kill,
    #[command(name = "mixrampdb", permission = 16)]
    MixRampDb { decibels: f32 },
    #[command(name = "mixrampdelay", permission = 16)]
    MixRampDelay { seconds: f32 },

    // Unknown/Invalid
    #[command(name = "unknown")]
    Unknown(String),
    #[command(name = "unknown")]
    UnknownSubcmd(String, String),
    #[command(name = "unknown")]
    ArgError(String, String, String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagTypesSubcommand {
    All,
    Clear,
    Enable { tags: Vec<String> },
    Disable { tags: Vec<String> },
    Available,
    Reset { tags: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolSubcommand {
    All,
    Clear,
    Enable { features: Vec<String> },
    Disable { features: Vec<String> },
    Available,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StringNormalizationSubcommand {
    All,
    Clear,
    Enable { options: Vec<String> },
    Disable { options: Vec<String> },
    Available,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeleteTarget {
    Position(u32),
    Range(u32, u32), // START:END (exclusive end)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoveFrom {
    Position(u32),
    Range(u32, u32), // START:END (exclusive end)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SaveMode {
    Create,  // Default: create new playlist or fail if exists
    Append,  // Append to existing playlist
    Replace, // Replace existing playlist
}

/// `load`'s POSITION argument: an absolute queue index, or a position
/// relative to the currently playing song (`+N` after, `-N` before), as
/// described for `addid` in the protocol docs. Resolved against the queue's
/// length and current song by `resolve_insert_position` in the handler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InsertPosition {
    Absolute(u32),
    After(u32),
    Before(u32),
}

thread_local! {
    /// The exact MPD-style message for the most recent argument-*value*
    /// failure (e.g. "Boolean (0/1) expected: X"), recorded by the token
    /// parsers below via `arg_error`/`record_arg_error` right before they
    /// fail. `parse_command`'s fallback surfaces it verbatim instead of a
    /// generic arity message whenever the argument count is otherwise in
    /// bounds — mirroring MPD's `command_check_request` (arity) running
    /// before the handler's own `Request::ParseXxx` (value) calls.
    static ARG_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Record `msg` as the pending argument-value error without failing (used
/// by helpers like `range_parts` that report failure via `Option`, not
/// `PResult`).
fn record_arg_error(msg: String) {
    ARG_ERROR.with(|cell| *cell.borrow_mut() = Some(msg));
}

/// Record `msg` and return a `Cut` failure. `Cut` (unlike `Backtrack`) is
/// never silently swallowed by `opt()`, so an optional argument that is
/// *present but invalid* still surfaces as a real error rather than being
/// treated as absent.
fn arg_error(msg: String) -> ErrMode<ContextError> {
    record_arg_error(msg);
    ErrMode::Cut(ContextError::default())
}

pub fn parse_command(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    ARG_ERROR.with(|cell| *cell.borrow_mut() = None);
    command_parser.parse(input).map_err(|_| {
        let cmd_name = input.split_whitespace().next().unwrap_or(input);
        let rest = &input[cmd_name.len()..];
        let Some((min, max)) = command_arity(cmd_name) else {
            // Not a real MPD command (e.g. `command_list_begin` with extra
            // arguments, or encountered a second time inside an active
            // command list): MPD looks it up in `commands[]`, finds
            // nothing, and reports it as unknown rather than a bad count.
            return format!("unknown command \"{cmd_name}\"");
        };
        let take_value_error = || ARG_ERROR.with(|cell| cell.borrow_mut().take());
        // MPD's `command_check_request` (arity) always runs before the
        // handler's own value parsing, so a genuine count mismatch wins
        // over any value error a token parser happened to record while
        // walking the (too long/too short) argument list.
        match count_args(rest) {
            // `count_args` itself couldn't tokenize the argument list (e.g.
            // an unquoted token containing a literal quote character,
            // mirroring `Tokenizer::NextUnquoted`) — the count is unknown,
            // so surface whatever specific error a primitive recorded.
            None => take_value_error()
                .unwrap_or_else(|| format!("wrong number of arguments for \"{cmd_name}\"")),
            Some(n) => {
                let n = n as i32;
                if min == max && n != min {
                    format!("wrong number of arguments for \"{cmd_name}\"")
                } else if n < min {
                    format!("too few arguments for \"{cmd_name}\"")
                } else if max >= 0 && n > max {
                    format!("too many arguments for \"{cmd_name}\"")
                } else {
                    // Argument count is within bounds: the failure is a bad
                    // argument value, not an arity error. Use the specific
                    // message a token parser recorded, if any.
                    take_value_error()
                        .unwrap_or_else(|| format!("wrong number of arguments for \"{cmd_name}\""))
                }
            }
        }
    })
}

/// Counts argument tokens after the command name, using the same
/// quote-aware tokenization as the real parsers. Returns `None` if the
/// argument list itself is malformed (e.g. an unterminated quote).
fn count_args(rest: &str) -> Option<usize> {
    let mut rest = rest;
    let mut n = 0usize;
    loop {
        let _: PResult<&str> = space0.parse_next(&mut rest);
        if rest.is_empty() {
            return Some(n);
        }
        parse_quoted_or_unquoted.parse_next(&mut rest).ok()?;
        n += 1;
    }
}

/// Per-command `(min, max)` argument-count bounds, mirroring the `commands[]`
/// table in MPD's `src/command/AllCommands.cxx` (`max == -1` means
/// unlimited). `close` and `kill` have unchecked arity in MPD (`min: -1`,
/// "don't check args") and are omitted; their parser arms consume any
/// trailing arguments instead of failing on them.
fn command_arity(name: &str) -> Option<(i32, i32)> {
    Some(match name {
        "add" | "addid" | "cleartagid" | "listplaylist" | "listplaylistinfo" => (1, 2),
        "addtagid" | "outputset" => (3, 3),
        "albumart" | "readpicture" => (2, 2),
        "binarylimit" | "consume" | "crossfade" | "delete" | "deleteid" | "delpartition"
        | "disableoutput" | "enableoutput" | "getfingerprint" | "mixrampdb" | "mixrampdelay"
        | "moveoutput" | "newpartition" | "partition" | "password" | "playlistclear"
        | "playlistlength" | "random" | "readcomments" | "repeat" | "replay_gain_mode" | "rm"
        | "setvol" | "single" | "subscribe" | "toggleoutput" | "unmount" | "unsubscribe"
        | "volume" => (1, 1),
        "channels" | "clear" | "clearerror" | "commands" | "config" | "currentsong"
        | "decoders" | "getvol" | "listmounts" | "listneighbors" | "listpartitions"
        | "listplaylists" | "next" | "notcommands" | "outputs" | "playlist" | "previous"
        | "ping" | "readmessages" | "replay_gain_status" | "stats" | "status" | "stickernames"
        | "stickertypes" | "stop" | "urlhandlers" => (0, 0),
        "count" | "find" | "findadd" | "list" | "playlistfind" | "playlistsearch" | "search"
        | "searchadd" | "searchcount" => (1, -1),
        "idle" | "protocol" | "stringnormalization" | "tagtypes" => (0, -1),
        "listall" | "listallinfo" | "listfiles" | "lsinfo" | "pause" | "play" | "playid"
        | "playlistid" | "playlistinfo" | "rescan" | "shuffle" | "stickernamestypes" | "update" => {
            (0, 1)
        }
        "load" => (1, 3),
        "mount" | "move" | "moveid" | "playlistdelete" | "rangeid" | "rename" | "seek"
        | "seekid" | "sendmessage" | "swap" | "swapid" => (2, 2),
        "plchanges" | "plchangesposid" | "save" => (1, 2),
        "playlistadd" => (2, 3),
        "playlistmove" => (3, 3),
        "sticker" => (3, -1),
        "prio" | "prioid" | "searchaddpl" => (2, -1),
        "searchplaylist" => (2, 4),
        "seekcur" => (1, 1),
        _ => return None,
    })
}

fn command_parser(input: &mut &str) -> PResult<Command> {
    let cmd = take_while(1.., |c: char| c.is_ascii_alphabetic() || c == '_').parse_next(input)?;
    let _ = space0.parse_next(input)?;

    match cmd {
        "play" => {
            // MPD treats play -1 same as play (no position) — skip negative values
            let _ = space0.parse_next(input)?;
            let pos = if input.starts_with('-') {
                // Consume the negative token and treat as no-arg
                let _ = take_while(1.., |c: char| !c.is_whitespace()).parse_next(input)?;
                None
            } else {
                opt(parse_u32_or_quoted).parse_next(input)?
            };
            Ok(Command::Play { position: pos })
        }
        "playid" => {
            // MPD treats playid -1 same as playid (no id) — skip negative values
            let _ = space0.parse_next(input)?;
            let id = if input.starts_with('-') {
                let _ = take_while(1.., |c: char| !c.is_whitespace()).parse_next(input)?;
                None
            } else {
                opt(parse_u32_or_quoted).parse_next(input)?
            };
            Ok(Command::PlayId { id })
        }
        "pause" => {
            let state = opt(parse_bool_or_quoted).parse_next(input)?;
            Ok(Command::Pause { state })
        }
        "stop" => Ok(Command::Stop),
        "next" => Ok(Command::Next),
        "previous" => Ok(Command::Previous),
        "seek" => {
            let position = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let time = parse_f64_or_quoted.parse_next(input)?;
            Ok(Command::Seek { position, time })
        }
        "seekid" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let time = parse_f64_or_quoted.parse_next(input)?;
            Ok(Command::SeekId { id, time })
        }
        "seekcur" => {
            let time_str = parse_quoted_or_unquoted.parse_next(input)?;
            let (time, relative) = if time_str.starts_with('+') || time_str.starts_with('-') {
                (
                    time_str
                        .parse()
                        .map_err(|_| ErrMode::Cut(ContextError::default()))?,
                    true,
                )
            } else {
                (
                    time_str
                        .parse()
                        .map_err(|_| ErrMode::Cut(ContextError::default()))?,
                    false,
                )
            };
            Ok(Command::SeekCur { time, relative })
        }
        "add" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let position = opt(parse_insert_position).parse_next(input)?;
            Ok(Command::Add { uri, position })
        }
        "addid" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let position = opt(parse_insert_position).parse_next(input)?;
            Ok(Command::AddId { uri, position })
        }
        "delete" => {
            let target = parse_delete_target.parse_next(input)?;
            Ok(Command::Delete { target })
        }
        "deleteid" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::DeleteId { id })
        }
        "clear" => Ok(Command::Clear),
        "move" => {
            let from = parse_move_from.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let to = parse_insert_position.parse_next(input)?;
            Ok(Command::Move { from, to })
        }
        "moveid" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let to = parse_insert_position.parse_next(input)?;
            Ok(Command::MoveId { id, to })
        }
        "shuffle" => {
            let range = opt(parse_range).parse_next(input)?;
            Ok(Command::Shuffle { range })
        }
        "swap" => {
            let pos1 = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let pos2 = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::Swap { pos1, pos2 })
        }
        "swapid" => {
            let id1 = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let id2 = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::SwapId { id1, id2 })
        }
        "status" => Ok(Command::Status),
        "currentsong" => Ok(Command::CurrentSong),
        "stats" => Ok(Command::Stats),
        "clearerror" => Ok(Command::ClearError),
        "playlistinfo" => {
            let range = opt(parse_range).parse_next(input)?;
            Ok(Command::PlaylistInfo { range })
        }
        "playlistid" => {
            let id = opt(parse_u32_or_quoted).parse_next(input)?;
            Ok(Command::PlaylistId { id })
        }
        "playlist" => Ok(Command::Playlist),
        "plchanges" => {
            let version = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let range = opt(parse_range).parse_next(input)?;
            Ok(Command::PlChanges { version, range })
        }
        "plchangesposid" => {
            let version = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let range = opt(parse_range).parse_next(input)?;
            Ok(Command::PlChangesPosId { version, range })
        }
        "playlistfind" => {
            let (filters, sort, window) = parse_find_search_filters(input)?;
            Ok(Command::PlaylistFind {
                filters,
                sort,
                window,
            })
        }
        "playlistsearch" => {
            let (filters, sort, window) = parse_find_search_filters(input)?;
            Ok(Command::PlaylistSearch {
                filters,
                sort,
                window,
            })
        }
        "setvol" => {
            let val_str = parse_quoted_or_unquoted.parse_next(input)?;
            match val_str.parse::<i64>() {
                Ok(v) if (0..=100).contains(&v) => Ok(Command::SetVol { volume: v as u8 }),
                Ok(v) => Ok(Command::ArgError(
                    "setvol".into(),
                    format!("Number too large: {v}"),
                    val_str,
                )),
                Err(_) => Ok(Command::ArgError(
                    "setvol".into(),
                    format!("Integer expected: {val_str}"),
                    val_str,
                )),
            }
        }
        "volume" => {
            let val_str = parse_quoted_or_unquoted.parse_next(input)?;
            match val_str.parse::<i32>() {
                Ok(v) if (-100..=100).contains(&v) => Ok(Command::Volume { change: v }),
                Ok(v) => Ok(Command::ArgError(
                    "volume".into(),
                    format!("Number too large: {v}"),
                    val_str,
                )),
                Err(_) => Ok(Command::ArgError(
                    "volume".into(),
                    format!("Integer expected: {val_str}"),
                    val_str,
                )),
            }
        }
        "getvol" => Ok(Command::GetVol),
        "repeat" => {
            let val = parse_quoted_or_unquoted.parse_next(input)?;
            match val.as_str() {
                "0" => Ok(Command::Repeat { enabled: false }),
                "1" => Ok(Command::Repeat { enabled: true }),
                _ => Ok(Command::ArgError(
                    "repeat".into(),
                    format!("Boolean (0/1) expected: {val}"),
                    val,
                )),
            }
        }
        "random" => {
            let val = parse_quoted_or_unquoted.parse_next(input)?;
            match val.as_str() {
                "0" => Ok(Command::Random { enabled: false }),
                "1" => Ok(Command::Random { enabled: true }),
                _ => Ok(Command::ArgError(
                    "random".into(),
                    format!("Boolean (0/1) expected: {val}"),
                    val,
                )),
            }
        }
        "single" => {
            let mode = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Single { mode })
        }
        "consume" => {
            let mode = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Consume { mode })
        }
        "crossfade" => {
            let val_str = parse_quoted_or_unquoted.parse_next(input)?;
            match val_str.parse::<i64>() {
                Ok(v) if v >= 0 => Ok(Command::Crossfade { seconds: v as u32 }),
                Ok(v) => Ok(Command::ArgError(
                    "crossfade".into(),
                    format!("Number too large: {v}"),
                    val_str,
                )),
                Err(_) => Ok(Command::ArgError(
                    "crossfade".into(),
                    format!("Integer expected: {val_str}"),
                    val_str,
                )),
            }
        }
        "replay_gain_mode" => {
            let mode = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::ReplayGainMode { mode })
        }
        "replay_gain_status" => Ok(Command::ReplayGainStatus),
        // MPD's `close`/`kill` have unchecked arity (`AllCommands.cxx`:
        // min = -1, "don't check args"): they accept and ignore any number
        // of trailing arguments instead of failing on them.
        "close" => {
            *input = "";
            Ok(Command::Close)
        }
        "ping" => Ok(Command::Ping),
        "password" => {
            let secret = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Password { password: secret })
        }
        "binarylimit" => {
            let size = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::BinaryLimit { size })
        }
        "protocol" => {
            // Check for subcommand
            if input.is_empty() {
                Ok(Command::Protocol { subcommand: None })
            } else {
                let subcommand_str = parse_quoted_or_unquoted.parse_next(input)?;
                let _ = space0.parse_next(input)?;

                match subcommand_str.as_str() {
                    "all" => Ok(Command::Protocol {
                        subcommand: Some(ProtocolSubcommand::All),
                    }),
                    "clear" => Ok(Command::Protocol {
                        subcommand: Some(ProtocolSubcommand::Clear),
                    }),
                    "available" => Ok(Command::Protocol {
                        subcommand: Some(ProtocolSubcommand::Available),
                    }),
                    "enable" => {
                        let mut features = Vec::new();
                        while !input.is_empty() {
                            let feature = parse_quoted_or_unquoted.parse_next(input)?;
                            features.push(feature);
                            let _ = space0.parse_next(input)?;
                        }
                        Ok(Command::Protocol {
                            subcommand: Some(ProtocolSubcommand::Enable { features }),
                        })
                    }
                    "disable" => {
                        let mut features = Vec::new();
                        while !input.is_empty() {
                            let feature = parse_quoted_or_unquoted.parse_next(input)?;
                            features.push(feature);
                            let _ = space0.parse_next(input)?;
                        }
                        Ok(Command::Protocol {
                            subcommand: Some(ProtocolSubcommand::Disable { features }),
                        })
                    }
                    _ => Ok(Command::UnknownSubcmd(
                        "protocol".to_string(),
                        subcommand_str,
                    )),
                }
            }
        }
        "commands" => Ok(Command::Commands),
        "notcommands" => Ok(Command::NotCommands),
        "tagtypes" => {
            // Check for subcommand
            if input.is_empty() {
                Ok(Command::TagTypes { subcommand: None })
            } else {
                // Accept quoted or unquoted subcommand
                let subcommand_str = parse_quoted_or_unquoted.parse_next(input)?;
                let _ = space0.parse_next(input)?;

                match subcommand_str.as_str() {
                    "all" => Ok(Command::TagTypes {
                        subcommand: Some(TagTypesSubcommand::All),
                    }),
                    "clear" => Ok(Command::TagTypes {
                        subcommand: Some(TagTypesSubcommand::Clear),
                    }),
                    "available" => Ok(Command::TagTypes {
                        subcommand: Some(TagTypesSubcommand::Available),
                    }),
                    "enable" => {
                        let mut tags = Vec::new();
                        while !input.is_empty() {
                            let tag = parse_quoted_or_unquoted.parse_next(input)?;
                            tags.push(tag);
                            let _ = space0.parse_next(input)?;
                        }
                        Ok(Command::TagTypes {
                            subcommand: Some(TagTypesSubcommand::Enable { tags }),
                        })
                    }
                    "disable" => {
                        let mut tags = Vec::new();
                        while !input.is_empty() {
                            let tag = parse_quoted_or_unquoted.parse_next(input)?;
                            tags.push(tag);
                            let _ = space0.parse_next(input)?;
                        }
                        Ok(Command::TagTypes {
                            subcommand: Some(TagTypesSubcommand::Disable { tags }),
                        })
                    }
                    "reset" => {
                        let mut tags = Vec::new();
                        while !input.is_empty() {
                            let tag = parse_quoted_or_unquoted.parse_next(input)?;
                            tags.push(tag);
                            let _ = space0.parse_next(input)?;
                        }
                        Ok(Command::TagTypes {
                            subcommand: Some(TagTypesSubcommand::Reset { tags }),
                        })
                    }
                    _ => Ok(Command::UnknownSubcmd(
                        "tagtypes".to_string(),
                        subcommand_str,
                    )),
                }
            }
        }
        "urlhandlers" => Ok(Command::UrlHandlers),
        "decoders" => Ok(Command::Decoders),
        "stringnormalization" => {
            if input.is_empty() {
                Ok(Command::StringNormalization { subcommand: None })
            } else {
                let subcommand_str = parse_quoted_or_unquoted.parse_next(input)?;
                let _ = space0.parse_next(input)?;

                match subcommand_str.as_str() {
                    "all" => Ok(Command::StringNormalization {
                        subcommand: Some(StringNormalizationSubcommand::All),
                    }),
                    "clear" => Ok(Command::StringNormalization {
                        subcommand: Some(StringNormalizationSubcommand::Clear),
                    }),
                    "available" => Ok(Command::StringNormalization {
                        subcommand: Some(StringNormalizationSubcommand::Available),
                    }),
                    "enable" => {
                        let mut options = Vec::new();
                        while !input.is_empty() {
                            let option = parse_quoted_or_unquoted.parse_next(input)?;
                            options.push(option);
                            let _ = space0.parse_next(input)?;
                        }
                        Ok(Command::StringNormalization {
                            subcommand: Some(StringNormalizationSubcommand::Enable { options }),
                        })
                    }
                    "disable" => {
                        let mut options = Vec::new();
                        while !input.is_empty() {
                            let option = parse_quoted_or_unquoted.parse_next(input)?;
                            options.push(option);
                            let _ = space0.parse_next(input)?;
                        }
                        Ok(Command::StringNormalization {
                            subcommand: Some(StringNormalizationSubcommand::Disable { options }),
                        })
                    }
                    _ => Ok(Command::UnknownSubcmd(
                        "stringnormalization".to_string(),
                        subcommand_str,
                    )),
                }
            }
        }
        "update" => {
            let path = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::Update { path })
        }
        "rescan" => {
            let path = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::Rescan { path })
        }
        "find" => {
            let (filters, sort, window) = parse_find_search_filters(input)?;
            Ok(Command::Find {
                filters,
                sort,
                window,
            })
        }
        "search" => {
            let (filters, sort, window) = parse_find_search_filters(input)?;
            Ok(Command::Search {
                filters,
                sort,
                window,
            })
        }
        "list" => {
            let tag = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Collect the rest as a flat token list, then apply MPD's exact
            // order of operations: strip trailing "window", then check the
            // legacy 3-arg form, then strip repeated trailing "group TAG".
            let mut tokens: Vec<String> = Vec::new();
            loop {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    break;
                }
                tokens.push(parse_quoted_or_unquoted.parse_next(input)?);
            }

            let window = if tokens.len() >= 2 && tokens[tokens.len() - 2] == "window" {
                let range_tok = tokens.pop().expect("checked len >= 2");
                tokens.pop();
                Some(range_parts(&range_tok).ok_or_else(|| ErrMode::Cut(ContextError::default()))?)
            } else {
                None
            };

            // Legacy (< 0.12) 3-arg form: `list album ARTIST` — a single
            // bare (non-expression) remaining token with no tag name.
            if tokens.len() == 1 && !tokens[0].starts_with('(') {
                if !tag.eq_ignore_ascii_case("album") {
                    return Err(ErrMode::Cut(ContextError::default()));
                }
                let value = tokens.pop().expect("checked len == 1");
                return Ok(Command::List {
                    tag,
                    filters: vec![("artist".to_string(), value)],
                    groups: Vec::new(),
                    window,
                });
            }

            let mut groups = Vec::new();
            while tokens.len() >= 2 && tokens[tokens.len() - 2] == "group" {
                groups.push(tokens.pop().expect("checked len >= 2"));
                tokens.pop();
            }
            groups.reverse();

            let filters = if tokens.len() == 1 && tokens[0].starts_with('(') {
                vec![(tokens.pop().expect("checked len == 1"), String::new())]
            } else {
                let mut pairs = Vec::new();
                let mut it = tokens.into_iter();
                while let Some(t) = it.next() {
                    let Some(v) = it.next() else {
                        return Err(ErrMode::Cut(ContextError::default()));
                    };
                    pairs.push((t, v));
                }
                pairs
            };

            Ok(Command::List {
                tag,
                filters,
                groups,
                window,
            })
        }
        "listall" => {
            let path = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::ListAll { path })
        }
        "listallinfo" => {
            let path = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::ListAllInfo { path })
        }
        "lsinfo" => {
            let path = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::LsInfo { path })
        }
        "count" => {
            let (filters, group) = parse_count_filters(input)?;
            Ok(Command::Count { filters, group })
        }
        "searchcount" => {
            let (filters, group) = parse_count_filters(input)?;
            Ok(Command::SearchCount { filters, group })
        }
        "getfingerprint" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::GetFingerprint { uri })
        }
        "readcomments" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::ReadComments { uri })
        }
        "albumart" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let offset_str = parse_quoted_or_unquoted.parse_next(input)?;
            let offset = offset_str
                .parse::<usize>()
                .map_err(|_| ErrMode::Cut(ContextError::default()))?;
            Ok(Command::AlbumArt { uri, offset })
        }
        "readpicture" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let offset_str = parse_quoted_or_unquoted.parse_next(input)?;
            let offset = offset_str
                .parse::<usize>()
                .map_err(|_| ErrMode::Cut(ContextError::default()))?;
            Ok(Command::ReadPicture { uri, offset })
        }
        "save" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // The mode token is captured verbatim here; `handle_save_command`
            // validates it against 'create'/'append'/'replace' (case-sensitive,
            // matching MPD) and reports MPD's exact ACK text on a bad value.
            let mode = if !input.is_empty() {
                Some(parse_quoted_or_unquoted.parse_next(input)?.to_string())
            } else {
                None
            };

            Ok(Command::Save { name, mode })
        }
        "load" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // RANGE and POSITION are purely positional, matching MPD's
            // `Request::ParseOptional(1, RangeArg::All())`: a bare number in
            // the second slot is a single-song range (e.g. "5" -> [5,6)),
            // not a position — POSITION only exists once RANGE is present.
            let range = opt(parse_range).parse_next(input)?;
            let _ = space0.parse_next(input)?;

            let position = opt(parse_insert_position).parse_next(input)?;

            Ok(Command::Load {
                name,
                range,
                position,
            })
        }
        "listplaylists" => Ok(Command::ListPlaylists),
        "listplaylist" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let range = opt(parse_range).parse_next(input)?;
            Ok(Command::ListPlaylist { name, range })
        }
        "listplaylistinfo" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let range = opt(parse_range).parse_next(input)?;
            Ok(Command::ListPlaylistInfo { name, range })
        }
        "playlistadd" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let position = opt(parse_u32_or_quoted).parse_next(input)?;
            Ok(Command::PlaylistAdd {
                name,
                uri,
                position,
            })
        }
        "playlistclear" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::PlaylistClear { name })
        }
        "playlistdelete" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            // MPD 0.24 accepts a range here, not just a single index; a bare
            // number is a single-song range (matching `playlistmove`/list*).
            let range = parse_range.parse_next(input)?;
            Ok(Command::PlaylistDelete { name, range })
        }
        "playlistmove" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            // MPD 0.24 accepts a range for FROM, not just a single index.
            let from = parse_range.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let to = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::PlaylistMove { name, from, to })
        }
        "rm" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Rm { name })
        }
        "rename" => {
            let name1 = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let name2 = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Rename {
                from: name1,
                to: name2,
            })
        }
        "searchplaylist" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            // Same filter grammar as `find`/`search`; `searchplaylist` has no
            // `sort` clause, so the parsed sort (if any) is simply discarded.
            let (filters, _sort, window) = parse_find_search_filters(input)?;
            Ok(Command::SearchPlaylist {
                name,
                filters,
                window,
            })
        }
        "playlistlength" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::PlaylistLength { name })
        }
        "idle" => {
            // Parse optional subsystem list
            let mut subsystems = Vec::new();
            while !input.is_empty() {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    break;
                }
                let subsystem = parse_quoted_or_unquoted.parse_next(input)?;
                if !subsystem.is_empty() {
                    subsystems.push(subsystem);
                }
            }
            Ok(Command::Idle { subsystems })
        }
        "noidle" => Ok(Command::NoIdle),
        "outputs" => Ok(Command::Outputs),
        "enableoutput" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::EnableOutput { id })
        }
        "disableoutput" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::DisableOutput { id })
        }
        "toggleoutput" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::ToggleOutput { id })
        }
        "outputset" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::OutputSet { id, name, value })
        }
        "command_list_begin" => Ok(Command::CommandListBegin),
        "command_list_ok_begin" => Ok(Command::CommandListOkBegin),
        "command_list_end" => Ok(Command::CommandListEnd),
        // Advanced database
        "searchadd" => {
            let (filters, sort, window, position) = parse_add_filters(input)?;
            Ok(Command::SearchAdd {
                filters,
                sort,
                window,
                position,
            })
        }
        "searchaddpl" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let (filters, sort, window, position) = parse_searchaddpl_filters(input)?;
            Ok(Command::SearchAddPl {
                name,
                filters,
                sort,
                window,
                position,
            })
        }
        "findadd" => {
            let (filters, sort, window, position) = parse_add_filters(input)?;
            Ok(Command::FindAdd {
                filters,
                sort,
                window,
                position,
            })
        }
        "listfiles" => {
            let uri = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::ListFiles { uri })
        }
        // Stickers
        "sticker" => {
            let operation = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let sticker_type = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            match operation.as_str() {
                "get" => {
                    let name = parse_quoted_or_unquoted.parse_next(input)?;
                    Ok(Command::StickerGet {
                        sticker_type,
                        uri,
                        name,
                    })
                }
                "set" => {
                    let name = parse_quoted_or_unquoted.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let value = parse_quoted_or_unquoted.parse_next(input)?;
                    Ok(Command::StickerSet {
                        sticker_type,
                        uri,
                        name,
                        value,
                    })
                }
                "delete" => {
                    let name = opt(parse_quoted_or_unquoted).parse_next(input)?;
                    Ok(Command::StickerDelete {
                        sticker_type,
                        uri,
                        name,
                    })
                }
                "list" => Ok(Command::StickerList { sticker_type, uri }),
                "find" => {
                    let name = parse_quoted_or_unquoted.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    // Optional filter `OP VALUE` (`sticker/Match.hxx`'s
                    // StickerOperator): "=" | "<" | ">" | "eq" | "lt" | "gt" |
                    // "contains" | "starts_with". Anything else at this
                    // position (including "sort"/"window") means no filter.
                    // Known operators are encoded as "op\x00val" in the value
                    // field so the handler can decode them without another
                    // Command enum change.
                    let saved = *input;
                    let value = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                        Some(op)
                            if matches!(
                                op.as_str(),
                                "=" | "<" | ">" | "eq" | "lt" | "gt" | "contains" | "starts_with"
                            ) =>
                        {
                            let _ = space0.parse_next(input)?;
                            let val = parse_quoted_or_unquoted.parse_next(input)?;
                            Some(format!("{op}\x00{val}"))
                        }
                        Some(tok) if tok == "sort" || tok == "window" || tok.is_empty() => {
                            *input = saved;
                            None
                        }
                        Some(_) => return Err(ErrMode::Cut(ContextError::default())),
                        None => None,
                    };
                    let (sort, window) = parse_sort_window(input)?;
                    Ok(Command::StickerFind {
                        sticker_type,
                        uri,
                        name,
                        value,
                        sort,
                        window,
                    })
                }
                "inc" => {
                    let name = parse_quoted_or_unquoted.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    // The delta argument is mandatory in real MPD (missing it
                    // falls through to "bad request", matching an unrecognized
                    // subcommand); a present-but-non-numeric value is still a
                    // valid call, coerced like SQLite's `value + ?` (0 for junk).
                    match opt(parse_quoted_or_unquoted).parse_next(input)? {
                        Some(value_str) => Ok(Command::StickerInc {
                            sticker_type,
                            uri,
                            name,
                            delta: sticker_delta_cast(&value_str),
                        }),
                        None => {
                            *input = "";
                            Ok(Command::StickerInvalid { sticker_type })
                        }
                    }
                }
                "dec" => {
                    let name = parse_quoted_or_unquoted.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    match opt(parse_quoted_or_unquoted).parse_next(input)? {
                        Some(value_str) => Ok(Command::StickerDec {
                            sticker_type,
                            uri,
                            name,
                            delta: sticker_delta_cast(&value_str),
                        }),
                        None => {
                            *input = "";
                            Ok(Command::StickerInvalid { sticker_type })
                        }
                    }
                }
                _ => {
                    // Domain is validated before the subcommand in real MPD
                    // (StickerCommands.cxx resolves args[1] first); consume
                    // any trailing tokens since the handler now owns the
                    // "unknown domain" vs "bad request" distinction.
                    *input = "";
                    Ok(Command::StickerInvalid { sticker_type })
                }
            }
        }
        "stickernames" => Ok(Command::StickerNames),
        "stickertypes" => Ok(Command::StickerTypes),
        "stickernamestypes" => {
            let sticker_type = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::StickerNamesTypes { sticker_type })
        }
        // Partitions
        "partition" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Partition { name })
        }
        "listpartitions" => Ok(Command::ListPartitions),
        "newpartition" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::NewPartition { name })
        }
        "delpartition" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::DelPartition { name })
        }
        "moveoutput" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::MoveOutput { name })
        }
        // Mounts
        "mount" => {
            let path = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Mount { path, uri })
        }
        "unmount" => {
            let path = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Unmount { path })
        }
        "listmounts" => Ok(Command::ListMounts),
        "listneighbors" => Ok(Command::ListNeighbors),
        // Client messaging
        "subscribe" => {
            let channel = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Subscribe { channel })
        }
        "unsubscribe" => {
            let channel = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Unsubscribe { channel })
        }
        "channels" => Ok(Command::Channels),
        "readmessages" => Ok(Command::ReadMessages),
        "sendmessage" => {
            let channel = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let message = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::SendMessage { channel, message })
        }
        // Advanced queue
        "prio" => {
            let priority = parse_u8_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Parse first range (required)
            let first_range = parse_range.parse_next(input)?;
            let mut ranges = vec![first_range];

            // Parse additional ranges (optional)
            loop {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    break;
                }
                match opt(parse_range).parse_next(input)? {
                    Some(range) => ranges.push(range),
                    None => break,
                }
            }

            Ok(Command::Prio { priority, ranges })
        }
        "prioid" => {
            let priority = parse_u8_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Parse first ID (required)
            let first_id = parse_u32_or_quoted.parse_next(input)?;
            let mut ids = vec![first_id];

            // Parse additional IDs (optional)
            loop {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    break;
                }
                match opt(parse_u32_or_quoted).parse_next(input)? {
                    Some(id) => ids.push(id),
                    None => break,
                }
            }

            Ok(Command::PrioId { priority, ids })
        }
        "rangeid" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let tok = parse_quoted_or_unquoted.parse_next(input)?;
            let range = parse_song_range(&tok).ok_or_else(|| arg_error("Bad range".to_string()))?;
            Ok(Command::RangeId { id, range })
        }
        "addtagid" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let tag = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::AddTagId { id, tag, value })
        }
        "cleartagid" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let tag = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::ClearTagId { id, tag })
        }
        // Miscellaneous
        "config" => Ok(Command::Config),
        "kill" => {
            *input = "";
            Ok(Command::Kill)
        }
        "mixrampdb" => {
            let decibels = parse_f64_or_quoted.parse_next(input)? as f32;
            Ok(Command::MixRampDb { decibels })
        }
        "mixrampdelay" => {
            let seconds = parse_f64_or_quoted.parse_next(input)? as f32;
            Ok(Command::MixRampDelay { seconds })
        }
        _ => Ok(Command::Unknown(cmd.to_string())),
    }
}

/// Build MPD's exact message for a failed unsigned-integer parse ("Integer
/// expected: X" for non-numeric, "Number too large: X" for overflow),
/// matching `ParseCommandArgUnsigned` (ArgParser.cxx). `s` is always the
/// whole original token, matching MPD's `MakeArgError`.
fn integer_error(s: &str, kind: &std::num::IntErrorKind) -> String {
    match kind {
        std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
            format!("Number too large: {s}")
        }
        _ => format!("Integer expected: {s}"),
    }
}

fn parse_u32_or_quoted(input: &mut &str) -> PResult<u32> {
    let s = parse_quoted_or_unquoted.parse_next(input)?;
    if s.is_empty() {
        return Err(ErrMode::Backtrack(ContextError::default()));
    }
    s.parse()
        .map_err(|e: std::num::ParseIntError| arg_error(integer_error(&s, e.kind())))
}

fn parse_u8_or_quoted(input: &mut &str) -> PResult<u8> {
    let s = parse_quoted_or_unquoted.parse_next(input)?;
    if s.is_empty() {
        return Err(ErrMode::Backtrack(ContextError::default()));
    }
    s.parse()
        .map_err(|e: std::num::ParseIntError| arg_error(integer_error(&s, e.kind())))
}

/// Parse a range argument (quote-aware): `START:END`, `START:`, or a bare
/// `NUM` (single position → `[NUM, NUM+1)`). libmpdclient quotes every
/// argument, so the whole token may arrive as `"5:10"`.
fn parse_range(input: &mut &str) -> PResult<(u32, u32)> {
    let tok = parse_quoted_or_unquoted.parse_next(input)?;
    // A range token is unambiguous once present, so a malformed or inverted
    // range must hard-fail (Cut) rather than backtrack into "no range given".
    range_parts(&tok).ok_or(ErrMode::Cut(ContextError::default()))
}

/// Parse a POSITION argument shared by `add`/`addid` (insert position) and
/// the `move`/`moveid` destination: `+N`/`-N` (relative to the current
/// song, as `ParseInsertPosition`/`ParseMoveDestination` do in MPD's
/// `PositionArg.cxx`) or a plain absolute index. Resolved to an absolute
/// queue index by the caller (`commands::queue`, `commands::playlists`).
fn parse_insert_position(input: &mut &str) -> PResult<InsertPosition> {
    let tok = parse_quoted_or_unquoted.parse_next(input)?;
    if tok.is_empty() {
        return Err(ErrMode::Backtrack(ContextError::default()));
    }
    let num = |digits: &str| -> Result<u32, ErrMode<ContextError>> {
        digits
            .parse()
            .map_err(|e: std::num::ParseIntError| arg_error(integer_error(&tok, e.kind())))
    };
    if let Some(rest) = tok.strip_prefix('+') {
        num(rest).map(InsertPosition::After)
    } else if let Some(rest) = tok.strip_prefix('-') {
        num(rest).map(InsertPosition::Before)
    } else {
        num(&tok).map(InsertPosition::Absolute)
    }
}

fn parse_delete_target(input: &mut &str) -> PResult<DeleteTarget> {
    let tok = parse_quoted_or_unquoted.parse_next(input)?;
    let (start, end) = range_parts(&tok).ok_or_else(|| ErrMode::Cut(ContextError::default()))?;
    if tok.contains(':') {
        Ok(DeleteTarget::Range(start, end))
    } else {
        Ok(DeleteTarget::Position(start))
    }
}

fn parse_move_from(input: &mut &str) -> PResult<MoveFrom> {
    let tok = parse_quoted_or_unquoted.parse_next(input)?;
    let (start, end) = range_parts(&tok).ok_or_else(|| ErrMode::Cut(ContextError::default()))?;
    if tok.contains(':') {
        Ok(MoveFrom::Range(start, end))
    } else {
        Ok(MoveFrom::Position(start))
    }
}

/// Parse `rangeid`'s `START:END` token: fractional-second offsets, both
/// optional, mirroring MPD's `parse_time_range` (QueueCommands.cxx). An
/// omitted side defaults to `0`; `end == 0` means "no upper bound". A token
/// that resolves to `(0, 0)` (e.g. a bare `":"`) means "clear the range,
/// play the whole song" and is reported as `None`.
fn parse_song_range(s: &str) -> Option<Option<(f64, f64)>> {
    let (start_str, end_str) = s.split_once(':')?;
    let start: f64 = if start_str.is_empty() {
        0.0
    } else {
        start_str.parse().ok()?
    };
    let end: f64 = if end_str.is_empty() {
        0.0
    } else {
        end_str.parse().ok()?
    };
    if start < 0.0 || end < 0.0 || !(end == 0.0 || end > start) {
        return None;
    }
    if start == 0.0 && end == 0.0 {
        Some(None)
    } else {
        Some(Some((start, end)))
    }
}

/// Parse the content of a range token (`"START:END"`, `"START:"`, or bare
/// `"NUM"`) into a half-open `[start, end)` pair. A bare number yields
/// `[NUM, NUM+1)`. Returns `None` on malformed input or when `start > end`.
fn range_parts(s: &str) -> Option<(u32, u32)> {
    // A range token is unambiguous once present, so any failure below
    // records MPD's exact `ParseCommandArgRange` (ArgParser.cxx) message;
    // callers turn `None` into a `Cut` failure so `opt()` never swallows it.
    let parse_component = |tok: &str| -> Option<u32> {
        match tok.parse::<u32>() {
            Ok(v) => Some(v),
            Err(e) => {
                let msg = if *e.kind() == std::num::IntErrorKind::PosOverflow {
                    format!("Number too large: {s}")
                } else {
                    format!("Integer or range expected: {s}")
                };
                record_arg_error(msg);
                None
            }
        }
    };
    let (start, end) = match s.split_once(':') {
        Some((a, b)) => {
            let start = parse_component(a)?;
            let end = if b.is_empty() {
                u32::MAX
            } else {
                parse_component(b)?
            };
            (start, end)
        }
        None => {
            let start = parse_component(s)?;
            (start, start.saturating_add(1))
        }
    };
    // MPD rejects inverted ranges (e.g. "5:2") rather than treating them as
    // empty or auto-swapping the bounds.
    if start > end {
        record_arg_error(format!("Malformed range: {s}"));
        return None;
    }
    Some((start, end))
}

/// Parse the filters, optional `sort TAG`, and optional `window START:END` for
/// the `find` and `search` commands. The two commands are syntactically
/// identical; the caller wraps the result in `Command::Find` or `Command::Search`.
fn parse_find_search_filters(
    input: &mut &str,
) -> PResult<(Vec<(String, String)>, Option<String>, Option<(u32, u32)>)> {
    let tag = parse_quoted_or_unquoted.parse_next(input)?;
    let _ = space0.parse_next(input)?;

    let filters = if tag.starts_with('(') {
        // Filter expression: the whole (…) expression is a single filter token.
        vec![(tag, String::new())]
    } else {
        // Traditional syntax: tag value [tag value ...] [sort TAG] [window START:END]
        let mut filters = Vec::new();
        let value = parse_quoted_or_unquoted
            .parse_next(input)
            .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
        filters.push((tag, value));

        loop {
            let _ = space0.parse_next(input)?;
            if input.is_empty() {
                break;
            }
            let saved_input = *input;
            let next_token = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                Some(t) if !t.is_empty() => t,
                _ => break,
            };
            if next_token == "sort" || next_token == "window" {
                *input = saved_input;
                break;
            }
            let _ = space0.parse_next(input)?;
            let next_value = parse_quoted_or_unquoted
                .parse_next(input)
                .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
            filters.push((next_token, next_value));
        }
        filters
    };

    let (sort, window) = parse_sort_window(input)?;
    Ok((filters, sort, window))
}

/// Coerce a `sticker inc`/`dec` value token the way SQLite's `value + ?`
/// arithmetic does: a leading optional sign plus digits, 0 for anything else.
fn sticker_delta_cast(s: &str) -> i32 {
    let trimmed = s.trim_start();
    let mut end = 0;
    for (i, c) in trimmed.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    trimmed[..end].parse().unwrap_or(0)
}

/// Parse optional trailing `sort TAG` and `window START:END` clauses
/// (quote-aware, any order) shared by `find`/`search`. Stops at end of input
/// or an unrecognised keyword (which it leaves unconsumed).
fn parse_sort_window(input: &mut &str) -> PResult<(Option<String>, Option<(u32, u32)>)> {
    let mut sort = None;
    let mut window = None;
    loop {
        let _ = space0.parse_next(input)?;
        if input.is_empty() {
            break;
        }
        let saved_input = *input;
        let keyword = match opt(parse_quoted_or_unquoted).parse_next(input)? {
            Some(k) if !k.is_empty() => k,
            _ => break,
        };
        match keyword.as_str() {
            "sort" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    // Mirrors MPD's song::Filter treating a bare trailing
                    // `sort`/`window` keyword as an unpaired filter tag.
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                sort = Some(parse_quoted_or_unquoted.parse_next(input)?);
            }
            "window" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                window = Some(parse_range.parse_next(input)?);
            }
            _ => {
                *input = saved_input;
                break;
            }
        }
    }
    Ok((sort, window))
}

/// Parse the filters and optional trailing `group GROUPTYPE` for `count`
/// and `searchcount`. Filter grammar is identical to `find`/`search`; MPD's
/// `count` accepts no `sort`/`window`, only `group`.
fn parse_count_filters(input: &mut &str) -> PResult<(Vec<(String, String)>, Option<String>)> {
    let tag = parse_quoted_or_unquoted.parse_next(input)?;
    let _ = space0.parse_next(input)?;

    if tag == "group" && !input.is_empty() {
        // MPD only recognizes `group` as the trailing-clause marker when a
        // tag name follows it (`args[size - 2] == "group"` in
        // `handle_count_internal`); a bare "count group" with nothing after
        // falls through to filter parsing instead, where the lone "group"
        // token becomes a valueless filter tag.
        let group = parse_quoted_or_unquoted.parse_next(input)?;
        return Ok((Vec::new(), Some(group)));
    }

    let filters = if tag.starts_with('(') {
        vec![(tag, String::new())]
    } else {
        let mut filters = Vec::new();
        let value = parse_quoted_or_unquoted
            .parse_next(input)
            .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
        filters.push((tag, value));

        loop {
            let _ = space0.parse_next(input)?;
            if input.is_empty() {
                break;
            }
            let saved_input = *input;
            let next_token = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                Some(t) if !t.is_empty() => t,
                _ => break,
            };
            if next_token == "group" && !input.trim_start().is_empty() {
                *input = saved_input;
                break;
            }
            let _ = space0.parse_next(input)?;
            let next_value = parse_quoted_or_unquoted
                .parse_next(input)
                .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
            filters.push((next_token, next_value));
        }
        filters
    };

    let _ = space0.parse_next(input)?;
    let group = if !input.is_empty() {
        let saved = *input;
        let keyword = opt(parse_quoted_or_unquoted).parse_next(input)?;
        if keyword.as_deref() == Some("group") && !input.trim_start().is_empty() {
            let _ = space0.parse_next(input)?;
            Some(parse_quoted_or_unquoted.parse_next(input)?)
        } else {
            *input = saved;
            None
        }
    } else {
        None
    };

    Ok((filters, group))
}

/// Parse the filter (parenthesized expression or legacy `TAG VALUE [...]`
/// pairs), plus trailing `sort`/`window`/`position` clauses — shared by
/// `findadd`/`searchadd`/`searchaddpl`. Filter grammar is identical to
/// `find`/`search`; 0.24 adds the optional `position` clause.
fn parse_add_filters(
    input: &mut &str,
) -> PResult<(
    Vec<(String, String)>,
    Option<String>,
    Option<(u32, u32)>,
    Option<InsertPosition>,
)> {
    let tag = parse_quoted_or_unquoted.parse_next(input)?;
    let _ = space0.parse_next(input)?;

    let filters = if tag.starts_with('(') {
        vec![(tag, String::new())]
    } else {
        let mut filters = Vec::new();
        let value = parse_quoted_or_unquoted
            .parse_next(input)
            .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
        filters.push((tag, value));

        loop {
            let _ = space0.parse_next(input)?;
            if input.is_empty() {
                break;
            }
            let saved_input = *input;
            let next_token = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                Some(t) if !t.is_empty() => t,
                _ => break,
            };
            if next_token == "sort" || next_token == "window" || next_token == "position" {
                *input = saved_input;
                break;
            }
            let _ = space0.parse_next(input)?;
            let next_value = parse_quoted_or_unquoted
                .parse_next(input)
                .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
            filters.push((next_token, next_value));
        }
        filters
    };

    let (sort, window, position) = parse_sort_window_position(input)?;
    Ok((filters, sort, window, position))
}

/// Parse optional trailing `sort TAG`, `window START:END`, and `position
/// POS` clauses (quote-aware, any order); like [`parse_sort_window`] but
/// also accepts the 0.24 `position` clause used by `findadd`/`searchadd`/
/// `searchaddpl`.
fn parse_sort_window_position(
    input: &mut &str,
) -> PResult<(Option<String>, Option<(u32, u32)>, Option<InsertPosition>)> {
    let mut sort = None;
    let mut window = None;
    let mut position = None;
    loop {
        let _ = space0.parse_next(input)?;
        if input.is_empty() {
            break;
        }
        let saved_input = *input;
        let keyword = match opt(parse_quoted_or_unquoted).parse_next(input)? {
            Some(k) if !k.is_empty() => k,
            _ => break,
        };
        match keyword.as_str() {
            "sort" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                sort = Some(parse_quoted_or_unquoted.parse_next(input)?);
            }
            "window" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                window = Some(parse_range.parse_next(input)?);
            }
            "position" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                position = Some(parse_insert_position.parse_next(input)?);
            }
            _ => {
                *input = saved_input;
                break;
            }
        }
    }
    Ok((sort, window, position))
}

/// Like [`parse_add_filters`], but for `searchaddpl`: `position` is a plain
/// absolute index (no `+N`/`-N`) since it inserts into a stored playlist
/// file, not the queue — there's no "current song" to be relative to
/// (unlike `searchadd`/`findadd`, `playlistadd` uses a plain index too).
fn parse_searchaddpl_filters(
    input: &mut &str,
) -> PResult<(
    Vec<(String, String)>,
    Option<String>,
    Option<(u32, u32)>,
    Option<u32>,
)> {
    let tag = parse_quoted_or_unquoted.parse_next(input)?;
    let _ = space0.parse_next(input)?;

    let filters = if tag.starts_with('(') {
        vec![(tag, String::new())]
    } else {
        let mut filters = Vec::new();
        let value = parse_quoted_or_unquoted
            .parse_next(input)
            .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
        filters.push((tag, value));

        loop {
            let _ = space0.parse_next(input)?;
            if input.is_empty() {
                break;
            }
            let saved_input = *input;
            let next_token = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                Some(t) if !t.is_empty() => t,
                _ => break,
            };
            if next_token == "sort" || next_token == "window" || next_token == "position" {
                *input = saved_input;
                break;
            }
            let _ = space0.parse_next(input)?;
            let next_value = parse_quoted_or_unquoted
                .parse_next(input)
                .map_err(|_| arg_error("Incorrect number of filter arguments".to_string()))?;
            filters.push((next_token, next_value));
        }
        filters
    };

    let mut sort = None;
    let mut window = None;
    let mut position = None;
    loop {
        let _ = space0.parse_next(input)?;
        if input.is_empty() {
            break;
        }
        let saved_input = *input;
        let keyword = match opt(parse_quoted_or_unquoted).parse_next(input)? {
            Some(k) if !k.is_empty() => k,
            _ => break,
        };
        match keyword.as_str() {
            "sort" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                sort = Some(parse_quoted_or_unquoted.parse_next(input)?);
            }
            "window" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                window = Some(parse_range.parse_next(input)?);
            }
            "position" => {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    return Err(arg_error(
                        "Incorrect number of filter arguments".to_string(),
                    ));
                }
                position = Some(parse_u32_or_quoted.parse_next(input)?);
            }
            _ => {
                *input = saved_input;
                break;
            }
        }
    }
    Ok((filters, sort, window, position))
}

fn parse_f64_or_quoted(input: &mut &str) -> PResult<f64> {
    let s = parse_quoted_or_unquoted.parse_next(input)?;
    if s.is_empty() {
        return Err(ErrMode::Backtrack(ContextError::default()));
    }
    s.parse()
        .map_err(|_| arg_error(format!("Float expected: {s}")))
}

fn parse_bool_or_quoted(input: &mut &str) -> PResult<bool> {
    let s = parse_quoted_or_unquoted.parse_next(input)?;
    match s.as_str() {
        "0" => Ok(false),
        "1" => Ok(true),
        "" => Err(ErrMode::Backtrack(ContextError::default())),
        _ => Err(arg_error(format!("Boolean (0/1) expected: {s}"))),
    }
}

fn parse_string(input: &mut &str) -> PResult<String> {
    let tok =
        take_till(1.., |c: char| c.is_whitespace() || c == '\n' || c == '\r').parse_next(input)?;
    if tok.contains(['"', '\'']) {
        // Mirrors `Tokenizer::NextUnquoted`: an unquoted token may not
        // contain a literal quote character.
        return Err(arg_error("Invalid unquoted character".to_string()));
    }
    Ok(tok.to_string())
}

fn parse_quoted_or_unquoted(input: &mut &str) -> PResult<String> {
    if input.starts_with('"') {
        parse_quoted_string.parse_next(input)
    } else {
        parse_string.parse_next(input)
    }
}

fn parse_quoted_string(input: &mut &str) -> PResult<String> {
    let _ = '"'.parse_next(input)?;
    let mut result = String::new();
    let mut chars = input.chars();
    let mut consumed = 0;
    loop {
        match chars.next() {
            Some('"') => {
                consumed += 1;
                break;
            }
            Some('\\') => {
                consumed += 1;
                // Backslash escapes the following character
                match chars.next() {
                    Some(c) => {
                        consumed += c.len_utf8();
                        result.push(c);
                    }
                    None => return Err(arg_error("Missing closing '\"'".to_string())),
                }
            }
            Some(c) => {
                consumed += c.len_utf8();
                result.push(c);
            }
            None => return Err(arg_error("Missing closing '\"'".to_string())),
        }
    }
    *input = &input[consumed..];
    // Mirrors `Tokenizer::NextString`: a closing quote must be followed by
    // whitespace or end-of-line, e.g. `"foo"bar` is rejected.
    if !input.is_empty() && !input.starts_with(|c: char| c.is_whitespace()) {
        return Err(arg_error("Space expected after closing '\"'".to_string()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_play_command() {
        assert_eq!(
            parse_command("play").unwrap(),
            Command::Play { position: None }
        );
        assert_eq!(
            parse_command("play 5").unwrap(),
            Command::Play { position: Some(5) }
        );
    }

    #[test]
    fn test_pause_command() {
        assert_eq!(
            parse_command("pause").unwrap(),
            Command::Pause { state: None }
        );
        assert_eq!(
            parse_command("pause 1").unwrap(),
            Command::Pause { state: Some(true) }
        );
        assert_eq!(
            parse_command("pause 0").unwrap(),
            Command::Pause { state: Some(false) }
        );
    }

    #[test]
    fn test_add_command() {
        assert_eq!(
            parse_command("add song.mp3").unwrap(),
            Command::Add {
                uri: "song.mp3".to_string(),
                position: None
            }
        );
    }

    #[test]
    fn test_add_command_with_quotes() {
        assert_eq!(
            parse_command(r#"add "/home/user/song with spaces.mp3""#).unwrap(),
            Command::Add {
                uri: "/home/user/song with spaces.mp3".to_string(),
                position: None
            }
        );
    }

    #[test]
    fn test_add_command_with_path() {
        assert_eq!(
            parse_command("add /home/user/song.mp3").unwrap(),
            Command::Add {
                uri: "/home/user/song.mp3".to_string(),
                position: None
            }
        );
    }

    #[test]
    fn test_status_command() {
        assert_eq!(parse_command("status").unwrap(), Command::Status);
    }

    #[test]
    fn test_shuffle_with_range() {
        assert_eq!(
            parse_command("shuffle 5:10").unwrap(),
            Command::Shuffle {
                range: Some((5, 10))
            }
        );
        assert_eq!(
            parse_command("shuffle").unwrap(),
            Command::Shuffle { range: None }
        );
    }

    #[test]
    fn test_playlistinfo_with_range() {
        assert_eq!(
            parse_command("playlistinfo 0:5").unwrap(),
            Command::PlaylistInfo {
                range: Some((0, 5))
            }
        );
        assert_eq!(
            parse_command("playlistinfo").unwrap(),
            Command::PlaylistInfo { range: None }
        );
    }

    #[test]
    fn test_plchanges_with_range() {
        assert_eq!(
            parse_command("plchanges 0 5:10").unwrap(),
            Command::PlChanges {
                version: 0,
                range: Some((5, 10))
            }
        );
        assert_eq!(
            parse_command("plchanges 10").unwrap(),
            Command::PlChanges {
                version: 10,
                range: None
            }
        );
    }

    #[test]
    fn test_prio_with_multiple_ranges() {
        assert_eq!(
            parse_command("prio 10 5:10").unwrap(),
            Command::Prio {
                priority: 10,
                ranges: vec![(5, 10)]
            }
        );
        assert_eq!(
            parse_command("prio 10 5:10 15:20").unwrap(),
            Command::Prio {
                priority: 10,
                ranges: vec![(5, 10), (15, 20)]
            }
        );
        assert_eq!(
            parse_command("prio 255 0:5 10:15 20:25").unwrap(),
            Command::Prio {
                priority: 255,
                ranges: vec![(0, 5), (10, 15), (20, 25)]
            }
        );
    }

    #[test]
    fn test_prioid_with_multiple_ids() {
        assert_eq!(
            parse_command("prioid 10 5").unwrap(),
            Command::PrioId {
                priority: 10,
                ids: vec![5]
            }
        );
        assert_eq!(
            parse_command("prioid 10 5 15").unwrap(),
            Command::PrioId {
                priority: 10,
                ids: vec![5, 15]
            }
        );
        assert_eq!(
            parse_command("prioid 255 1 2 3 4 5").unwrap(),
            Command::PrioId {
                priority: 255,
                ids: vec![1, 2, 3, 4, 5]
            }
        );
    }

    #[test]
    fn test_find_with_sort_and_window() {
        assert_eq!(
            parse_command("find artist Metallica").unwrap(),
            Command::Find {
                filters: vec![("artist".to_string(), "Metallica".to_string())],
                sort: None,
                window: None
            }
        );
        assert_eq!(
            parse_command("find artist Metallica sort album").unwrap(),
            Command::Find {
                filters: vec![("artist".to_string(), "Metallica".to_string())],
                sort: Some("album".to_string()),
                window: None
            }
        );
        assert_eq!(
            parse_command("find artist Metallica window 0:10").unwrap(),
            Command::Find {
                filters: vec![("artist".to_string(), "Metallica".to_string())],
                sort: None,
                window: Some((0, 10))
            }
        );
        assert_eq!(
            parse_command("find artist Metallica sort album window 0:10").unwrap(),
            Command::Find {
                filters: vec![("artist".to_string(), "Metallica".to_string())],
                sort: Some("album".to_string()),
                window: Some((0, 10))
            }
        );
    }

    #[test]
    fn test_count_with_filters_and_group() {
        assert_eq!(
            parse_command("count artist Metallica").unwrap(),
            Command::Count {
                filters: vec![("artist".to_string(), "Metallica".to_string())],
                group: None
            }
        );
        assert_eq!(
            parse_command("count artist Metallica group album").unwrap(),
            Command::Count {
                filters: vec![("artist".to_string(), "Metallica".to_string())],
                group: Some("album".to_string())
            }
        );
        assert_eq!(
            parse_command("count artist Metallica album \"Master of Puppets\"").unwrap(),
            Command::Count {
                filters: vec![
                    ("artist".to_string(), "Metallica".to_string()),
                    ("album".to_string(), "Master of Puppets".to_string())
                ],
                group: None
            }
        );
    }

    // libmpdclient (used by mympd, mpc, ncmpcpp, …) quotes *every* command
    // argument, and MPD's tokenizer accepts quoted or unquoted uniformly. These
    // guard the two commands whose parsers were not quote-aware, which broke the
    // mympd connect handshake with a spurious "wrong number of arguments".
    #[test]
    fn test_binarylimit_accepts_quoted_argument() {
        assert_eq!(
            parse_command("binarylimit 8192").unwrap(),
            Command::BinaryLimit { size: 8192 }
        );
        assert_eq!(
            parse_command("binarylimit \"8192\"").unwrap(),
            Command::BinaryLimit { size: 8192 }
        );
    }

    #[test]
    fn test_protocol_accepts_quoted_subcommand_and_features() {
        // Bare (legacy) forms still parse.
        assert_eq!(
            parse_command("protocol").unwrap(),
            Command::Protocol { subcommand: None }
        );
        assert_eq!(
            parse_command("protocol available").unwrap(),
            Command::Protocol {
                subcommand: Some(ProtocolSubcommand::Available)
            }
        );
        // Quoted subcommand (what libmpdclient actually sends).
        assert_eq!(
            parse_command("protocol \"available\"").unwrap(),
            Command::Protocol {
                subcommand: Some(ProtocolSubcommand::Available)
            }
        );
        assert_eq!(
            parse_command("protocol \"enable\" \"hide_playlists_in_root\"").unwrap(),
            Command::Protocol {
                subcommand: Some(ProtocolSubcommand::Enable {
                    features: vec!["hide_playlists_in_root".to_string()]
                })
            }
        );
    }
    #[test]
    fn test_quoted_arguments_libmpdclient() {
        // libmpdclient (used by mympd, etc.) quotes EVERY argument, including
        // numbers, ranges, positions and filter expressions. All must parse.
        assert_eq!(
            parse_command("playlistinfo \"5:10\"").unwrap(),
            Command::PlaylistInfo {
                range: Some((5, 10))
            }
        );
        assert_eq!(
            parse_command("playlistinfo \"5\"").unwrap(),
            Command::PlaylistInfo {
                range: Some((5, 6))
            }
        );
        assert_eq!(
            parse_command("playlistid \"3\"").unwrap(),
            Command::PlaylistId { id: Some(3) }
        );
        assert!(matches!(
            parse_command("delete \"5:10\"").unwrap(),
            Command::Delete {
                target: DeleteTarget::Range(5, 10)
            }
        ));
        assert!(matches!(
            parse_command("delete \"5\"").unwrap(),
            Command::Delete {
                target: DeleteTarget::Position(5)
            }
        ));
        assert!(matches!(
            parse_command("move \"5:10\" \"2\"").unwrap(),
            Command::Move {
                from: MoveFrom::Range(5, 10),
                to: InsertPosition::Absolute(2)
            }
        ));
        assert_eq!(
            parse_command("seekcur \"123.5\"").unwrap(),
            Command::SeekCur {
                time: 123.5,
                relative: false
            }
        );
        assert_eq!(
            parse_command("seekcur \"+10\"").unwrap(),
            Command::SeekCur {
                time: 10.0,
                relative: true
            }
        );
        // find/search with a quoted window, including the (expression) form
        assert_eq!(
            parse_command("find \"artist\" \"Metallica\" window \"0:10\"").unwrap(),
            Command::Find {
                filters: vec![("artist".to_string(), "Metallica".to_string())],
                sort: None,
                window: Some((0, 10)),
            }
        );
        assert_eq!(
            parse_command("search \"(Album == \\\"x\\\")\" window \"0:100\"").unwrap(),
            Command::Search {
                filters: vec![("(Album == \"x\")".to_string(), String::new())],
                sort: None,
                window: Some((0, 100)),
            }
        );
        // sticker: libmpdclient quotes the subcommand and type too
        assert_eq!(
            parse_command("sticker \"list\" \"song\" \"foo/bar.flac\"").unwrap(),
            Command::StickerList {
                sticker_type: "song".to_string(),
                uri: "foo/bar.flac".to_string()
            }
        );
        assert_eq!(
            parse_command("sticker \"set\" \"song\" \"foo.flac\" \"rating\" \"10\"").unwrap(),
            Command::StickerSet {
                sticker_type: "song".to_string(),
                uri: "foo.flac".to_string(),
                name: "rating".to_string(),
                value: "10".to_string(),
            }
        );
        assert_eq!(
            parse_command("sticker \"get\" \"song\" \"foo.flac\" \"rating\"").unwrap(),
            Command::StickerGet {
                sticker_type: "song".to_string(),
                uri: "foo.flac".to_string(),
                name: "rating".to_string(),
            }
        );
    }

    #[test]
    fn test_range_parts_rejects_inverted_range() {
        assert_eq!(range_parts("5:2"), None);
        assert_eq!(range_parts("2:5"), Some((2, 5)));
        assert_eq!(range_parts("3:3"), Some((3, 3)));
        assert_eq!(range_parts("5:"), Some((5, u32::MAX)));
    }

    #[test]
    fn test_inverted_range_rejected_by_commands() {
        assert!(parse_command("playlistinfo 5:2").is_err());
        assert!(parse_command("delete 5:2").is_err());
        assert!(parse_command("move 5:2 0").is_err());
    }
}
