use winnow::prelude::*;
use winnow::token::{take_till, take_while};
use winnow::combinator::opt;
use winnow::ascii::space0;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    // Playback control
    Play { position: Option<u32> },
    PlayId { id: Option<u32> },
    Pause { state: Option<bool> },
    Stop,
    Next,
    Previous,
    Seek { position: u32, time: f64 },
    SeekId { id: u32, time: f64 },
    SeekCur { time: f64, relative: bool },

    // Queue management
    Add { uri: String },
    AddId { uri: String, position: Option<u32> },
    Delete { position: u32 },
    DeleteId { id: u32 },
    Clear,
    Move { from: u32, to: u32 },
    MoveId { id: u32, to: u32 },
    Shuffle { range: Option<(u32, u32)> },
    Swap { pos1: u32, pos2: u32 },
    SwapId { id1: u32, id2: u32 },

    // Status
    Status,
    CurrentSong,
    Stats,
    ClearError,

    // Queue inspection
    PlaylistInfo { range: Option<(u32, u32)> },
    PlaylistId { id: Option<u32> },
    Playlist,  // Deprecated, use PlaylistInfo
    PlChanges { version: u32 },
    PlChangesPosId { version: u32 },
    PlaylistFind { tag: String, value: String },
    PlaylistSearch { tag: String, value: String },

    // Volume
    SetVol { volume: u8 },
    Volume { change: i8 },
    GetVol,

    // Options
    Repeat { enabled: bool },
    Random { enabled: bool },
    Single { mode: String },
    Consume { mode: String },
    Crossfade { seconds: u32 },
    ReplayGainMode { mode: String },
    ReplayGainStatus,

    // Connection
    Close,
    Ping,
    Password { password: String },
    BinaryLimit { size: u32 },
    Protocol { min_version: Option<String>, max_version: Option<String> },

    // Reflection
    Commands,
    NotCommands,
    TagTypes,
    UrlHandlers,
    Decoders,

    // Database
    Update { path: Option<String> },
    Rescan { path: Option<String> },
    Find { tag: String, value: String },
    Search { tag: String, value: String },
    List { tag: String, filter_tag: Option<String>, filter_value: Option<String>, group: Option<String> },
    ListAll { path: Option<String> },
    ListAllInfo { path: Option<String> },
    LsInfo { path: Option<String> },
    Count { tag: String, value: String },
    SearchCount { tag: String, value: String, group: Option<String> },
    GetFingerprint { uri: String },
    ReadComments { uri: String },

    // Album art
    AlbumArt { uri: String, offset: usize },
    ReadPicture { uri: String, offset: usize },

    // Stored playlists
    Save { name: String },
    Load { name: String },
    ListPlaylists,
    ListPlaylist { name: String },
    ListPlaylistInfo { name: String },
    PlaylistAdd { name: String, uri: String },
    PlaylistClear { name: String },
    PlaylistDelete { name: String, position: u32 },
    PlaylistMove { name: String, from: u32, to: u32 },
    Rm { name: String },
    Rename { from: String, to: String },
    SearchPlaylist { name: String, tag: String, value: String },
    PlaylistLength { name: String },

    // Idle notifications
    Idle { subsystems: Vec<String> },
    NoIdle,

    // Output control
    Outputs,
    EnableOutput { id: u32 },
    DisableOutput { id: u32 },
    ToggleOutput { id: u32 },
    OutputSet { id: u32, name: String, value: String },

    // Command batching
    CommandListBegin,
    CommandListOkBegin,
    CommandListEnd,

    // Advanced database
    SearchAdd { tag: String, value: String },
    SearchAddPl { name: String, tag: String, value: String },
    FindAdd { tag: String, value: String },
    ListFiles { uri: Option<String> },

    // Sticker database
    StickerGet { uri: String, name: String },
    StickerSet { uri: String, name: String, value: String },
    StickerDelete { uri: String, name: Option<String> },
    StickerList { uri: String },
    StickerFind { uri: String, name: String, value: Option<String> },
    StickerInc { uri: String, name: String, delta: Option<i32> },
    StickerDec { uri: String, name: String, delta: Option<i32> },
    StickerNames { uri: Option<String> },
    StickerTypes,
    StickerNamesTypes { uri: Option<String> },

    // Partitions
    Partition { name: String },
    ListPartitions,
    NewPartition { name: String },
    DelPartition { name: String },
    MoveOutput { name: String },

    // Mounts
    Mount { path: String, uri: String },
    Unmount { path: String },
    ListMounts,
    ListNeighbors,

    // Client-to-client messaging
    Subscribe { channel: String },
    Unsubscribe { channel: String },
    Channels,
    ReadMessages,
    SendMessage { channel: String, message: String },

    // Advanced queue operations
    Prio { priority: u8, range: (u32, u32) },
    PrioId { priority: u8, id: u32 },
    RangeId { id: u32, range: (f64, f64) },
    AddTagId { id: u32, tag: String, value: String },
    ClearTagId { id: u32, tag: Option<String> },

    // Miscellaneous
    Config,
    Kill,
    MixRampDb { decibels: f32 },
    MixRampDelay { seconds: f32 },

    // Unknown/Invalid
    Unknown(String),
}

