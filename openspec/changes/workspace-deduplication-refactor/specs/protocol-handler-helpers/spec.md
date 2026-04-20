## ADDED Requirements

### Requirement: Playlist version update helper
The `rmpd-protocol` crate SHALL provide a `pub(crate) async fn update_playlist_version(state: &AppState)` helper that atomically increments `playlist_version` and updates `playlist_length`, replacing 13 inline occurrences in `commands/queue.rs`.

#### Scenario: Queue modification updates version
- **WHEN** `update_playlist_version(state)` is called after any queue modification
- **THEN** it SHALL acquire a write lock on `state.status`, increment `playlist_version` by 1, and set `playlist_length` to the current queue length

#### Scenario: Consistent behavior across all queue commands
- **WHEN** any queue-modifying command (add, addid, delete, move, shuffle, clear, etc.) completes successfully
- **THEN** it SHALL call `update_playlist_version()` instead of inlining the version update logic

### Requirement: URI scheme validation helper
The `rmpd-protocol` crate SHALL provide a `pub(crate) fn is_known_uri_scheme(scheme: &str) -> bool` function that validates whether a URI scheme is recognized for stream playback.

#### Scenario: Known streaming schemes
- **WHEN** `is_known_uri_scheme("http")` or `is_known_uri_scheme("https")` is called
- **THEN** it SHALL return `true`

#### Scenario: All supported schemes
- **WHEN** called with any of: "http", "https", "ftp", "rtsp", "rtp", "mms", "mmsh", "mmst", "mmsu", "gopher", "nfs", "smb", "smbclient", "file", "cdda", "alsa", "tidal", "qobuz"
- **THEN** it SHALL return `true`

#### Scenario: Unknown schemes
- **WHEN** `is_known_uri_scheme("banana")` is called
- **THEN** it SHALL return `false`

### Requirement: Stream song factory function
The `rmpd-protocol` crate SHALL provide a `pub(crate) fn create_stream_song(uri: &str) -> Song` function that constructs a `Song` struct for streaming URIs with all metadata fields set to `None`/empty defaults.

#### Scenario: Creating a stream song
- **WHEN** `create_stream_song("http://radio.example.com/stream")` is called
- **THEN** it SHALL return a `Song` with `path` set to the URI, `id` set to 0, and all optional metadata fields set to `None`

### Requirement: Player state transition helper
The `rmpd-protocol` crate SHALL provide a `pub(crate) async fn update_player_state(state: &AppState, new_state: PlayerState)` function that updates the status and emits the corresponding event.

#### Scenario: Transition to play state
- **WHEN** `update_player_state(state, PlayerState::Play)` is called
- **THEN** it SHALL write `PlayerState::Play` to `state.status`, drop the write lock, and emit `Event::PlayerStateChanged(PlayerState::Play)` on the event bus

#### Scenario: Transition to stop clears audio format
- **WHEN** `update_player_state(state, PlayerState::Stop)` is called
- **THEN** it SHALL set state to `Stop`, set `audio_format` to `None`, and emit the state change event

### Requirement: Audio format extraction helper
The `rmpd-protocol` crate SHALL provide a `pub(crate) fn extract_audio_format(song: &Song) -> Option<AudioFormat>` that constructs an `AudioFormat` from a song's sample_rate, channels, and bits_per_sample fields.

#### Scenario: Song with complete audio metadata
- **WHEN** `extract_audio_format(song)` is called on a song with all three audio fields populated
- **THEN** it SHALL return `Some(AudioFormat { sample_rate, channels, bits_per_sample })`

#### Scenario: Song with missing audio metadata
- **WHEN** `extract_audio_format(song)` is called on a song missing any of the three audio fields
- **THEN** it SHALL return `None`

### Requirement: Filter expression parsing helper
The `rmpd-protocol` crate SHALL provide a shared function for parsing filter expressions used by both `find` and `search` commands, eliminating the duplicated ~50-line blocks in `commands/database.rs`.

#### Scenario: Parenthesized filter expression
- **WHEN** the shared parser receives a filter string starting with "("
- **THEN** it SHALL parse it as a structured filter expression using `rmpd_core::filter`

#### Scenario: Key-value pair filters
- **WHEN** the shared parser receives tag-value pairs without parentheses
- **THEN** it SHALL construct the appropriate filter with exact match (find) or contains match (search) based on the `case_sensitive` parameter
