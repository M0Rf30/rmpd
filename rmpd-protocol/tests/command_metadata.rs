use rmpd_protocol::parser::{Command, DeleteTarget, MoveFrom};

const PERMISSION_NONE: u8 = 0;
const PERMISSION_READ: u8 = 1;
const PERMISSION_ADD: u8 = 2;
const PERMISSION_CONTROL: u8 = 4;
const PERMISSION_ADMIN: u8 = 8;

fn s(v: &str) -> String {
    v.to_string()
}

fn check(cmd: &Command, expected_name: &str, expected_perm: u8) {
    assert_eq!(
        cmd.command_name(),
        expected_name,
        "command_name mismatch for {:?}",
        cmd
    );
    assert_eq!(
        cmd.command_required_permission(),
        expected_perm,
        "permission mismatch for {:?} (expected {}, got {})",
        cmd,
        expected_perm,
        cmd.command_required_permission()
    );
}

#[test]
fn playback_control_metadata() {
    check(
        &Command::Play { position: None },
        "play",
        PERMISSION_CONTROL,
    );
    check(&Command::PlayId { id: None }, "playid", PERMISSION_CONTROL);
    check(&Command::Pause { state: None }, "pause", PERMISSION_CONTROL);
    check(&Command::Stop, "stop", PERMISSION_CONTROL);
    check(&Command::Next, "next", PERMISSION_CONTROL);
    check(&Command::Previous, "previous", PERMISSION_CONTROL);
    check(
        &Command::Seek {
            position: 0,
            time: 0.0,
        },
        "seek",
        PERMISSION_CONTROL,
    );
    check(
        &Command::SeekId { id: 0, time: 0.0 },
        "seekid",
        PERMISSION_CONTROL,
    );
    check(
        &Command::SeekCur {
            time: 0.0,
            relative: false,
        },
        "seekcur",
        PERMISSION_CONTROL,
    );
}

#[test]
fn queue_management_metadata() {
    check(
        &Command::Add {
            uri: s(""),
            position: None,
        },
        "add",
        PERMISSION_ADD,
    );
    check(
        &Command::AddId {
            uri: s(""),
            position: None,
        },
        "addid",
        PERMISSION_ADD,
    );
    check(
        &Command::Delete {
            target: DeleteTarget::Position(0),
        },
        "delete",
        PERMISSION_CONTROL,
    );
    check(&Command::DeleteId { id: 0 }, "deleteid", PERMISSION_CONTROL);
    check(&Command::Clear, "clear", PERMISSION_CONTROL);
    check(
        &Command::Move {
            from: MoveFrom::Position(0),
            to: 0,
        },
        "move",
        PERMISSION_CONTROL,
    );
    check(
        &Command::MoveId { id: 0, to: 0 },
        "moveid",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Shuffle { range: None },
        "shuffle",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Swap { pos1: 0, pos2: 0 },
        "swap",
        PERMISSION_CONTROL,
    );
    check(
        &Command::SwapId { id1: 0, id2: 0 },
        "swapid",
        PERMISSION_CONTROL,
    );
}

#[test]
fn status_metadata() {
    check(&Command::Status, "status", PERMISSION_READ);
    check(&Command::CurrentSong, "currentsong", PERMISSION_READ);
    check(&Command::Stats, "stats", PERMISSION_READ);
    check(&Command::ClearError, "clearerror", PERMISSION_CONTROL);
}

#[test]
fn queue_inspection_metadata() {
    check(
        &Command::PlaylistInfo { range: None },
        "playlistinfo",
        PERMISSION_READ,
    );
    check(
        &Command::PlaylistId { id: None },
        "playlistid",
        PERMISSION_READ,
    );
    check(&Command::Playlist, "playlist", PERMISSION_READ);
    check(
        &Command::PlChanges {
            version: 0,
            range: None,
        },
        "plchanges",
        PERMISSION_READ,
    );
    check(
        &Command::PlChangesPosId {
            version: 0,
            range: None,
        },
        "plchangesposid",
        PERMISSION_READ,
    );
    check(
        &Command::PlaylistFind {
            tag: s(""),
            value: s(""),
        },
        "playlistfind",
        PERMISSION_READ,
    );
    check(
        &Command::PlaylistSearch {
            tag: s(""),
            value: s(""),
        },
        "playlistsearch",
        PERMISSION_READ,
    );
}

