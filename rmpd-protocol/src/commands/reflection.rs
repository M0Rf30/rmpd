//! MPD protocol reflection and introspection command handlers
//!
//! These commands allow clients to query the server's capabilities,
//! supported commands, tag types, decoders, and URL handlers.

use crate::response::ResponseBuilder;

pub async fn handle_commands_command() -> String {
    let mut resp = ResponseBuilder::new();

    // Playback control
    resp.field("command", "play");
    resp.field("command", "playid");
    resp.field("command", "pause");
    resp.field("command", "stop");
    resp.field("command", "next");
    resp.field("command", "previous");
    resp.field("command", "seek");
    resp.field("command", "seekid");
    resp.field("command", "seekcur");

    // Queue management
    resp.field("command", "add");
    resp.field("command", "addid");
    resp.field("command", "delete");
    resp.field("command", "deleteid");
    resp.field("command", "clear");
    resp.field("command", "move");
    resp.field("command", "moveid");
    resp.field("command", "swap");
    resp.field("command", "swapid");
    resp.field("command", "shuffle");
    resp.field("command", "playlistid");

    // Status & inspection
    resp.field("command", "status");
    resp.field("command", "currentsong");
    resp.field("command", "stats");
    resp.field("command", "playlistinfo");
    resp.field("command", "playlistid");

    // Volume
    resp.field("command", "setvol");
    resp.field("command", "volume");

    // Options
    resp.field("command", "repeat");
    resp.field("command", "random");
    resp.field("command", "single");
    resp.field("command", "consume");
    resp.field("command", "crossfade");

    // Connection
    resp.field("command", "close");
    resp.field("command", "ping");
    resp.field("command", "password");

    // Reflection
    resp.field("command", "commands");
    resp.field("command", "notcommands");
    resp.field("command", "tagtypes");
    resp.field("command", "urlhandlers");
    resp.field("command", "decoders");

    // Database
    resp.field("command", "update");
    resp.field("command", "rescan");
    resp.field("command", "find");
    resp.field("command", "search");
    resp.field("command", "list");
    resp.field("command", "listall");
    resp.field("command", "listallinfo");
    resp.field("command", "lsinfo");
    resp.field("command", "count");

    // Album art
    resp.field("command", "albumart");
    resp.field("command", "readpicture");

    // Stored playlists
    resp.field("command", "save");
    resp.field("command", "load");
    resp.field("command", "listplaylists");
    resp.field("command", "listplaylist");
    resp.field("command", "listplaylistinfo");
    resp.field("command", "playlistadd");
    resp.field("command", "playlistclear");
    resp.field("command", "playlistdelete");
    resp.field("command", "playlistmove");
    resp.field("command", "rm");
    resp.field("command", "rename");

    // Idle
    resp.field("command", "idle");
    resp.field("command", "noidle");

    // Outputs
    resp.field("command", "outputs");
    resp.field("command", "enableoutput");
    resp.field("command", "disableoutput");
    resp.field("command", "toggleoutput");
    resp.field("command", "outputset");

    // Command batching
    resp.field("command", "command_list_begin");
    resp.field("command", "command_list_ok_begin");
    resp.field("command", "command_list_end");

    resp.ok()
}

pub async fn handle_notcommands_command() -> String {
    // Return empty list - no password-protected commands yet
    ResponseBuilder::new().ok()
}

pub async fn handle_tagtypes_command(subcommand: Option<crate::parser::TagTypesSubcommand>) -> String {
    use crate::parser::TagTypesSubcommand;

    let mut resp = ResponseBuilder::new();

    match subcommand {
        None | Some(TagTypesSubcommand::Available) => {
            // List all supported metadata tags
            resp.field("tagtype", "Artist");
            resp.field("tagtype", "ArtistSort");
            resp.field("tagtype", "Album");
            resp.field("tagtype", "AlbumSort");
            resp.field("tagtype", "AlbumArtist");
            resp.field("tagtype", "AlbumArtistSort");
            resp.field("tagtype", "Title");
            resp.field("tagtype", "Track");
            resp.field("tagtype", "Name");
            resp.field("tagtype", "Genre");
            resp.field("tagtype", "Date");
            resp.field("tagtype", "Composer");
            resp.field("tagtype", "Performer");
            resp.field("tagtype", "Comment");
            resp.field("tagtype", "Disc");
        }
        Some(TagTypesSubcommand::All) => {
            // Enable all tag types for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK as all tags are enabled by default
        }
        Some(TagTypesSubcommand::Clear) => {
            // Disable all tag types for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK
        }
        Some(TagTypesSubcommand::Enable { tags: _ }) => {
            // Enable specific tags for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK as all tags are enabled by default
        }
        Some(TagTypesSubcommand::Disable { tags: _ }) => {
            // Disable specific tags for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK
        }
        Some(TagTypesSubcommand::Reset { tags: _ }) => {
            // Reset specific tags to default state for this client
            // TODO: Store per-client tag mask in connection state
            // For now, just return OK
        }
    }

    resp.ok()
}

pub async fn handle_protocol_command(subcommand: Option<crate::parser::ProtocolSubcommand>) -> String {
    use crate::parser::ProtocolSubcommand;

    let mut resp = ResponseBuilder::new();

    match subcommand {
        None | Some(ProtocolSubcommand::Available) => {
            // List all available protocol features
            // Based on MPD 0.24.x protocol features
            resp.field("feature", "binary"); // Binary responses
            resp.field("feature", "command_list_ok"); // Command lists with OK markers
            resp.field("feature", "idle"); // Idle notifications
            resp.field("feature", "ranges"); // Range syntax (START:END)
            resp.field("feature", "tags"); // Tag type negotiation
        }
        Some(ProtocolSubcommand::All) => {
            // Enable all protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK as all features are enabled by default
        }
        Some(ProtocolSubcommand::Clear) => {
            // Disable all protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK
        }
        Some(ProtocolSubcommand::Enable { features: _ }) => {
            // Enable specific protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK as all features are enabled by default
        }
        Some(ProtocolSubcommand::Disable { features: _ }) => {
            // Disable specific protocol features for this client
            // TODO: Store per-client protocol features in connection state
            // For now, just return OK
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
