//! MPD protocol reflection and introspection command handlers
//!
//! These commands allow clients to query the server's capabilities,
//! supported commands, tag types, decoders, and URL handlers.

use crate::connection::{
    ConnectionState, PERMISSION_ADD, PERMISSION_ADMIN, PERMISSION_CONTROL, PERMISSION_NONE,
    PERMISSION_PLAYER, PERMISSION_READ,
};
use crate::response::ResponseBuilder;

const COMMAND_PERMISSIONS: &[(&str, u8)] = &[
    ("add", PERMISSION_ADD),
    ("addid", PERMISSION_ADD),
    ("addtagid", PERMISSION_ADD),
    ("albumart", PERMISSION_READ),
    ("binarylimit", PERMISSION_NONE),
    ("channels", PERMISSION_READ),
    ("clear", PERMISSION_PLAYER),
    ("clearerror", PERMISSION_PLAYER),
    ("cleartagid", PERMISSION_ADD),
    ("close", PERMISSION_NONE),
    ("commands", PERMISSION_NONE),
    ("config", PERMISSION_ADMIN),
    ("consume", PERMISSION_PLAYER),
    ("count", PERMISSION_READ),
    ("crossfade", PERMISSION_PLAYER),
    ("currentsong", PERMISSION_READ),
    ("decoders", PERMISSION_READ),
    ("delete", PERMISSION_PLAYER),
    ("deleteid", PERMISSION_PLAYER),
    ("delpartition", PERMISSION_ADMIN),
    ("disableoutput", PERMISSION_ADMIN),
    ("enableoutput", PERMISSION_ADMIN),
    ("find", PERMISSION_READ),
    ("findadd", PERMISSION_ADD),
    ("getfingerprint", PERMISSION_READ),
    ("getvol", PERMISSION_READ),
    ("idle", PERMISSION_READ),
    ("kill", PERMISSION_ADMIN),
    ("list", PERMISSION_READ),
    ("listall", PERMISSION_READ),
    ("listallinfo", PERMISSION_READ),
    ("listfiles", PERMISSION_READ),
    ("listmounts", PERMISSION_READ),
    ("listneighbors", PERMISSION_READ),
    ("listpartitions", PERMISSION_READ),
    ("listplaylist", PERMISSION_READ),
    ("listplaylistinfo", PERMISSION_READ),
    ("listplaylists", PERMISSION_READ),
    ("load", PERMISSION_ADD),
    ("lsinfo", PERMISSION_READ),
    ("mixrampdb", PERMISSION_PLAYER),
    ("mixrampdelay", PERMISSION_PLAYER),
    ("mount", PERMISSION_ADMIN),
    ("move", PERMISSION_PLAYER),
    ("moveid", PERMISSION_PLAYER),
    ("moveoutput", PERMISSION_ADMIN),
    ("newpartition", PERMISSION_ADMIN),
    ("next", PERMISSION_PLAYER),
    ("notcommands", PERMISSION_NONE),
    ("outputset", PERMISSION_ADMIN),
    ("outputs", PERMISSION_READ),
    ("partition", PERMISSION_READ),
    ("password", PERMISSION_NONE),
    ("pause", PERMISSION_PLAYER),
    ("ping", PERMISSION_NONE),
    ("play", PERMISSION_PLAYER),
    ("playid", PERMISSION_PLAYER),
    ("playlist", PERMISSION_READ),
    ("playlistadd", PERMISSION_CONTROL),
    ("playlistclear", PERMISSION_CONTROL),
    ("playlistdelete", PERMISSION_CONTROL),
    ("playlistfind", PERMISSION_READ),
    ("playlistid", PERMISSION_READ),
    ("playlistinfo", PERMISSION_READ),
    ("playlistlength", PERMISSION_READ),
    ("playlistmove", PERMISSION_CONTROL),
    ("playlistsearch", PERMISSION_READ),
    ("plchanges", PERMISSION_READ),
    ("plchangesposid", PERMISSION_READ),
    ("previous", PERMISSION_PLAYER),
    ("prio", PERMISSION_PLAYER),
    ("prioid", PERMISSION_PLAYER),
    ("protocol", PERMISSION_NONE),
    ("random", PERMISSION_PLAYER),
    ("rangeid", PERMISSION_ADD),
    ("readcomments", PERMISSION_READ),
    ("readmessages", PERMISSION_READ),
    ("readpicture", PERMISSION_READ),
    ("rename", PERMISSION_CONTROL),
    ("repeat", PERMISSION_PLAYER),
    ("replay_gain_mode", PERMISSION_PLAYER),
    ("replay_gain_status", PERMISSION_READ),
    ("rescan", PERMISSION_CONTROL),
    ("rm", PERMISSION_CONTROL),
    ("save", PERMISSION_CONTROL),
    ("search", PERMISSION_READ),
    ("searchadd", PERMISSION_ADD),
    ("searchaddpl", PERMISSION_CONTROL),
    ("searchcount", PERMISSION_READ),
    ("searchplaylist", PERMISSION_READ),
    ("seek", PERMISSION_PLAYER),
    ("seekcur", PERMISSION_PLAYER),
    ("seekid", PERMISSION_PLAYER),
    ("sendmessage", PERMISSION_CONTROL),
    ("setvol", PERMISSION_PLAYER),
    ("shuffle", PERMISSION_PLAYER),
    ("single", PERMISSION_PLAYER),
    ("stats", PERMISSION_READ),
    ("status", PERMISSION_READ),
    ("sticker", PERMISSION_ADMIN),
    ("stickernames", PERMISSION_ADMIN),
    ("stickernamestypes", PERMISSION_ADMIN),
    ("stickertypes", PERMISSION_ADMIN),
    ("stop", PERMISSION_PLAYER),
    ("stringnormalization", PERMISSION_NONE),
    ("subscribe", PERMISSION_READ),
    ("swap", PERMISSION_PLAYER),
    ("swapid", PERMISSION_PLAYER),
    ("tagtypes", PERMISSION_NONE),
    ("toggleoutput", PERMISSION_ADMIN),
    ("unmount", PERMISSION_ADMIN),
    ("unsubscribe", PERMISSION_READ),
    ("update", PERMISSION_CONTROL),
    ("urlhandlers", PERMISSION_READ),
    ("volume", PERMISSION_PLAYER),
];

