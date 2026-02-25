use winnow::ascii::space0;
use winnow::combinator::opt;
use winnow::error::{ContextError, ErrMode};
use winnow::prelude::*;
use winnow::token::{take_till, take_while};

// Type alias for parser results (winnow 0.7 compatibility)
type PResult<O> = Result<O, ErrMode<ContextError>>;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    // Playback control
    Play {
        position: Option<u32>,
    },
    PlayId {
        id: Option<u32>,
    },
    Pause {
        state: Option<bool>,
    },
    Stop,
    Next,
    Previous,
    Seek {
        position: u32,
        time: f64,
    },
    SeekId {
        id: u32,
        time: f64,
    },
    SeekCur {
        time: f64,
        relative: bool,
    },

    // Queue management
    Add {
        uri: String,
        position: Option<u32>,
    },
    AddId {
        uri: String,
        position: Option<u32>,
    },
    Delete {
        target: DeleteTarget,
    },
    DeleteId {
        id: u32,
    },
    Clear,
    Move {
        from: MoveFrom,
        to: u32,
    },
    MoveId {
        id: u32,
        to: u32,
    },
    Shuffle {
        range: Option<(u32, u32)>,
    },
    Swap {
        pos1: u32,
        pos2: u32,
    },
    SwapId {
        id1: u32,
        id2: u32,
    },

    // Status
    Status,
    CurrentSong,
    Stats,
    ClearError,

    // Queue inspection
    PlaylistInfo {
        range: Option<(u32, u32)>,
    },
    PlaylistId {
        id: Option<u32>,
    },
    Playlist, // Deprecated, use PlaylistInfo
    PlChanges {
        version: u32,
        range: Option<(u32, u32)>,
    },
    PlChangesPosId {
        version: u32,
        range: Option<(u32, u32)>,
    },
    PlaylistFind {
        tag: String,
        value: String,
    },
    PlaylistSearch {
        tag: String,
        value: String,
    },

    // Volume
    SetVol {
        volume: u8,  // validated [0, 100] in parser
    },
    Volume {
        change: i32,  // validated [-100, 100] in parser
    },
    GetVol,

    // Options
    Repeat {
        enabled: bool,
    },
    Random {
        enabled: bool,
    },
    Single {
        mode: String,
    },
    Consume {
        mode: String,
    },
    Crossfade {
        seconds: u32,
    },
    ReplayGainMode {
        mode: String,
    },
    ReplayGainStatus,

    // Connection
    Close,
    Ping,
    Password {
        password: String,
    },
    BinaryLimit {
        size: u32,
    },
    Protocol {
        subcommand: Option<ProtocolSubcommand>,
    },

    // Reflection
    Commands,
    NotCommands,
    TagTypes {
        subcommand: Option<TagTypesSubcommand>,
    },
    UrlHandlers,
    Decoders,

    // Database
    Update {
        path: Option<String>,
    },
    Rescan {
        path: Option<String>,
    },
    Find {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
    },
    Search {
        filters: Vec<(String, String)>,
        sort: Option<String>,
        window: Option<(u32, u32)>,
    },
    List {
        tag: String,
        filter_tag: Option<String>,
        filter_value: Option<String>,
        group: Option<String>,
    },
    ListAll {
        path: Option<String>,
    },
    ListAllInfo {
        path: Option<String>,
    },
    LsInfo {
        path: Option<String>,
    },
    Count {
        filters: Vec<(String, String)>,
        group: Option<String>,
    },
    SearchCount {
        tag: String,
        value: String,
        group: Option<String>,
    },
    GetFingerprint {
        uri: String,
    },
    ReadComments {
        uri: String,
    },

    // Album art
    AlbumArt {
        uri: String,
        offset: usize,
    },
    ReadPicture {
        uri: String,
        offset: usize,
    },

    // Stored playlists
    Save {
        name: String,
        mode: Option<SaveMode>,
    },
    Load {
        name: String,
        range: Option<(u32, u32)>,
        position: Option<u32>,
    },
    ListPlaylists,
    ListPlaylist {
        name: String,
        range: Option<(u32, u32)>,
    },
    ListPlaylistInfo {
        name: String,
        range: Option<(u32, u32)>,
    },
    PlaylistAdd {
        name: String,
        uri: String,
        position: Option<u32>,
    },
    PlaylistClear {
        name: String,
    },
    PlaylistDelete {
        name: String,
        position: u32,
    },
    PlaylistMove {
        name: String,
        from: u32,
        to: u32,
    },
    Rm {
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
    SearchPlaylist {
        name: String,
        tag: String,
        value: String,
    },
    PlaylistLength {
        name: String,
    },

    // Idle notifications
    Idle {
        subsystems: Vec<String>,
    },
    NoIdle,

    // Output control
    Outputs,
    EnableOutput {
        id: u32,
    },
    DisableOutput {
        id: u32,
    },
    ToggleOutput {
        id: u32,
    },
    OutputSet {
        id: u32,
        name: String,
        value: String,
    },

    // Command batching
    CommandListBegin,
    CommandListOkBegin,
    CommandListEnd,

    // Advanced database
    SearchAdd {
        tag: String,
        value: String,
    },
    SearchAddPl {
        name: String,
        tag: String,
        value: String,
    },
    FindAdd {
        tag: String,
        value: String,
    },
    ListFiles {
        uri: Option<String>,
    },

    // Sticker database
    StickerGet {
        uri: String,
        name: String,
    },
    StickerSet {
        uri: String,
        name: String,
        value: String,
    },
    StickerDelete {
        uri: String,
        name: Option<String>,
    },
    StickerList {
        uri: String,
    },
    StickerFind {
        uri: String,
        name: String,
        value: Option<String>,
    },
    StickerInc {
        uri: String,
        name: String,
        delta: Option<i32>,
    },
    StickerDec {
        uri: String,
        name: String,
        delta: Option<i32>,
    },
    StickerNames {
        uri: Option<String>,
    },
    StickerTypes,
    StickerNamesTypes {
        uri: Option<String>,
    },

    // Partitions
    Partition {
        name: String,
    },
    ListPartitions,
    NewPartition {
        name: String,
    },
    DelPartition {
        name: String,
    },
    MoveOutput {
        name: String,
    },

    // Mounts
    Mount {
        path: String,
        uri: String,
    },
    Unmount {
        path: String,
    },
    ListMounts,
    ListNeighbors,

    // Client-to-client messaging
    Subscribe {
        channel: String,
    },
    Unsubscribe {
        channel: String,
    },
    Channels,
    ReadMessages,
    SendMessage {
        channel: String,
        message: String,
    },

    // Advanced queue operations
    Prio {
        priority: u8,
        ranges: Vec<(u32, u32)>,
    },
    PrioId {
        priority: u8,
        ids: Vec<u32>,
    },
    RangeId {
        id: u32,
        range: (f64, f64),
    },
    AddTagId {
        id: u32,
        tag: String,
        value: String,
    },
    ClearTagId {
        id: u32,
        tag: Option<String>,
    },

    // Miscellaneous
    Config,
    Kill,
    MixRampDb {
        decibels: f32,
    },
    MixRampDelay {
        seconds: f32,
    },

    // Unknown/Invalid
    Unknown(String),
    /// Unknown subcommand for a known command (e.g. `tagtypes list`, `protocol list`)
    /// Fields: (main_command, unknown_subcommand)
    UnknownSubcmd(String, String),
    /// Argument validation error (ACK [2]) — command parsed ok but arg value is invalid.
    /// Fields: (command_name, error_message, raw_arg)
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

pub fn parse_command(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    command_parser.parse(input).map_err(|_| {
        // Extract just the command name (first token) for a useful error message.
        // If parsing fails after the command name is known, it is almost always an
        // arg-count mismatch, so report "wrong number of arguments for \"cmd\"".
        let cmd_name = input
            .split_whitespace()
            .next()
            .unwrap_or(input);
        // Commands with min=max args use "wrong number"; those with min<max use
        // "too few". We use a small lookup table to match MPD exactly.
        let too_few = matches!(
            cmd_name,
            "addid" | "add" | "find" | "search" | "list" | "findadd" | "searchadd"
                | "searchaddpl" | "listplaylist" | "listplaylistinfo"
                | "playlistlength" | "searchplaylist"
                | "rename" | "load" | "save" | "playlistfind" | "playlistsearch"
                | "rename" | "load" | "save" | "playlistfind" | "playlistsearch"
        );
        if too_few {
            format!("too few arguments for \"{cmd_name}\"")
        } else {
            format!("wrong number of arguments for \"{cmd_name}\"")
        }
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
            let time_str = take_till(0.., |c: char| c.is_whitespace()).parse_next(input)?;
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
            let position = opt(parse_u32).parse_next(input)?;
            Ok(Command::Add { uri, position })
        }
        "addid" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let position = opt(parse_u32_or_quoted).parse_next(input)?;
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
            let to = parse_u32.parse_next(input)?;
            Ok(Command::Move { from, to })
        }
        "moveid" => {
            let id = parse_u32_or_quoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let to = parse_u32_or_quoted.parse_next(input)?;
            Ok(Command::MoveId { id, to })
        }
        "shuffle" => {
            let range = opt(parse_range).parse_next(input)?;
            Ok(Command::Shuffle { range })
        }
        "swap" => {
            let pos1 = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let pos2 = parse_u32.parse_next(input)?;
            Ok(Command::Swap { pos1, pos2 })
        }
        "swapid" => {
            let id1 = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let id2 = parse_u32.parse_next(input)?;
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
            let id = opt(parse_u32).parse_next(input)?;
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
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::PlaylistFind { tag, value })
        }
        "playlistsearch" => {
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::PlaylistSearch { tag, value })
        }
        "setvol" => {
            let val_str = parse_quoted_or_unquoted.parse_next(input)?;
            match val_str.parse::<i64>() {
                Ok(v) if v >= 0 && v <= 100 => Ok(Command::SetVol { volume: v as u8 }),
                Ok(v) => Ok(Command::ArgError(
                    "setvol".into(),
                    format!("Number too large: {v}"),
                    val_str,
                )),
                Err(_) => Ok(Command::ArgError(
                    "setvol".into(),
                    format!("Number too large: {val_str}"),
                    val_str,
                )),
            }
        }
        "volume" => {
            let val_str = parse_quoted_or_unquoted.parse_next(input)?;
            match val_str.parse::<i32>() {
                Ok(v) if v >= -100 && v <= 100 => Ok(Command::Volume { change: v }),
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
            let mode = parse_string.parse_next(input)?;
            Ok(Command::ReplayGainMode { mode })
        }
        "replay_gain_status" => Ok(Command::ReplayGainStatus),
        "close" => Ok(Command::Close),
        "ping" => Ok(Command::Ping),
        "password" => {
            let password = parse_string.parse_next(input)?;
            Ok(Command::Password { password })
        }
        "binarylimit" => {
            let size = parse_u32.parse_next(input)?;
            Ok(Command::BinaryLimit { size })
        }
        "protocol" => {
            // Check for subcommand
            if input.is_empty() {
                Ok(Command::Protocol { subcommand: None })
            } else {
                let subcommand_str =
                    take_while(1.., |c: char| c.is_ascii_alphabetic()).parse_next(input)?;
                let _ = space0.parse_next(input)?;

                match subcommand_str {
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
                    _ => Ok(Command::UnknownSubcmd("protocol".to_string(), subcommand_str.to_string())),
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
                    _ => Ok(Command::UnknownSubcmd("tagtypes".to_string(), subcommand_str)),
                }
            }
        }
        "urlhandlers" => Ok(Command::UrlHandlers),
        "decoders" => Ok(Command::Decoders),
        "update" => {
            let path = opt(parse_string).parse_next(input)?;
            Ok(Command::Update { path })
        }
        "rescan" => {
            let path = opt(parse_string).parse_next(input)?;
            Ok(Command::Rescan { path })
        }
        "find" => {
            let tag = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Check if this is a filter expression (starts with '(')
            if tag.starts_with('(') {
                // Filter expression - treat as single filter
                Ok(Command::Find {
                    filters: vec![(tag, String::new())],
                    sort: None,
                    window: None,
                })
            } else {
                // Traditional syntax: tag value [tag value ...] [sort TAG] [window START:END]
                let mut filters = Vec::new();
                let value = parse_quoted_or_unquoted.parse_next(input)?;
                filters.push((tag, value));

                // Parse additional tag-value pairs until we hit sort/window keywords
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

                    // Check for sort or window keywords
                    if next_token == "sort" || next_token == "window" {
                        *input = saved_input;
                        break;
                    }

                    let _ = space0.parse_next(input)?;
                    let next_value = parse_quoted_or_unquoted.parse_next(input)?;
                    filters.push((next_token, next_value));
                }

                // Parse optional sort and window
                let mut sort = None;
                let mut window = None;

                loop {
                    let _ = space0.parse_next(input)?;
                    if input.is_empty() {
                        break;
                    }

                    let saved_input = *input;
                    let keyword = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                        Some(k) => k,
                        None => break,
                    };

                    match keyword.as_str() {
                        "sort" => {
                            let _ = space0.parse_next(input)?;
                            sort = Some(parse_quoted_or_unquoted.parse_next(input)?);
                        }
                        "window" => {
                            let _ = space0.parse_next(input)?;
                            window = Some(parse_range.parse_next(input)?);
                        }
                        _ => {
                            *input = saved_input;
                            break;
                        }
                    }
                }

                Ok(Command::Find {
                    filters,
                    sort,
                    window,
                })
            }
        }
        "search" => {
            let tag = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Check if this is a filter expression (starts with '(')
            if tag.starts_with('(') {
                // Filter expression - treat as single filter
                Ok(Command::Search {
                    filters: vec![(tag, String::new())],
                    sort: None,
                    window: None,
                })
            } else {
                // Traditional syntax: tag value [tag value ...] [sort TAG] [window START:END]
                let mut filters = Vec::new();
                let value = parse_quoted_or_unquoted.parse_next(input)?;
                filters.push((tag, value));

                // Parse additional tag-value pairs until we hit sort/window keywords
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

                    // Check for sort or window keywords
                    if next_token == "sort" || next_token == "window" {
                        *input = saved_input;
                        break;
                    }

                    let _ = space0.parse_next(input)?;
                    let next_value = parse_quoted_or_unquoted.parse_next(input)?;
                    filters.push((next_token, next_value));
                }

                // Parse optional sort and window
                let mut sort = None;
                let mut window = None;

                loop {
                    let _ = space0.parse_next(input)?;
                    if input.is_empty() {
                        break;
                    }

                    let saved_input = *input;
                    let keyword = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                        Some(k) => k,
                        None => break,
                    };

                    match keyword.as_str() {
                        "sort" => {
                            let _ = space0.parse_next(input)?;
                            sort = Some(parse_quoted_or_unquoted.parse_next(input)?);
                        }
                        "window" => {
                            let _ = space0.parse_next(input)?;
                            window = Some(parse_range.parse_next(input)?);
                        }
                        _ => {
                            *input = saved_input;
                            break;
                        }
                    }
                }

                Ok(Command::Search {
                    filters,
                    sort,
                    window,
                })
            }
        }
        "list" => {
            let tag = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Try to parse optional filter or group
            let saved_input = *input;
            let next_token = opt(parse_quoted_or_unquoted)
                .parse_next(input)?
                .filter(|s| !s.is_empty()); // Filter out empty strings

            let (filter_tag, filter_value, group) = if next_token.as_deref() == Some("group") {
                // Format: list TAG group GROUPTYPE
                let _ = space0.parse_next(input)?;
                let group_type = opt(parse_quoted_or_unquoted)
                    .parse_next(input)?
                    .filter(|s| !s.is_empty());
                (None, None, group_type)
            } else if let Some(ref ft) = next_token {
                if ft.starts_with('(') {
                    // Filter expression: list TAG "(expr)" [group GROUPTYPE]
                    let _ = space0.parse_next(input)?;

                    // Check for optional "group" keyword
                    let saved_input2 = *input;
                    let group_keyword = opt(parse_quoted_or_unquoted)
                        .parse_next(input)?
                        .filter(|s| !s.is_empty());

                    let group_type = if group_keyword.as_deref() == Some("group") {
                        let _ = space0.parse_next(input)?;
                        opt(parse_quoted_or_unquoted)
                            .parse_next(input)?
                            .filter(|s| !s.is_empty())
                    } else {
                        *input = saved_input2;
                        None
                    };

                    (Some(next_token.unwrap()), None, group_type)
                } else {
                    // Traditional: list TAG FILTER_TAG FILTER_VALUE [t2 v2 ...] [group GROUPTYPE]
                    let _ = space0.parse_next(input)?;
                    let fv = parse_quoted_or_unquoted.parse_next(input)?;
                    let _ = space0.parse_next(input)?;

                    // Collect additional filter pairs (MPD legacy multi-filter support)
                    let mut extra_pairs: Vec<(String, String)> = Vec::new();
                    loop {
                        let saved_loop = *input;
                        let maybe_tag = opt(parse_quoted_or_unquoted).parse_next(input)?.filter(|s| !s.is_empty());
                        match maybe_tag {
                            None => break,
                            Some(ref t) if t == "group" => {
                                *input = saved_loop;
                                break;
                            }
                            Some(t) => {
                                // looks like another filter tag - read its value
                                let _ = space0.parse_next(input)?;
                                match opt(parse_quoted_or_unquoted).parse_next(input)? {
                                    Some(v) if !v.is_empty() => {
                                        extra_pairs.push((t, v));
                                        let _ = space0.parse_next(input)?;
                                    }
                                    _ => {
                                        // Couldn't read value; backtrack and stop
                                        *input = saved_loop;
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // Check for optional "group" keyword
                    let saved_input2 = *input;
                    let group_keyword = opt(parse_quoted_or_unquoted)
                        .parse_next(input)?
                        .filter(|s| !s.is_empty());

                    let group_type = if group_keyword.as_deref() == Some("group") {
                        let _ = space0.parse_next(input)?;
                        opt(parse_quoted_or_unquoted)
                            .parse_next(input)?
                            .filter(|s| !s.is_empty())
                    } else {
                        *input = saved_input2;
                        None
                    };

                    // If we have extra pairs, build a combined expression string
                    let (ft_out, fv_out) = if extra_pairs.is_empty() {
                        (Some(next_token.unwrap()), Some(fv))
                    } else {
                        // Build AND filter expression from all pairs
                        let first_tag = next_token.unwrap();
                        let mut expr = format!("({} == {:?})", first_tag, fv);
                        for (et, ev) in &extra_pairs {
                            expr.push_str(&format!(" AND ({} == {:?})", et, ev));
                        }
                        (Some(expr), None)
                    };

                    (ft_out, fv_out, group_type)
                }
            } else {
                // Format: list TAG
                *input = saved_input;
                (None, None, None)
            };

            Ok(Command::List {
                tag,
                filter_tag,
                filter_value,
                group,
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
            let first = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Check if this is a filter expression (starts with '(')
            if first.starts_with('(') {
                // Filter expression - treat as single filter
                let filters = vec![(first, String::new())];

                // Parse optional group
                let group = if !input.is_empty() {
                    let saved = *input;
                    let keyword = opt(parse_quoted_or_unquoted).parse_next(input)?;
                    if keyword.as_deref() == Some("group") {
                        let _ = space0.parse_next(input)?;
                        opt(parse_quoted_or_unquoted).parse_next(input)?
                    } else {
                        *input = saved;
                        None
                    }
                } else {
                    None
                };

                Ok(Command::Count { filters, group })
            } else {
                // Traditional syntax: TAG VALUE [TAG VALUE ...] [group GROUPTAG]
                let mut filters = Vec::new();
                // Check for "group" keyword on the first token
                if first == "group" {
                    let _ = space0.parse_next(input)?;
                    let group = opt(parse_quoted_or_unquoted).parse_next(input)?;
                    return Ok(Command::Count { filters, group });
                }
                let value = parse_quoted_or_unquoted.parse_next(input)?;
                filters.push((first, value));

                // Parse additional tag-value pairs
                loop {
                    let _ = space0.parse_next(input)?;
                    if input.is_empty() {
                        break;
                    }
                    let saved_input = *input;
                    let tag = match opt(parse_quoted_or_unquoted).parse_next(input)? {
                        Some(t) if !t.is_empty() => t,
                        _ => break,
                    };
                    if tag == "group" {
                        *input = saved_input;
                        break;
                    }

                    let _ = space0.parse_next(input)?;
                    let next_value = parse_quoted_or_unquoted.parse_next(input)?;
                    filters.push((tag, next_value));
                }

                // Parse optional group
                let _ = space0.parse_next(input)?;
                let group = if !input.is_empty() {
                    let keyword = opt(parse_quoted_or_unquoted).parse_next(input)?;
                    if keyword.as_deref() == Some("group") {
                        let _ = space0.parse_next(input)?;
                        opt(parse_quoted_or_unquoted).parse_next(input)?
                    } else {
                        None
                    }
                } else {
                    None
                };

                Ok(Command::Count { filters, group })
            }
        }
        "searchcount" => {
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            // Parse optional "group GROUPTAG" at end
            let group = if !input.is_empty() {
                let saved = *input;
                let keyword = opt(parse_quoted_or_unquoted).parse_next(input)?;
                if keyword.as_deref() == Some("group") {
                    let _ = space0.parse_next(input)?;
                    opt(parse_quoted_or_unquoted).parse_next(input)?
                } else {
                    *input = saved;
                    None
                }
            } else {
                None
            };
            Ok(Command::SearchCount { tag, value, group })
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

            // Try to parse optional mode
            let mode = if !input.is_empty() {
                let mode_str = parse_quoted_or_unquoted.parse_next(input)?;
                match mode_str.to_lowercase().as_str() {
                    "create" => Some(SaveMode::Create),
                    "append" => Some(SaveMode::Append),
                    "replace" => Some(SaveMode::Replace),
                    _ => return Err(ErrMode::Cut(ContextError::default())),
                }
            } else {
                None
            };

            Ok(Command::Save { name, mode })
        }
        "load" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Try to parse optional range (START:END) — must contain colon
            // to distinguish from the optional position argument that follows.
            let range = opt(parse_colon_range).parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Try to parse optional position
            let position = opt(parse_u32).parse_next(input)?;

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
            let position = opt(parse_u32).parse_next(input)?;
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
            let position = parse_u32.parse_next(input)?;
            Ok(Command::PlaylistDelete { name, position })
        }
        "playlistmove" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let from = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let to = parse_u32.parse_next(input)?;
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
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::SearchPlaylist { name, tag, value })
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
                let subsystem = parse_string.parse_next(input)?;
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
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::SearchAdd { tag, value })
        }
        "searchaddpl" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::SearchAddPl { name, tag, value })
        }
        "findadd" => {
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::FindAdd { tag, value })
        }
        "listfiles" => {
            let uri = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::ListFiles { uri })
        }
        // Stickers
        "sticker" => {
            let operation = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let _type_str = parse_string.parse_next(input)?; // "song" for now
            let _ = space0.parse_next(input)?;
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            match operation.as_str() {
                "get" => {
                    let name = parse_string.parse_next(input)?;
                    Ok(Command::StickerGet { uri, name })
                }
                "set" => {
                    let name = parse_string.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let value = parse_quoted_or_unquoted.parse_next(input)?;
                    Ok(Command::StickerSet { uri, name, value })
                }
                "delete" => {
                    let name = opt(parse_string).parse_next(input)?;
                    Ok(Command::StickerDelete { uri, name })
                }
                "list" => Ok(Command::StickerList { uri }),
                "find" => {
                    let name = parse_string.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    // MPD supports optional comparison: [eq|ne|lt|gt|lte|gte VALUE]
                    // Parse up to two more optional tokens (operator and value)
                    let first = opt(parse_quoted_or_unquoted).parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let second = opt(parse_quoted_or_unquoted).parse_next(input)?;
                    // If two tokens: first is operator, second is comparison value
                    // If one token: it is the sticker value filter
                    let value = match (first, second) {
                        (Some(op), Some(val)) => {
                            // operator + value form — store value, ignore op for now
                            let _ = op;
                            Some(val)
                        }
                        (Some(v), None) => Some(v),
                        (None, _) => None,
                    };
                    Ok(Command::StickerFind { uri, name, value })
                }
                "inc" => {
                    let name = parse_string.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let delta = opt(|input: &mut &str| {
                        parse_string
                            .parse_next(input)?
                            .parse::<i32>()
                            .map_err(|_| ErrMode::Cut(ContextError::default()))
                    })
                    .parse_next(input)?;
                    Ok(Command::StickerInc { uri, name, delta })
                }
                "dec" => {
                    let name = parse_string.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let delta = opt(|input: &mut &str| {
                        parse_string
                            .parse_next(input)?
                            .parse::<i32>()
                            .map_err(|_| ErrMode::Cut(ContextError::default()))
                    })
                    .parse_next(input)?;
                    Ok(Command::StickerDec { uri, name, delta })
                }
                _ => Ok(Command::Unknown(format!("sticker {operation}"))),
            }
        }
        "stickernames" => {
            let uri = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::StickerNames { uri })
        }
        "stickertypes" => Ok(Command::StickerTypes),
        "stickernamestypes" => {
            let uri = opt(parse_quoted_or_unquoted).parse_next(input)?;
            Ok(Command::StickerNamesTypes { uri })
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
            let priority = parse_u8.parse_next(input)?;
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
            let priority = parse_u8.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Parse first ID (required)
            let first_id = parse_u32.parse_next(input)?;
            let mut ids = vec![first_id];

            // Parse additional IDs (optional)
            loop {
                let _ = space0.parse_next(input)?;
                if input.is_empty() {
                    break;
                }
                match opt(parse_u32).parse_next(input)? {
                    Some(id) => ids.push(id),
                    None => break,
                }
            }

            Ok(Command::PrioId { priority, ids })
        }
        "rangeid" => {
            let id = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            // MPD format: start:end (floats with colon separator)
            // Both parts optional: ":" means clear range, "0.5:" means start only, etc.
            let rest = input.trim();
            let range = if let Some(colon_pos) = rest.find(':') {
                let start_str = &rest[..colon_pos];
                let end_str = &rest[colon_pos + 1..];
                let start: Option<f64> = if start_str.is_empty() {
                    None
                } else {
                    Some(start_str.parse().map_err(|_| {
                        winnow::error::ErrMode::Backtrack(winnow::error::ContextError::new())
                    })?)
                };
                let end: Option<f64> = if end_str.is_empty() {
                    None
                } else {
                    Some(end_str.parse().map_err(|_| {
                        winnow::error::ErrMode::Backtrack(winnow::error::ContextError::new())
                    })?)
                };
                *input = "";
                (start.unwrap_or(0.0), end.unwrap_or(0.0))
            } else {
                return Err(winnow::error::ErrMode::Backtrack(
                    winnow::error::ContextError::new(),
                ));
            };
            Ok(Command::RangeId { id, range })
        }
        "addtagid" => {
            let id = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::AddTagId { id, tag, value })
        }
        "cleartagid" => {
            let id = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let tag = opt(parse_string).parse_next(input)?;
            Ok(Command::ClearTagId { id, tag })
        }
        // Miscellaneous
        "config" => Ok(Command::Config),
        "kill" => Ok(Command::Kill),
        "mixrampdb" => {
            let decibels = parse_f64.parse_next(input)? as f32;
            Ok(Command::MixRampDb { decibels })
        }
        "mixrampdelay" => {
            let seconds = parse_f64.parse_next(input)? as f32;
            Ok(Command::MixRampDelay { seconds })
        }
        _ => Ok(Command::Unknown(cmd.to_string())),
    }
}