pub fn parse_command(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    command_parser.parse(input).map_err(|e| e.to_string())
}

fn command_parser(input: &mut &str) -> PResult<Command> {
    let cmd = take_while(1.., |c: char| c.is_ascii_alphabetic() || c == '_').parse_next(input)?;
    let _ = space0.parse_next(input)?;

    match cmd {
        "play" => {
            let pos = opt(parse_u32).parse_next(input)?;
            Ok(Command::Play { position: pos })
        }
        "playid" => {
            let id = opt(parse_u32).parse_next(input)?;
            Ok(Command::PlayId { id })
        }
        "pause" => {
            let state = opt(parse_bool).parse_next(input)?;
            Ok(Command::Pause { state })
        }
        "stop" => Ok(Command::Stop),
        "next" => Ok(Command::Next),
        "previous" => Ok(Command::Previous),
        "seek" => {
            let position = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let time = parse_f64.parse_next(input)?;
            Ok(Command::Seek { position, time })
        }
        "seekid" => {
            let id = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let time = parse_f64.parse_next(input)?;
            Ok(Command::SeekId { id, time })
        }
        "seekcur" => {
            let time_str = take_till(0.., |c: char| c.is_whitespace()).parse_next(input)?;
            let (time, relative) = if time_str.starts_with('+') || time_str.starts_with('-') {
                (time_str.parse().map_err(|_| winnow::error::ErrMode::Cut(
                    winnow::error::ContextError::default()
                ))?, true)
            } else {
                (time_str.parse().map_err(|_| winnow::error::ErrMode::Cut(
                    winnow::error::ContextError::default()
                ))?, false)
            };
            Ok(Command::SeekCur { time, relative })
        }
        "add" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Add { uri })
        }
        "addid" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let position = opt(parse_u32).parse_next(input)?;
            Ok(Command::AddId { uri, position })
        }
        "delete" => {
            let position = parse_u32.parse_next(input)?;
            Ok(Command::Delete { position })
        }
        "deleteid" => {
            let id = parse_u32.parse_next(input)?;
            Ok(Command::DeleteId { id })
        }
        "clear" => Ok(Command::Clear),
        "move" => {
            let from = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let to = parse_u32.parse_next(input)?;
            Ok(Command::Move { from, to })
        }
        "moveid" => {
            let id = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let to = parse_u32.parse_next(input)?;
            Ok(Command::MoveId { id, to })
        }
        "shuffle" => Ok(Command::Shuffle { range: None }),
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
        "playlistinfo" => Ok(Command::PlaylistInfo { range: None }),
        "playlistid" => {
            let id = opt(parse_u32).parse_next(input)?;
            Ok(Command::PlaylistId { id })
        }
        "playlist" => Ok(Command::Playlist),
        "plchanges" => {
            let version = parse_u32.parse_next(input)?;
            Ok(Command::PlChanges { version })
        }
        "plchangesposid" => {
            let version = parse_u32.parse_next(input)?;
            Ok(Command::PlChangesPosId { version })
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
            let volume = val_str.parse::<u8>()
                .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))?;
            Ok(Command::SetVol { volume })
        }
        "volume" => {
            let val_str = parse_quoted_or_unquoted.parse_next(input)?;
            let change = val_str.parse::<i8>()
                .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))?;
            Ok(Command::Volume { change })
        }
        "getvol" => Ok(Command::GetVol),
        "repeat" => {
            let val = parse_quoted_or_unquoted.parse_next(input)?;
            let enabled = match val.as_str() {
                "0" => false,
                "1" => true,
                _ => return Err(winnow::error::ErrMode::Cut(winnow::error::ContextError::default())),
            };
            Ok(Command::Repeat { enabled })
        }
        "random" => {
            let val = parse_quoted_or_unquoted.parse_next(input)?;
            let enabled = match val.as_str() {
                "0" => false,
                "1" => true,
                _ => return Err(winnow::error::ErrMode::Cut(winnow::error::ContextError::default())),
            };
            Ok(Command::Random { enabled })
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
            let seconds = parse_u32.parse_next(input)?;
            Ok(Command::Crossfade { seconds })
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
            let min = opt(parse_string).parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let max = opt(parse_string).parse_next(input)?;
            Ok(Command::Protocol { min_version: min, max_version: max })
        }
        "commands" => Ok(Command::Commands),
        "notcommands" => Ok(Command::NotCommands),
        "tagtypes" => Ok(Command::TagTypes),
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
            // Second parameter is optional for filter expressions
            let value = opt(parse_quoted_or_unquoted).parse_next(input)?.unwrap_or_default();
            Ok(Command::Find { tag, value })
        }
        "search" => {
            let tag = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            // Second parameter is optional for filter expressions
            let value = opt(parse_quoted_or_unquoted).parse_next(input)?.unwrap_or_default();
            Ok(Command::Search { tag, value })
        }
        "list" => {
            let tag = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;

            // Try to parse optional filter or group
            let saved_input = *input;
            let next_token = opt(parse_quoted_or_unquoted).parse_next(input)?
                .filter(|s| !s.is_empty());  // Filter out empty strings

            let (filter_tag, filter_value, group) = if next_token.as_deref() == Some("group") {
                // Format: list TAG group GROUPTYPE
                let _ = space0.parse_next(input)?;
                let group_type = opt(parse_quoted_or_unquoted).parse_next(input)?
                    .filter(|s| !s.is_empty());
                (None, None, group_type)
            } else if let Some(ft) = next_token {
                // Format: list TAG FILTER_TAG FILTER_VALUE [group GROUPTYPE]
                let _ = space0.parse_next(input)?;
                let fv = parse_quoted_or_unquoted.parse_next(input)?;
                let _ = space0.parse_next(input)?;

                // Check for optional "group" keyword
                let saved_input2 = *input;
                let group_keyword = opt(parse_quoted_or_unquoted).parse_next(input)?
                    .filter(|s| !s.is_empty());

                let group_type = if group_keyword.as_deref() == Some("group") {
                    let _ = space0.parse_next(input)?;
                    opt(parse_quoted_or_unquoted).parse_next(input)?
                        .filter(|s| !s.is_empty())
                } else {
                    *input = saved_input2;
                    None
                };

                (Some(ft), Some(fv), group_type)
            } else {
                // Format: list TAG
                *input = saved_input;
                (None, None, None)
            };

            Ok(Command::List { tag, filter_tag, filter_value, group })
        }
        "listall" => {
            let path = opt(parse_string).parse_next(input)?;
            Ok(Command::ListAll { path })
        }
        "listallinfo" => {
            let path = opt(parse_string).parse_next(input)?;
            Ok(Command::ListAllInfo { path })
        }
        "lsinfo" => {
            let path = opt(parse_string).parse_next(input)?;
            Ok(Command::LsInfo { path })
        }
        "count" => {
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Count { tag, value })
        }
        "searchcount" => {
            let tag = parse_string.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let value = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let group = opt(parse_string).parse_next(input)?;
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
            let offset = parse_usize.parse_next(input)?;
            Ok(Command::AlbumArt { uri, offset })
        }
        "readpicture" => {
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let offset = parse_usize.parse_next(input)?;
            Ok(Command::ReadPicture { uri, offset })
        }
        "save" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Save { name })
        }
        "load" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::Load { name })
        }
        "listplaylists" => Ok(Command::ListPlaylists),
        "listplaylist" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::ListPlaylist { name })
        }
        "listplaylistinfo" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::ListPlaylistInfo { name })
        }
        "playlistadd" => {
            let name = parse_quoted_or_unquoted.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let uri = parse_quoted_or_unquoted.parse_next(input)?;
            Ok(Command::PlaylistAdd { name, uri })
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
            Ok(Command::Rename { from: name1, to: name2 })
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
            let id = parse_u32.parse_next(input)?;
            Ok(Command::EnableOutput { id })
        }
        "disableoutput" => {
            let id = parse_u32.parse_next(input)?;
            Ok(Command::DisableOutput { id })
        }
        "toggleoutput" => {
            let id = parse_u32.parse_next(input)?;
            Ok(Command::ToggleOutput { id })
        }
        "outputset" => {
            let id = parse_u32.parse_next(input)?;
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
            let uri = opt(parse_string).parse_next(input)?;
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
                "list" => {
                    Ok(Command::StickerList { uri })
                }
                "find" => {
                    let name = parse_string.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let value = opt(parse_quoted_or_unquoted).parse_next(input)?;
                    Ok(Command::StickerFind { uri, name, value })
                }
                "inc" => {
                    let name = parse_string.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let delta = opt(|input: &mut &str| {
                        parse_string.parse_next(input)?
                            .parse::<i32>()
                            .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))
                    }).parse_next(input)?;
                    Ok(Command::StickerInc { uri, name, delta })
                }
                "dec" => {
                    let name = parse_string.parse_next(input)?;
                    let _ = space0.parse_next(input)?;
                    let delta = opt(|input: &mut &str| {
                        parse_string.parse_next(input)?
                            .parse::<i32>()
                            .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))
                    }).parse_next(input)?;
                    Ok(Command::StickerDec { uri, name, delta })
                }
                _ => Ok(Command::Unknown(format!("sticker {}", operation))),
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
            let start = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let end = parse_u32.parse_next(input)?;
            Ok(Command::Prio { priority, range: (start, end) })
        }
        "prioid" => {
            let priority = parse_u8.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let id = parse_u32.parse_next(input)?;
            Ok(Command::PrioId { priority, id })
        }
        "rangeid" => {
            let id = parse_u32.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let start = parse_f64.parse_next(input)?;
            let _ = space0.parse_next(input)?;
            let end = parse_f64.parse_next(input)?;
            Ok(Command::RangeId { id, range: (start, end) })
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
        .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))
}