#[test]
fn volume_metadata() {
    check(
        &Command::SetVol { volume: 50 },
        "setvol",
        PERMISSION_CONTROL,
    );
    check(&Command::Volume { change: 0 }, "volume", PERMISSION_CONTROL);
    check(&Command::GetVol, "getvol", PERMISSION_READ);
}

#[test]
fn options_metadata() {
    check(
        &Command::Repeat { enabled: false },
        "repeat",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Random { enabled: false },
        "random",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Single { mode: s("0") },
        "single",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Consume { mode: s("0") },
        "consume",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Crossfade { seconds: 0 },
        "crossfade",
        PERMISSION_CONTROL,
    );
    check(
        &Command::ReplayGainMode { mode: s("off") },
        "replay_gain_mode",
        PERMISSION_CONTROL,
    );
    check(
        &Command::ReplayGainStatus,
        "replay_gain_status",
        PERMISSION_READ,
    );
}

#[test]
fn connection_metadata() {
    check(&Command::Close, "close", PERMISSION_NONE);
    check(&Command::Ping, "ping", PERMISSION_NONE);
    check(
        &Command::Password { password: s("") },
        "password",
        PERMISSION_NONE,
    );
    check(
        &Command::BinaryLimit { size: 0 },
        "binarylimit",
        PERMISSION_NONE,
    );
    check(
        &Command::Protocol { subcommand: None },
        "protocol",
        PERMISSION_NONE,
    );
}

#[test]
fn reflection_metadata() {
    check(&Command::Commands, "commands", PERMISSION_NONE);
    check(&Command::NotCommands, "notcommands", PERMISSION_NONE);
    check(
        &Command::TagTypes { subcommand: None },
        "tagtypes",
        PERMISSION_NONE,
    );
    check(&Command::UrlHandlers, "urlhandlers", PERMISSION_READ);
    check(&Command::Decoders, "decoders", PERMISSION_READ);
    check(
        &Command::StringNormalization,
        "stringnormalization",
        PERMISSION_NONE,
    );
}

#[test]
fn database_metadata() {
    check(
        &Command::Update { path: None },
        "update",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Rescan { path: None },
        "rescan",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Find {
            filters: vec![],
            sort: None,
            window: None,
        },
        "find",
        PERMISSION_READ,
    );
    check(
        &Command::Search {
            filters: vec![],
            sort: None,
            window: None,
        },
        "search",
        PERMISSION_READ,
    );
    check(
        &Command::List {
            tag: s(""),
            filter_tag: None,
            filter_value: None,
            group: None,
        },
        "list",
        PERMISSION_READ,
    );
    check(&Command::ListAll { path: None }, "listall", PERMISSION_READ);
    check(
        &Command::ListAllInfo { path: None },
        "listallinfo",
        PERMISSION_READ,
    );
    check(&Command::LsInfo { path: None }, "lsinfo", PERMISSION_READ);
    check(
        &Command::Count {
            filters: vec![],
            group: None,
        },
        "count",
        PERMISSION_READ,
    );
    check(
        &Command::SearchCount {
            tag: s(""),
            value: s(""),
            group: None,
        },
        "searchcount",
        PERMISSION_READ,
    );
    check(
        &Command::GetFingerprint { uri: s("") },
        "getfingerprint",
        PERMISSION_READ,
    );
    check(
        &Command::ReadComments { uri: s("") },
        "readcomments",
        PERMISSION_READ,
    );
}

#[test]
fn album_art_metadata() {
    check(
        &Command::AlbumArt {
            uri: s(""),
            offset: 0,
        },
        "albumart",
        PERMISSION_READ,
    );
    check(
        &Command::ReadPicture {
            uri: s(""),
            offset: 0,
        },
        "readpicture",
        PERMISSION_READ,
    );
}

