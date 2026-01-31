//! Per-client connection state
//!
//! This module manages state that is specific to each client connection,
//! including tag type masks and protocol feature negotiation.

use std::collections::HashSet;

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

    /// Channels this client is subscribed to
    pub subscribed_channels: Vec<String>,

    /// Current partition for this connection (defaults to "default")
    pub current_partition: String,
}

impl ConnectionState {
    /// Create a new connection state with default settings
    ///
    /// By default, all tags and features are enabled, and the connection
    /// starts in the "default" partition
    pub fn new() -> Self {
        Self {
            enabled_tags: None, // All enabled
            enabled_features: None, // All enabled
            subscribed_channels: Vec::new(),
            current_partition: "default".to_string(),
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
        tags.insert("Composer".to_string());
        tags.insert("Performer".to_string());
        tags.insert("Comment".to_string());
        tags.insert("Disc".to_string());
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

    /// Get the default set of protocol features
    fn default_features() -> HashSet<String> {
        let mut features = HashSet::new();
        features.insert("binary".to_string());
        features.insert("command_list_ok".to_string());
        features.insert("idle".to_string());
        features.insert("ranges".to_string());
        features.insert("tags".to_string());
        features
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
        assert!(state.is_feature_enabled("binary"));
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
}