fn parse_u32(input: &mut &str) -> PResult<u32> {
    take_while(1.., |c: char| c.is_ascii_digit())
        .parse_next(input)?
        .parse()
        .map_err(|_| ErrMode::Cut(ContextError::default()))
}

fn parse_u32_or_quoted(input: &mut &str) -> PResult<u32> {
    let s = parse_quoted_or_unquoted.parse_next(input)?;
    if s.is_empty() {
        return Err(ErrMode::Backtrack(ContextError::default()));
    }
    s.parse().map_err(|_| ErrMode::Cut(ContextError::default()))
}

fn parse_u8(input: &mut &str) -> PResult<u8> {
    take_while(1.., |c: char| c.is_ascii_digit())
        .parse_next(input)?
        .parse()
        .map_err(|_| ErrMode::Cut(ContextError::default()))
}

fn parse_range(input: &mut &str) -> PResult<(u32, u32)> {
    // Parse MPD range syntax:
    //   "START:END"  — range [START, END)
    //   "START:"     — open-ended range [START, ...)
    //   "NUM"        — single position (equivalent to NUM:NUM+1)
    let start = parse_u32.parse_next(input)?;
    if input.starts_with(':') {
        let _ = winnow::token::one_of(':').parse_next(input)?;
        let end = opt(parse_u32).parse_next(input)?;
        Ok((start, end.unwrap_or(u32::MAX)))
    } else {
        Ok((start, start + 1))
    }
}