#[test]
fn stored_playlists_metadata() {
    check(
        &Command::Save {
            name: s(""),
            mode: None,
        },
        "save",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Load {
            name: s(""),
            range: None,
            position: None,
        },
        "load",
        PERMISSION_ADD,
    );
    check(&Command::ListPlaylists, "listplaylists", PERMISSION_READ);
    check(
        &Command::ListPlaylist {
            name: s(""),
            range: None,
        },
        "listplaylist",
        PERMISSION_READ,
    );
    check(
        &Command::ListPlaylistInfo {
            name: s(""),
            range: None,
        },
        "listplaylistinfo",
        PERMISSION_READ,
    );
    check(
        &Command::PlaylistAdd {
            name: s(""),
            uri: s(""),
            position: None,
        },
        "playlistadd",
        PERMISSION_CONTROL,
    );
    check(
        &Command::PlaylistClear { name: s("") },
        "playlistclear",
        PERMISSION_CONTROL,
    );
    check(
        &Command::PlaylistDelete {
            name: s(""),
            position: 0,
        },
        "playlistdelete",
        PERMISSION_CONTROL,
    );
    check(
        &Command::PlaylistMove {
            name: s(""),
            from: 0,
            to: 0,
        },
        "playlistmove",
        PERMISSION_CONTROL,
    );
    check(&Command::Rm { name: s("") }, "rm", PERMISSION_CONTROL);
    check(
        &Command::Rename {
            from: s(""),
            to: s(""),
        },
        "rename",
        PERMISSION_CONTROL,
    );
    check(
        &Command::SearchPlaylist {
            name: s(""),
            tag: s(""),
            value: s(""),
        },
        "searchplaylist",
        PERMISSION_READ,
    );
    check(
        &Command::PlaylistLength { name: s("") },
        "playlistlength",
        PERMISSION_READ,
    );
}

#[test]
fn idle_metadata() {
    check(
        &Command::Idle { subsystems: vec![] },
        "idle",
        PERMISSION_NONE,
    );
    check(&Command::NoIdle, "noidle", PERMISSION_NONE);
}

#[test]
fn output_control_metadata() {
    check(&Command::Outputs, "outputs", PERMISSION_READ);
    check(
        &Command::EnableOutput { id: 0 },
        "enableoutput",
        PERMISSION_ADMIN,
    );
    check(
        &Command::DisableOutput { id: 0 },
        "disableoutput",
        PERMISSION_ADMIN,
    );
    check(
        &Command::ToggleOutput { id: 0 },
        "toggleoutput",
        PERMISSION_ADMIN,
    );
    check(
        &Command::OutputSet {
            id: 0,
            name: s(""),
            value: s(""),
        },
        "outputset",
        PERMISSION_ADMIN,
    );
}

#[test]
fn command_batching_metadata() {
    check(&Command::CommandListBegin, "command_list", PERMISSION_NONE);
    check(
        &Command::CommandListOkBegin,
        "command_list",
        PERMISSION_NONE,
    );
    check(&Command::CommandListEnd, "command_list", PERMISSION_NONE);
}

#[test]
fn advanced_database_metadata() {
    check(
        &Command::SearchAdd {
            tag: s(""),
            value: s(""),
        },
        "searchadd",
        PERMISSION_ADD,
    );
    check(
        &Command::SearchAddPl {
            name: s(""),
            tag: s(""),
            value: s(""),
        },
        "searchaddpl",
        PERMISSION_ADD,
    );
    check(
        &Command::FindAdd {
            tag: s(""),
            value: s(""),
        },
        "findadd",
        PERMISSION_ADD,
    );
    check(
        &Command::ListFiles { uri: None },
        "listfiles",
        PERMISSION_READ,
    );
}