pub async fn handle_commands_command(conn_state: &ConnectionState) -> String {
    let mut resp = ResponseBuilder::new();
    for (cmd, perm) in COMMAND_PERMISSIONS {
        if conn_state.has_permission(*perm) {
            resp.field("command", *cmd);
        }
    }
    resp.ok()
}

pub async fn handle_notcommands_command(conn_state: &ConnectionState) -> String {
    let mut resp = ResponseBuilder::new();
    for (cmd, perm) in COMMAND_PERMISSIONS {
        if !conn_state.has_permission(*perm) {
            resp.field("command", *cmd);
        }
    }
    resp.ok()
}

pub async fn handle_tagtypes_command(
    conn_state: &mut ConnectionState,
    subcommand: Option<crate::parser::TagTypesSubcommand>,
) -> String {
    use crate::parser::TagTypesSubcommand;

    let mut resp = ResponseBuilder::new();

    // Every tag type the server supports. Order and membership must match
    // MPD's tag/Names.cxx table.
    const ALL_TAGS: &[&str] = &[
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
        "DiscSubtitle",
        "Label",
        "MUSICBRAINZ_ARTISTID",
        "MUSICBRAINZ_ALBUMID",
        "MUSICBRAINZ_ALBUMARTISTID",
        "MUSICBRAINZ_TRACKID",
        "MUSICBRAINZ_RELEASETRACKID",
        "MUSICBRAINZ_WORKID",
        "MUSICBRAINZ_RELEASEGROUPID",
    ];

    // MPD's `global_tag_mask` defaults to All tags EXCEPT Comment
    // (tag/Settings.cxx: `TagMask::All() & ~TagMask(TAG_COMMENT)`), and is
    // ANDed into every client's effective mask (TagPrint.cxx). Only the
    // `metadata_to_use` config directive (unimplemented here) changes it, so
    // Comment never appears in `tagtypes` or `tagtypes available` output —
    // even after `tagtypes enable Comment`/`all` — though it stays a valid
    // tag name for filters and comment-reading commands.
    fn is_globally_advertised(tag: &str) -> bool {
        tag != "Comment"
    }

    match subcommand {
        None => {
            // List only the tags currently enabled for this connection,
            // masked by the server-wide default set.
            for tag in ALL_TAGS {
                if is_globally_advertised(tag) && conn_state.is_tag_enabled(tag) {
                    resp.field("tagtype", *tag);
                }
            }
        }
        Some(TagTypesSubcommand::Available) => {
            // List every tag type the server supports, regardless of this
            // connection's currently-enabled set (but still masked by the
            // server-wide default set).
            for tag in ALL_TAGS {
                if is_globally_advertised(tag) {
                    resp.field("tagtype", *tag);
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

/// Known protocol features in MPD 0.24.x.
/// These are negotiable features that clients can enable/disable via the
/// `protocol` command.
const KNOWN_PROTOCOL_FEATURES: &[&str] = &["hide_playlists_in_root", "binary"];
pub async fn handle_protocol_command(
    conn_state: &mut ConnectionState,
    subcommand: Option<crate::parser::ProtocolSubcommand>,
) -> String {
    use crate::commands::utils::ACK_ERROR_ARG;
    use crate::parser::ProtocolSubcommand;
    let mut resp = ResponseBuilder::new();
    match subcommand {
        None => {
            // Bare `protocol` — list enabled features for this connection.
            // By default none are enabled, so this returns just OK.
            for feature in KNOWN_PROTOCOL_FEATURES {
                if conn_state.is_feature_enabled(feature) {
                    resp.field("feature", *feature);
                }
            }
        }
        Some(ProtocolSubcommand::Available) => {
            // List all known protocol features (regardless of enabled state)
            for feature in KNOWN_PROTOCOL_FEATURES {
                resp.field("feature", *feature);
            }
        }
        Some(ProtocolSubcommand::All) => {
            // Enable all known protocol features for this client
            conn_state.set_features(
                KNOWN_PROTOCOL_FEATURES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        Some(ProtocolSubcommand::Clear) => {
            // Disable all protocol features for this client
            conn_state.clear_features();
        }
        Some(ProtocolSubcommand::Enable { features }) => {
            // Validate each feature name before enabling
            for feature in &features {
                if !KNOWN_PROTOCOL_FEATURES.contains(&feature.as_str()) {
                    return ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "protocol",
                        "Unknown protocol feature",
                    );
                }
            }
            conn_state.enable_features(features);
        }
        Some(ProtocolSubcommand::Disable { features }) => {
            // Validate each feature name before disabling
            for feature in &features {
                if !KNOWN_PROTOCOL_FEATURES.contains(&feature.as_str()) {
                    return ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "protocol",
                        "Unknown protocol feature",
                    );
                }
            }
            conn_state.disable_features(features);
        }
    }
    resp.ok()
}

/// The only string-normalization option MPD (and rmpd) currently support.
const KNOWN_STRING_NORMALIZATIONS: &[&str] = &["strip_diacritics"];

pub async fn handle_stringnormalization_command(
    conn_state: &mut ConnectionState,
    subcommand: Option<crate::parser::StringNormalizationSubcommand>,
) -> String {
    use crate::commands::utils::ACK_ERROR_ARG;
    use crate::parser::StringNormalizationSubcommand;
    let mut resp = ResponseBuilder::new();
    match subcommand {
        None => {
            // Bare `stringnormalization` — list enabled options for this
            // connection. None are enabled by default.
            for option in KNOWN_STRING_NORMALIZATIONS {
                if conn_state.is_normalization_enabled(option) {
                    resp.field("stringnormalization", *option);
                }
            }
        }
        Some(StringNormalizationSubcommand::Available) => {
            for option in KNOWN_STRING_NORMALIZATIONS {
                resp.field("stringnormalization", *option);
            }
        }
        Some(StringNormalizationSubcommand::All) => {
            conn_state.set_normalizations(
                KNOWN_STRING_NORMALIZATIONS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        Some(StringNormalizationSubcommand::Clear) => {
            conn_state.clear_normalizations();
        }
        Some(StringNormalizationSubcommand::Enable { options }) => {
            for option in &options {
                if !KNOWN_STRING_NORMALIZATIONS.contains(&option.as_str()) {
                    return ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "stringnormalization",
                        "Unknown string normalization",
                    );
                }
            }
            conn_state.enable_normalizations(options);
        }
        Some(StringNormalizationSubcommand::Disable { options }) => {
            for option in &options {
                if !KNOWN_STRING_NORMALIZATIONS.contains(&option.as_str()) {
                    return ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "stringnormalization",
                        "Unknown string normalization",
                    );
                }
            }
            conn_state.disable_normalizations(options);
        }
    }
    resp.ok()
}

pub async fn handle_urlhandlers_command(conn_state: &ConnectionState) -> String {
    let mut resp = ResponseBuilder::new();

    // MPD only exposes the local `file://` handler to clients connected via
    // the local Unix socket (`Client::IsLocal`); remote clients never see it.
    if conn_state.is_local {
        resp.field("handler", "file://");
    }
    // rmpd streams audio directly over HTTP(S) via rmpd_stream::HttpSource,
    // regardless of client locality.
    resp.field("handler", "http://");
    resp.field("handler", "https://");

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
