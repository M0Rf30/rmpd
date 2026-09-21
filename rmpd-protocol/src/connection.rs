//! Per-client connection state
//!
//! This module manages state that is specific to each client connection,
//! including tag type masks and protocol feature negotiation.

use rmpd_core::config::HostPermission;
use std::collections::HashSet;

/// Permission level constants matching MPD's permission system.
pub const PERMISSION_NONE: u8 = 0;
pub const PERMISSION_READ: u8 = 1;
pub const PERMISSION_ADD: u8 = 2;
pub const PERMISSION_CONTROL: u8 = 4;
pub const PERMISSION_ADMIN: u8 = 8;
pub const PERMISSION_PLAYER: u8 = 16;
pub const PERMISSION_ALL: u8 =
    PERMISSION_READ | PERMISSION_ADD | PERMISSION_CONTROL | PERMISSION_ADMIN | PERMISSION_PLAYER;

/// Map a single MPD permission-string ("read"|"add"|"control"|"admin"|
/// "player") to its bitflag. Unknown names return `None` — validating that
/// every configured permission string is one of these five is
/// `rmpd_core::config`'s job at load time (Contract); this is purely the
/// string -> bit lookup MPD's `Permission.cxx` `permission_map` performs.
pub fn permission_from_str(name: &str) -> Option<u8> {
    match name {
        "read" => Some(PERMISSION_READ),
        "add" => Some(PERMISSION_ADD),
        "control" => Some(PERMISSION_CONTROL),
        "admin" => Some(PERMISSION_ADMIN),
        "player" => Some(PERMISSION_PLAYER),
        _ => None,
    }
}

/// Fold a list of permission-name strings into a bitmask, applying the
/// `control` implies `player` rule (see `grant_permissions`). Names that
/// don't map to a known permission are silently skipped — they were already
/// rejected at config-load time. Used both for a `PasswordEntry`'s
/// permission set and for the plain `Vec<String>` permission lists
/// (`default_permissions`, `local_permissions`, `HostPermission::permissions`).
pub fn permission_bits_from_names<S: AsRef<str>>(names: &[S]) -> u8 {
    let mut bits = PERMISSION_NONE;
    for name in names {
        if let Some(bit) = permission_from_str(name.as_ref()) {
            bits |= bit;
        }
    }
    if bits & PERMISSION_CONTROL != 0 {
        bits |= PERMISSION_PLAYER;
    }
    bits
}

/// Resolve the pre-auth (no `password` command issued yet) permission bits
/// for a newly accepted connection, in MPD's precedence order:
///
/// 1. `local_permissions`, if the connection came in over the local Unix
///    domain socket (`is_local`).
/// 2. A `host_permissions` entry whose `host` exactly matches the peer's
///    address, for remote (TCP) connections. Only exact IP-literal matches
///    are supported — no CIDR/subnet matching, since that would need a new
///    dependency; see `HostPermission` in `rmpd_core::config`.
/// 3. `default_permissions`, if configured.
/// 4. `PERMISSION_ALL` when no password is configured at all (rmpd's
///    historical "wide open" default); otherwise `PERMISSION_NONE`, forcing
///    the client to authenticate via `password`.
pub fn resolve_initial_permissions(
    is_local: bool,
    peer_host: Option<&str>,
    local_permissions: Option<&[String]>,
    host_permissions: &[HostPermission],
    default_permissions: Option<&[String]>,
    any_password_configured: bool,
) -> u8 {
    if is_local && let Some(names) = local_permissions {
        return permission_bits_from_names(names);
    }
    if !is_local
        && let Some(host) = peer_host
        && let Some(entry) = host_permissions.iter().find(|h| h.host == host)
    {
        return permission_bits_from_names(&entry.permissions);
    }
    if let Some(names) = default_permissions {
        return permission_bits_from_names(names);
    }
    if any_password_configured {
        PERMISSION_NONE
    } else {
        PERMISSION_ALL
    }
}