fn parse_usize(input: &mut &str) -> PResult<usize> {
    take_while(1.., |c: char| c.is_ascii_digit())
        .parse_next(input)?
        .parse()
        .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))
}

fn parse_u8(input: &mut &str) -> PResult<u8> {
    take_while(1.., |c: char| c.is_ascii_digit())
        .parse_next(input)?
        .parse()
        .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))
}

fn parse_i8(input: &mut &str) -> PResult<i8> {
    take_while(1.., |c: char| c.is_ascii_digit() || c == '-' || c == '+')
        .parse_next(input)?
        .parse()
        .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))
}

fn parse_f64(input: &mut &str) -> PResult<f64> {
    take_while(1.., |c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
        .parse_next(input)?
        .parse()
        .map_err(|_| winnow::error::ErrMode::Cut(winnow::error::ContextError::default()))
}

fn parse_bool(input: &mut &str) -> PResult<bool> {
    let val = take_while(1.., |c: char| c.is_ascii_digit())
        .parse_next(input)?;
    match val {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(winnow::error::ErrMode::Cut(winnow::error::ContextError::default())),
    }
}

fn parse_string(input: &mut &str) -> PResult<String> {
    take_till(0.., |c: char| c.is_whitespace() || c == '\n' || c == '\r')
        .map(|s: &str| s.to_string())
        .parse_next(input)
}

fn parse_quoted_or_unquoted(input: &mut &str) -> PResult<String> {
    tracing::debug!("parse_quoted_or_unquoted input: {:?}", input);
    let result = if input.starts_with('"') {
        tracing::debug!("Using quoted string parser");
        parse_quoted_string.parse_next(input)
    } else {
        tracing::debug!("Using unquoted string parser");
        parse_string.parse_next(input)
    };
    tracing::debug!("parse_quoted_or_unquoted result: {:?}", result);
    result
}

fn parse_quoted_string(input: &mut &str) -> PResult<String> {
    let _ = '"'.parse_next(input)?;
    let content = take_till(0.., |c| c == '"').parse_next(input)?;
    let _ = '"'.parse_next(input)?;
    Ok(content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_play_command() {
        assert_eq!(parse_command("play").unwrap(), Command::Play { position: None });
        assert_eq!(parse_command("play 5").unwrap(), Command::Play { position: Some(5) });
    }

    #[test]
    fn test_pause_command() {
        assert_eq!(parse_command("pause").unwrap(), Command::Pause { state: None });
        assert_eq!(parse_command("pause 1").unwrap(), Command::Pause { state: Some(true) });
        assert_eq!(parse_command("pause 0").unwrap(), Command::Pause { state: Some(false) });
    }

    #[test]
    fn test_add_command() {
        assert_eq!(
            parse_command("add song.mp3").unwrap(),
            Command::Add { uri: "song.mp3".to_string() }
        );
    }

    #[test]
    fn test_add_command_with_quotes() {
        assert_eq!(
            parse_command(r#"add "/home/user/song with spaces.mp3""#).unwrap(),
            Command::Add { uri: "/home/user/song with spaces.mp3".to_string() }
        );
    }

    #[test]
    fn test_add_command_with_path() {
        assert_eq!(
            parse_command("add /home/user/song.mp3").unwrap(),
            Command::Add { uri: "/home/user/song.mp3".to_string() }
        );
    }

    #[test]
    fn test_status_command() {
        assert_eq!(parse_command("status").unwrap(), Command::Status);
    }
}
