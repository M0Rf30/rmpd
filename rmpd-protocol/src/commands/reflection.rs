//! MPD protocol reflection and introspection command handlers
//!
//! These commands allow clients to query the server's capabilities,
//! supported commands, tag types, decoders, and URL handlers.

use crate::connection::ConnectionState;
use crate::response::ResponseBuilder;

pub async fn handle_commands_command() -> String {
    let mut resp = ResponseBuilder::new();
    // All implemented commands, sorted alphabetically (matching MPD's output).
    // Excludes internal commands: command_list_begin/end, noidle.
    let cmds = [
        "add",
        "addid",
        "addtagid",
        "albumart",
        "binarylimit",
        "channels",
        "clear",
        "clearerror",
        "cleartagid",
        "close",
        "commands",
        "config",
        "consume",
        "count",
        "crossfade",
        "currentsong",
        "decoders",
        "delete",
        "deleteid",
        "delpartition",
        "disableoutput",
        "enableoutput",
        "find",
        "findadd",
        "getfingerprint",
        "getvol",
        "idle",
        "kill",
        "list",
        "listall",
        "listallinfo",
        "listfiles",
        "listmounts",
        "listneighbors",
        "listpartitions",
        "listplaylist",
        "listplaylistinfo",
        "listplaylists",
        "load",
        "lsinfo",
        "mixrampdb",
        "mixrampdelay",
        "mount",
        "move",
        "moveid",
        "moveoutput",
        "newpartition",
        "next",
        "notcommands",
        "outputset",
        "outputs",
        "partition",
        "password",
        "pause",
        "ping",
        "play",
        "playid",
        "playlist",
        "playlistadd",
        "playlistclear",
        "playlistdelete",
        "playlistfind",
        "playlistid",
        "playlistinfo",
        "playlistlength",
        "playlistmove",
        "playlistsearch",
        "plchanges",
        "plchangesposid",
        "previous",
        "prio",
        "prioid",
        "protocol",
        "random",
        "rangeid",
        "readcomments",
        "readmessages",
        "readpicture",
        "rename",
        "repeat",
        "replay_gain_mode",
        "replay_gain_status",
        "rescan",
        "rm",
        "save",
        "search",
        "searchadd",
        "searchaddpl",
        "searchcount",
        "searchplaylist",
        "seek",
        "seekcur",
        "seekid",
        "sendmessage",
        "setvol",
        "shuffle",
        "single",
        "stats",
        "status",
        "sticker",
        "stickernames",
        "stickernamestypes",
        "stickertypes",
        "stop",
        "subscribe",
        "swap",
        "swapid",
        "tagtypes",
        "toggleoutput",
        "unmount",
        "unsubscribe",
        "update",
        "urlhandlers",
        "volume",
    ];

    for cmd in cmds {
        resp.field("command", cmd);
    }
    resp.ok()
}

pub async fn handle_notcommands_command() -> String {
    // Return empty list - no password-protected commands yet
    ResponseBuilder::new().ok()
}

pub async fn handle_tagtypes_command(
    conn_state: &mut ConnectionState,
    subcommand: Option<crate::parser::TagTypesSubcommand>,
) -> String {
    use crate::parser::TagTypesSubcommand;

    let mut resp = ResponseBuilder::new();

    match subcommand {
        None | Some(TagTypesSubcommand::Available) => {
            // List all currently enabled metadata tags for this connection.
            // Must match MPD's tagtypes output order.
            let all_tags = vec![
                "Artist",
                "ArtistSort",
                "Album",
                "AlbumSort",
                "AlbumArtist",
                "AlbumArtistSort",
                "Title",
                "TitleSort",
                "Track",
                "Name",
                "Genre",
                "Mood",
                "Date",
                "OriginalDate",
                "Composer",
                "ComposerSort",
                "Performer",
                "Conductor",
                "Work",
                "Movement",
                "MovementNumber",
                "ShowMovement",
                "Ensemble",
                "Location",
                "Grouping",
                "Comment",
                "Disc",
                "Label",
                "MUSICBRAINZ_ARTISTID",
                "MUSICBRAINZ_ALBUMID",
                "MUSICBRAINZ_ALBUMARTISTID",
                "MUSICBRAINZ_TRACKID",
                "MUSICBRAINZ_RELEASETRACKID",
                "MUSICBRAINZ_WORKID",
                "MUSICBRAINZ_RELEASEGROUPID",
            ];

            for tag in all_tags {
                if conn_state.is_tag_enabled(tag) {
                    resp.field("tagtype", tag);
                }
            }
        }
        Some(TagTypesSubcommand::All) => {
            // Enable all tag types for this client
            conn_state.enable_all_tags();
        }
        Some(TagTypesSubcommand::Clear) => {
            // Disable all tag types for this client
            conn_state.disable_all_tags();
        }
        Some(TagTypesSubcommand::Enable { tags }) => {
            // Enable specific tags for this client
            conn_state.enable_tags(tags);
        }
        Some(TagTypesSubcommand::Disable { tags }) => {
            // Disable specific tags for this client
            conn_state.disable_tags(tags);
        }
        Some(TagTypesSubcommand::Reset { tags }) => {
            // Reset specific tags to default state for this client
            conn_state.reset_tags(tags);
        }
    }

    resp.ok()
}

pub async fn handle_protocol_command(
    conn_state: &mut ConnectionState,
    subcommand: Option<crate::parser::ProtocolSubcommand>,
) -> String {
    use crate::parser::ProtocolSubcommand;

    let mut resp = ResponseBuilder::new();

    match subcommand {
        None | Some(ProtocolSubcommand::Available) => {
            // List all currently enabled protocol features for this connection
            // Based on MPD 0.24.x protocol features
            let all_features = vec![
                "binary",          // Binary responses
                "command_list_ok", // Command lists with OK markers
                "idle",            // Idle notifications
                "ranges",          // Range syntax (START:END)
                "tags",            // Tag type negotiation
            ];

            for feature in all_features {
                if conn_state.is_feature_enabled(feature) {
                    resp.field("feature", feature);
                }
            }
        }
        Some(ProtocolSubcommand::All) => {
            // Enable all protocol features for this client
            conn_state.enable_all_features();
        }
        Some(ProtocolSubcommand::Clear) => {
            // Disable all protocol features for this client
            conn_state.disable_all_features();
        }
        Some(ProtocolSubcommand::Enable { features }) => {
            // Enable specific protocol features for this client
            conn_state.enable_features(features);
        }
        Some(ProtocolSubcommand::Disable { features }) => {
            // Disable specific protocol features for this client
            conn_state.disable_features(features);
        }
    }

    resp.ok()
}

pub async fn handle_urlhandlers_command() -> String {
    let mut resp = ResponseBuilder::new();

    // Supported URL schemes
    resp.field("handler", "file://");
    // Future: http://, https://, etc.

    resp.ok()
}

pub async fn handle_decoders_command() -> String {
    let mut resp = ResponseBuilder::new();

    // All decoders provided by Symphonia
    // Note: Unlike outputs, decoders are NOT separate entities - no blank lines between them
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