/// Per-client connection state
///
/// Each client connection maintains its own state for:
/// - Tag type filtering (which metadata tags to include in responses)
/// - Protocol feature negotiation (which MPD protocol features are enabled)
/// - Subscribed message channels
/// - Current partition (for multi-partition support)
#[derive(Debug, Clone)]
pub struct ConnectionState {
    /// Set of enabled tag types for this connection
    /// None means all tags are enabled (default)
    /// Some(set) means only tags in the set are enabled
    pub enabled_tags: Option<HashSet<String>>,

    /// Set of enabled protocol features for this connection
    /// None means all features are enabled (default)
    /// Some(set) means only features in the set are enabled
    pub enabled_features: Option<HashSet<String>>,

    /// Set of enabled string-normalization options for this connection
    /// (e.g. `strip_diacritics`). None means all are enabled; MPD starts
    /// with none enabled, so `ConnectionState::new()` uses `Some(empty)`.
    pub enabled_normalizations: Option<HashSet<String>>,

    /// Channels this client is subscribed to
    pub subscribed_channels: Vec<String>,

    /// Current partition for this connection (defaults to "default")
    pub current_partition: String,

    /// MPD permissions bitmask for this connection
    pub permissions: u8,

    /// Whether this connection came in over the local Unix domain socket.
    /// Mirrors MPD's `Client::IsLocal()` (true only when peer credentials
    /// are available, i.e. AF_UNIX — never for TCP, including loopback).
    /// Gates `config` and the `file://` line of `urlhandlers`.
    pub is_local: bool,

    /// Maximum size in bytes of a binary payload chunk (`albumart`,
    /// `readpicture`), settable via `binarylimit`. Matches MPD's
    /// `Client::binary_limit` default of 8192.
    pub binary_limit: u32,
}

impl ConnectionState {
    /// Create a new connection state with default settings
    ///
    /// By default, all tags and features are enabled, and the connection
    /// starts in the "default" partition
    pub fn new() -> Self {
        Self {
            enabled_tags: None,                           // All tags enabled
            enabled_features: Some(HashSet::new()),       // No protocol features enabled by default
            enabled_normalizations: Some(HashSet::new()), // None enabled by default
            subscribed_channels: Vec::new(),
            current_partition: "default".to_string(),
            permissions: PERMISSION_ALL,
            is_local: false,
            binary_limit: 8192,
        }
    }

    /// Subscribe to a channel
    pub fn subscribe(&mut self, channel: String) {
        if !self.subscribed_channels.contains(&channel) {
            self.subscribed_channels.push(channel);
        }
    }

    /// Unsubscribe from a channel
    pub fn unsubscribe(&mut self, channel: &str) {
        self.subscribed_channels.retain(|c| c != channel);
    }

    /// Get list of subscribed channels
    pub fn subscribed_channels(&self) -> &[String] {
        &self.subscribed_channels
    }

    /// Check whether the connection holds at least `required` permission bits.
    pub fn has_permission(&self, required: u8) -> bool {
        required == PERMISSION_NONE || (self.permissions & required) != 0
    }

    /// Grant all permissions (called after successful password auth or when no password configured).
    pub fn grant_all_permissions(&mut self) {
        self.permissions = PERMISSION_ALL;
    }

    /// OR-in additional permission bits. For backwards compatibility with
    /// MPD 0.22 and older, `control` implies `player` (see Permission.cxx).
    pub fn grant_permissions(&mut self, perms: u8) {
        let mut perms = perms;
        if perms & PERMISSION_CONTROL != 0 {
            perms |= PERMISSION_PLAYER;
        }
        self.permissions |= perms;
    }

    /// Replace the connection's permission set outright, rather than
    /// OR-ing bits in. MPD's `password` command resets permissions to
    /// exactly the matched password entry's set (`Client::SetPermission`,
    /// not `AddPermission`); `control` still implies `player` for MPD 0.22
    /// compatibility.
    pub fn set_permissions(&mut self, perms: u8) {
        let mut perms = perms;
        if perms & PERMISSION_CONTROL != 0 {
            perms |= PERMISSION_PLAYER;
        }
        self.permissions = perms;
    }