/// Parse a range that requires a colon (for commands where a bare number
/// is ambiguous with a following positional argument, e.g. `load`).
fn parse_colon_range(input: &mut &str) -> PResult<(u32, u32)> {
    let start = parse_u32.parse_next(input)?;
    let _ = winnow::token::one_of(':').parse_next(input)?;
    let end = opt(parse_u32).parse_next(input)?;
    Ok((start, end.unwrap_or(u32::MAX)))
}

fn parse_delete_target(input: &mut &str) -> PResult<DeleteTarget> {
    // Try to parse as range first (e.g., "5:10")
    let start = parse_u32.parse_next(input)?;

    // Check if there's a colon for range syntax
    if input.starts_with(':') {
        let _ = winnow::token::one_of(':').parse_next(input)?;
        let end = parse_u32.parse_next(input)?;
        Ok(DeleteTarget::Range(start, end))
    } else {
        Ok(DeleteTarget::Position(start))
    }
}

fn parse_move_from(input: &mut &str) -> PResult<MoveFrom> {
    // Try to parse as range first (e.g., "5:10")
    let start = parse_u32.parse_next(input)?;

    // Check if there's a colon for range syntax
    if input.starts_with(':') {
        let _ = winnow::token::one_of(':').parse_next(input)?;
        let end = parse_u32.parse_next(input)?;
        Ok(MoveFrom::Range(start, end))
    } else {
        Ok(MoveFrom::Position(start))
    }
}

