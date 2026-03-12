//! MPD protocol reflection and introspection command handlers
//!
//! These commands allow clients to query the server's capabilities,
//! supported commands, tag types, decoders, and URL handlers.

use crate::connection::{
    ConnectionState, PERMISSION_ADD, PERMISSION_ADMIN, PERMISSION_CONTROL, PERMISSION_NONE,
    PERMISSION_READ,
};
use crate::response::ResponseBuilder;

const COMMAND_PERMISSIONS: &[(&str, u8)] = &[
    ("add", PERMISSION_ADD),
    ("addid", PERMISSION_ADD),
    ("addtagid", PERMISSION_CONTROL),
    ("albumart", PERMISSION_READ),
    ("binarylimit", PERMISSION_NONE),
    ("channels", PERMISSION_READ),
    ("clear", PERMISSION_CONTROL),
    ("clearerror", PERMISSION_CONTROL),
    ("cleartagid", PERMISSION_CONTROL),
    ("close", PERMISSION_NONE),
    ("commands", PERMISSION_NONE),
    ("config", PERMISSION_ADMIN),
    ("consume", PERMISSION_CONTROL),
    ("count", PERMISSION_READ),
    ("crossfade", PERMISSION_CONTROL),
    ("currentsong", PERMISSION_READ),
    ("decoders", PERMISSION_READ),
    ("delete", PERMISSION_CONTROL),
    ("deleteid", PERMISSION_CONTROL),
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
    ("listpartitions", PERMISSION_READ),
    ("listplaylist", PERMISSION_READ),
    ("listplaylistinfo", PERMISSION_READ),
    ("listplaylists", PERMISSION_READ),
    ("load", PERMISSION_ADD),
    ("lsinfo", PERMISSION_READ),
    ("mixrampdb", PERMISSION_CONTROL),
    ("mixrampdelay", PERMISSION_CONTROL),
    ("mount", PERMISSION_ADMIN),
    ("move", PERMISSION_CONTROL),
    ("moveid", PERMISSION_CONTROL),
    ("moveoutput", PERMISSION_ADMIN),
    ("newpartition", PERMISSION_ADMIN),
    ("next", PERMISSION_CONTROL),
    ("notcommands", PERMISSION_NONE),
    ("outputset", PERMISSION_ADMIN),
    ("outputs", PERMISSION_READ),
    ("partition", PERMISSION_CONTROL),
    ("password", PERMISSION_NONE),
    ("pause", PERMISSION_CONTROL),
    ("ping", PERMISSION_NONE),
    ("play", PERMISSION_CONTROL),
    ("playid", PERMISSION_CONTROL),
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
    ("previous", PERMISSION_CONTROL),
    ("prio", PERMISSION_CONTROL),
    ("prioid", PERMISSION_CONTROL),
    ("protocol", PERMISSION_NONE),
    ("random", PERMISSION_CONTROL),
    ("rangeid", PERMISSION_CONTROL),
    ("readcomments", PERMISSION_READ),
    ("readmessages", PERMISSION_CONTROL),
    ("readpicture", PERMISSION_READ),
    ("rename", PERMISSION_CONTROL),
    ("repeat", PERMISSION_CONTROL),
    ("replay_gain_mode", PERMISSION_CONTROL),
    ("replay_gain_status", PERMISSION_READ),
    ("rescan", PERMISSION_CONTROL),
    ("rm", PERMISSION_CONTROL),
    ("save", PERMISSION_CONTROL),
    ("search", PERMISSION_READ),
    ("searchadd", PERMISSION_ADD),
    ("searchaddpl", PERMISSION_ADD),
    ("searchcount", PERMISSION_READ),
    ("searchplaylist", PERMISSION_READ),
    ("seek", PERMISSION_CONTROL),
    ("seekcur", PERMISSION_CONTROL),
    ("seekid", PERMISSION_CONTROL),
    ("sendmessage", PERMISSION_CONTROL),
    ("setvol", PERMISSION_CONTROL),
    ("shuffle", PERMISSION_CONTROL),
    ("single", PERMISSION_CONTROL),
    ("stats", PERMISSION_READ),
    ("status", PERMISSION_READ),
    ("sticker", PERMISSION_CONTROL),
    ("stickernames", PERMISSION_READ),
    ("stickernamestypes", PERMISSION_READ),
    ("stickertypes", PERMISSION_READ),
    ("stop", PERMISSION_CONTROL),
    ("stringnormalization", PERMISSION_NONE),
    ("subscribe", PERMISSION_CONTROL),
    ("swap", PERMISSION_CONTROL),
    ("swapid", PERMISSION_CONTROL),
    ("tagtypes", PERMISSION_NONE),
    ("toggleoutput", PERMISSION_ADMIN),
    ("unmount", PERMISSION_ADMIN),
    ("unsubscribe", PERMISSION_CONTROL),
    ("update", PERMISSION_CONTROL),
    ("urlhandlers", PERMISSION_READ),
    ("volume", PERMISSION_CONTROL),
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

pub async fn handle_stringnormalization_command() -> String {
    // MPD reports the active Unicode normalization form for tag comparisons.
    // rmpd performs no normalization (same as MPD's default "None" mode).
    let mut resp = ResponseBuilder::new();
    resp.field("stringnormalization", "None");
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