    /// Check if a tag type is enabled for this connection
    pub fn is_tag_enabled(&self, tag: &str) -> bool {
        match &self.enabled_tags {
            None => true, // All tags enabled
            Some(tags) => tags.contains(tag),
        }
    }

    /// Check if a protocol feature is enabled for this connection
    pub fn is_feature_enabled(&self, feature: &str) -> bool {
        match &self.enabled_features {
            None => true, // All features enabled
            Some(features) => features.contains(feature),
        }
    }

    /// Enable all tag types
    pub fn enable_all_tags(&mut self) {
        self.enabled_tags = None;
    }

    /// Disable all tag types
    pub fn disable_all_tags(&mut self) {
        self.enabled_tags = Some(HashSet::new());
    }

    /// Enable specific tag types
    pub fn enable_tags(&mut self, tags: Vec<String>) {
        match &mut self.enabled_tags {
            None => {
                // Currently all enabled, need to create set with default tags + new tags
                let mut tag_set = Self::default_tags();
                tag_set.extend(tags);
                self.enabled_tags = Some(tag_set);
            }
            Some(tag_set) => {
                // Add to existing set
                tag_set.extend(tags);
            }
        }
    }

    /// Disable specific tag types
    pub fn disable_tags(&mut self, tags: Vec<String>) {
        match &mut self.enabled_tags {
            None => {
                // Currently all enabled, create set with all except specified
                let mut tag_set = Self::default_tags();
                for tag in tags {
                    tag_set.remove(&tag);
                }
                self.enabled_tags = Some(tag_set);
            }
            Some(tag_set) => {
                // Remove from existing set
                for tag in tags {
                    tag_set.remove(&tag);
                }
            }
        }
    }

    /// Reset specific tag types to default state
    pub fn reset_tags(&mut self, tags: Vec<String>) {
        // Reset means re-enable if they're in the default set
        match &mut self.enabled_tags {
            None => {
                // Already at default (all enabled)
            }
            Some(tag_set) => {
                let defaults = Self::default_tags();
                for tag in tags {
                    if defaults.contains(&tag) {
                        tag_set.insert(tag);
                    }
                }
            }
        }
    }

    /// Get the default set of tag types
    fn default_tags() -> HashSet<String> {
        // MPD default = All tags EXCEPT Comment (see Settings.cxx: All & ~TAG_COMMENT)
        let mut tags = HashSet::new();
        tags.insert("Artist".to_string());
        tags.insert("ArtistSort".to_string());
        tags.insert("Album".to_string());
        tags.insert("AlbumSort".to_string());
        tags.insert("AlbumArtist".to_string());
        tags.insert("AlbumArtistSort".to_string());
        tags.insert("Title".to_string());
        tags.insert("Track".to_string());
        tags.insert("Name".to_string());
        tags.insert("Genre".to_string());
        tags.insert("Date".to_string());
        tags.insert("OriginalDate".to_string());
        tags.insert("Composer".to_string());
        tags.insert("Performer".to_string());
        tags.insert("Grouping".to_string());
        // Comment is excluded by default (matches MPD's global_tag_mask)
        tags.insert("Disc".to_string());
        tags.insert("DiscSubtitle".to_string());
        tags.insert("Label".to_string());
        tags.insert("MUSICBRAINZ_ARTISTID".to_string());
        tags.insert("MUSICBRAINZ_ALBUMID".to_string());
        tags.insert("MUSICBRAINZ_ALBUMARTISTID".to_string());
        tags.insert("MUSICBRAINZ_TRACKID".to_string());
        tags.insert("MUSICBRAINZ_RELEASETRACKID".to_string());
        tags.insert("MUSICBRAINZ_WORKID".to_string());
        tags.insert("MUSICBRAINZ_RELEASEGROUPID".to_string());
        tags
    }