fn parse_f64(input: &mut &str) -> PResult<f64> {
    take_while(1.., |c: char| {
        c.is_ascii_digit() || c == '.' || c == '-' || c == '+'
    })
    .parse_next(input)?
    .parse()
    .map_err(|_| ErrMode::Cut(ContextError::default()))
}

fn parse_f64_or_quoted(input: &mut &str) -> PResult<f64> {
    let s = parse_quoted_or_unquoted.parse_next(input)?;
    if s.is_empty() {
        return Err(ErrMode::Backtrack(ContextError::default()));
    }
    s.parse().map_err(|_| ErrMode::Cut(ContextError::default()))
}

fn parse_bool_or_quoted(input: &mut &str) -> PResult<bool> {
    let s = parse_quoted_or_unquoted.parse_next(input)?;
    match s.as_str() {
        "0" => Ok(false),
        "1" => Ok(true),
        "" => Err(ErrMode::Backtrack(ContextError::default())),
        _ => Err(ErrMode::Cut(ContextError::default())),
    }
}

fn parse_string(input: &mut &str) -> PResult<String> {
    take_till(1.., |c: char| c.is_whitespace() || c == '\n' || c == '\r')
        .map(|s: &str| s.to_string())
        .parse_next(input)
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
                    None => return Err(ErrMode::Cut(ContextError::default())),
                }
            }
            Some(c) => {
                consumed += c.len_utf8();
                result.push(c);
            }
            None => return Err(ErrMode::Cut(ContextError::default())),
        }
    }
    *input = &input[consumed..];
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
}