#[test]
fn sticker_metadata() {
    check(
        &Command::StickerGet {
            uri: s(""),
            name: s(""),
        },
        "sticker",
        PERMISSION_CONTROL,
    );
    check(
        &Command::StickerSet {
            uri: s(""),
            name: s(""),
            value: s(""),
        },
        "sticker",
        PERMISSION_CONTROL,
    );
    check(
        &Command::StickerDelete {
            uri: s(""),
            name: None,
        },
        "sticker",
        PERMISSION_CONTROL,
    );
    check(
        &Command::StickerList { uri: s("") },
        "sticker",
        PERMISSION_CONTROL,
    );
    check(
        &Command::StickerFind {
            uri: s(""),
            name: s(""),
            value: None,
        },
        "sticker",
        PERMISSION_CONTROL,
    );
    check(
        &Command::StickerInc {
            uri: s(""),
            name: s(""),
            delta: None,
        },
        "sticker",
        PERMISSION_CONTROL,
    );
    check(
        &Command::StickerDec {
            uri: s(""),
            name: s(""),
            delta: None,
        },
        "sticker",
        PERMISSION_CONTROL,
    );
    check(
        &Command::StickerNames { uri: None },
        "stickernames",
        PERMISSION_READ,
    );
    check(&Command::StickerTypes, "stickertypes", PERMISSION_READ);
    check(
        &Command::StickerNamesTypes { uri: None },
        "stickernamestypes",
        PERMISSION_READ,
    );
}

#[test]
fn partition_metadata() {
    check(
        &Command::Partition { name: s("") },
        "partition",
        PERMISSION_CONTROL,
    );
    check(&Command::ListPartitions, "listpartitions", PERMISSION_READ);
    check(
        &Command::NewPartition { name: s("") },
        "newpartition",
        PERMISSION_ADMIN,
    );
    check(
        &Command::DelPartition { name: s("") },
        "delpartition",
        PERMISSION_ADMIN,
    );
    check(
        &Command::MoveOutput { name: s("") },
        "moveoutput",
        PERMISSION_ADMIN,
    );
}

#[test]
fn mount_metadata() {
    check(
        &Command::Mount {
            path: s(""),
            uri: s(""),
        },
        "mount",
        PERMISSION_ADMIN,
    );
    check(
        &Command::Unmount { path: s("") },
        "unmount",
        PERMISSION_ADMIN,
    );
    check(&Command::ListMounts, "listmounts", PERMISSION_READ);
    check(&Command::ListNeighbors, "listneighbors", PERMISSION_READ);
}

#[test]
fn messaging_metadata() {
    check(
        &Command::Subscribe { channel: s("") },
        "subscribe",
        PERMISSION_CONTROL,
    );
    check(
        &Command::Unsubscribe { channel: s("") },
        "unsubscribe",
        PERMISSION_CONTROL,
    );
    check(&Command::Channels, "channels", PERMISSION_READ);
    check(&Command::ReadMessages, "readmessages", PERMISSION_CONTROL);
    check(
        &Command::SendMessage {
            channel: s(""),
            message: s(""),
        },
        "sendmessage",
        PERMISSION_CONTROL,
    );
}

#[test]
fn advanced_queue_metadata() {
    check(
        &Command::Prio {
            priority: 0,
            ranges: vec![],
        },
        "prio",
        PERMISSION_CONTROL,
    );
    check(
        &Command::PrioId {
            priority: 0,
            ids: vec![],
        },
        "prioid",
        PERMISSION_CONTROL,
    );
    check(
        &Command::RangeId {
            id: 0,
            range: (0.0, 0.0),
        },
        "rangeid",
        PERMISSION_CONTROL,
    );
    check(
        &Command::AddTagId {
            id: 0,
            tag: s(""),
            value: s(""),
        },
        "addtagid",
        PERMISSION_CONTROL,
    );
    check(
        &Command::ClearTagId { id: 0, tag: None },
        "cleartagid",
        PERMISSION_CONTROL,
    );
}

#[test]
fn miscellaneous_metadata() {
    check(&Command::Config, "config", PERMISSION_ADMIN);
    check(&Command::Kill, "kill", PERMISSION_ADMIN);
    check(
        &Command::MixRampDb { decibels: 0.0 },
        "mixrampdb",
        PERMISSION_CONTROL,
    );
    check(
        &Command::MixRampDelay { seconds: 0.0 },
        "mixrampdelay",
        PERMISSION_CONTROL,
    );
}

#[test]
fn unknown_variants_metadata() {
    check(&Command::Unknown(s("foo")), "unknown", PERMISSION_NONE);
    check(
        &Command::UnknownSubcmd(s("tagtypes"), s("bad")),
        "unknown",
        PERMISSION_NONE,
    );
    check(
        &Command::ArgError(s("play"), s("bad arg"), s("abc")),
        "unknown",
        PERMISSION_NONE,
    );
}