    /// Enable all protocol features
    pub fn enable_all_features(&mut self) {
        self.enabled_features = None;
    }
    /// Disable all protocol features
    pub fn disable_all_features(&mut self) {
        self.enabled_features = Some(HashSet::new());
    }

    /// Clear all protocol features (alias for disable_all)
    pub fn clear_features(&mut self) {
        self.enabled_features = Some(HashSet::new());
    }

    /// Set exactly these protocol features (replacing any existing set)
    pub fn set_features(&mut self, features: Vec<String>) {
        let mut feature_set = HashSet::new();
        feature_set.extend(features);
        self.enabled_features = Some(feature_set);
    }
    /// Enable specific protocol features
    pub fn enable_features(&mut self, features: Vec<String>) {
        match &mut self.enabled_features {
            None => {
                // Currently all enabled, create set with defaults + new features
                let mut feature_set = Self::default_features();
                feature_set.extend(features);
                self.enabled_features = Some(feature_set);
            }
            Some(feature_set) => {
                // Add to existing set
                feature_set.extend(features);
            }
        }
    }
    /// Disable specific protocol features
    pub fn disable_features(&mut self, features: Vec<String>) {
        match &mut self.enabled_features {
            None => {
                // Currently all enabled, create set with all except specified
                let mut feature_set = Self::default_features();
                for feature in features {
                    feature_set.remove(&feature);
                }
                self.enabled_features = Some(feature_set);
            }
            Some(feature_set) => {
                // Remove from existing set
                for feature in features {
                    feature_set.remove(&feature);
                }
            }
        }
    }

    /// Get the default set of protocol features (none enabled by default,
    /// matching MPD which starts with no protocol features active).
    fn default_features() -> HashSet<String> {
        HashSet::new()
    }

    /// Check if a string-normalization option is enabled for this connection
    pub fn is_normalization_enabled(&self, name: &str) -> bool {
        match &self.enabled_normalizations {
            None => true,
            Some(names) => names.contains(name),
        }
    }

    /// Disable all string-normalization options (`stringnormalization clear`)
    pub fn clear_normalizations(&mut self) {
        self.enabled_normalizations = Some(HashSet::new());
    }

    /// Set exactly these string-normalization options (`stringnormalization all`)
    pub fn set_normalizations(&mut self, names: Vec<String>) {
        self.enabled_normalizations = Some(names.into_iter().collect());
    }

    /// Enable specific string-normalization options
    pub fn enable_normalizations(&mut self, names: Vec<String>) {
        self.enabled_normalizations
            .get_or_insert_with(HashSet::new)
            .extend(names);
    }

    /// Disable specific string-normalization options
    pub fn disable_normalizations(&mut self, names: Vec<String>) {
        let set = self.enabled_normalizations.get_or_insert_with(HashSet::new);
        for name in names {
            set.remove(&name);
        }
    }
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_connection_state() {
        let state = ConnectionState::new();
        assert!(state.is_tag_enabled("Artist"));
        assert!(!state.is_feature_enabled("binary"));
    }

    #[test]
    fn test_disable_all_tags() {
        let mut state = ConnectionState::new();
        state.disable_all_tags();
        assert!(!state.is_tag_enabled("Artist"));
    }

    #[test]
    fn test_enable_specific_tags() {
        let mut state = ConnectionState::new();
        state.disable_all_tags();
        state.enable_tags(vec!["Artist".to_string(), "Title".to_string()]);
        assert!(state.is_tag_enabled("Artist"));
        assert!(state.is_tag_enabled("Title"));
        assert!(!state.is_tag_enabled("Album"));
    }

    #[test]
    fn test_disable_specific_tags() {
        let mut state = ConnectionState::new();
        state.disable_tags(vec!["Artist".to_string()]);
        assert!(!state.is_tag_enabled("Artist"));
        assert!(state.is_tag_enabled("Title"));
    }

    #[test]
    fn test_enable_all_features() {
        let mut state = ConnectionState::new();
        state.disable_all_features();
        assert!(!state.is_feature_enabled("binary"));
        state.enable_all_features();
        assert!(state.is_feature_enabled("binary"));
    }

    #[test]
    fn test_enable_specific_features() {
        let mut state = ConnectionState::new();
        state.disable_all_features();
        state.enable_features(vec!["binary".to_string()]);
        assert!(state.is_feature_enabled("binary"));
        assert!(!state.is_feature_enabled("idle"));
    }

    #[test]
    fn test_control_implies_player() {
        // MPD 0.22 backwards compatibility: granting `control` also grants
        // `player` (see Permission.cxx::parsePermissions).
        let mut state = ConnectionState::new();
        state.permissions = PERMISSION_NONE;
        state.grant_permissions(PERMISSION_CONTROL);
        assert!(state.has_permission(PERMISSION_CONTROL));
        assert!(state.has_permission(PERMISSION_PLAYER));
    }

    #[test]
    fn test_permission_all_includes_player() {
        let state = ConnectionState::new();
        assert!(state.has_permission(PERMISSION_PLAYER));
        assert_eq!(
            PERMISSION_ALL,
            PERMISSION_READ
                | PERMISSION_ADD
                | PERMISSION_CONTROL
                | PERMISSION_ADMIN
                | PERMISSION_PLAYER
        );
    }

    #[test]
    fn test_permission_from_str_mapping() {
        assert_eq!(permission_from_str("read"), Some(PERMISSION_READ));
        assert_eq!(permission_from_str("add"), Some(PERMISSION_ADD));
        assert_eq!(permission_from_str("control"), Some(PERMISSION_CONTROL));
        assert_eq!(permission_from_str("admin"), Some(PERMISSION_ADMIN));
        assert_eq!(permission_from_str("player"), Some(PERMISSION_PLAYER));
        assert_eq!(permission_from_str("bogus"), None);
    }

    #[test]
    fn test_permission_bits_from_names_control_implies_player() {
        let bits = permission_bits_from_names(&["read".to_string(), "control".to_string()]);
        assert_eq!(
            bits,
            PERMISSION_READ | PERMISSION_CONTROL | PERMISSION_PLAYER
        );
    }

    #[test]
    fn test_permission_bits_from_names_ignores_unknown() {
        let bits = permission_bits_from_names(&["read".to_string(), "bogus".to_string()]);
        assert_eq!(bits, PERMISSION_READ);
    }

    #[test]
    fn test_resolve_initial_permissions_precedence() {
        let host_perms = vec![HostPermission {
            host: "10.0.0.5".to_string(),
            permissions: vec!["read".to_string()],
        }];
        let local_perms = vec!["admin".to_string()];
        let default_perms = vec!["add".to_string()];

        // Local connection: local_permissions wins even though
        // host_permissions/default_permissions are also configured.
        assert_eq!(
            resolve_initial_permissions(
                true,
                None,
                Some(local_perms.as_slice()),
                &host_perms,
                Some(default_perms.as_slice()),
                true,
            ),
            PERMISSION_ADMIN
        );

        // Remote connection matching a host entry: host_permissions wins
        // over default_permissions.
        assert_eq!(
            resolve_initial_permissions(
                false,
                Some("10.0.0.5"),
                Some(local_perms.as_slice()),
                &host_perms,
                Some(default_perms.as_slice()),
                true,
            ),
            PERMISSION_READ
        );

        // Remote connection with no matching host entry: falls back to
        // default_permissions.
        assert_eq!(
            resolve_initial_permissions(
                false,
                Some("10.0.0.9"),
                Some(local_perms.as_slice()),
                &host_perms,
                Some(default_perms.as_slice()),
                true,
            ),
            PERMISSION_ADD
        );

        // Nothing configured at all and no password: historical wide-open
        // default.
        assert_eq!(
            resolve_initial_permissions(false, None, None, &[], None, false),
            PERMISSION_ALL
        );

        // A password is configured but no default_permissions: unauthenticated
        // clients get nothing until they `password` in.
        assert_eq!(
            resolve_initial_permissions(false, None, None, &[], None, true),
            PERMISSION_NONE
        );
    }
}
