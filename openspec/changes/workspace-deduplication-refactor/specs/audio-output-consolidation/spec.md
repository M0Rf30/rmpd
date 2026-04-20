## ADDED Requirements

### Requirement: Shared sample format conversion module
The `rmpd-player` crate SHALL provide a `conversion.rs` module with public functions for PCM sample format conversion, replacing duplicated private functions across output backends.

#### Scenario: f32 to s16le byte conversion
- **WHEN** `samples_to_s16le(&[f32])` is called with interleaved f32 PCM samples in range -1.0..1.0
- **THEN** it SHALL return a `Vec<u8>` of little-endian signed 16-bit samples, clamping input values to -1.0..1.0

#### Scenario: f32 to i16 scalar conversion
- **WHEN** `f32_to_i16(val: f32)` is called
- **THEN** it SHALL return the clamped value scaled to `i16` range (multiply by 32767.0)

#### Scenario: f32 to i32 scalar conversion
- **WHEN** `f32_to_i32(val: f32)` is called
- **THEN** it SHALL return the clamped value scaled to `i32` range (multiply by 2147483647.0)

### Requirement: Default pause/resume behavior on AudioOutput
The `AudioOutput` trait SHALL provide default implementations for `pause()`, `resume()`, and `is_paused()` so that output backends do not need to re-implement identical pause state tracking.

#### Scenario: Backend uses default pause tracking
- **WHEN** a new output backend implements `AudioOutput` without overriding `pause()`, `resume()`, or `is_paused()`
- **THEN** calling `pause()` SHALL set paused state to true, `resume()` SHALL set it to false, and `is_paused()` SHALL return the current paused state

#### Scenario: Backend can override pause behavior
- **WHEN** an output backend needs custom pause logic (e.g., hardware-level pause)
- **THEN** it SHALL be able to override the default `pause()`, `resume()`, and `is_paused()` methods

### Requirement: Generic sample buffer for cpal callbacks
The `rmpd-player` crate SHALL provide a `SampleBuffer<T>` struct that encapsulates the repeated channel-based buffer refill pattern used in cpal output callbacks.

#### Scenario: Buffer refill from channel
- **WHEN** the cpal callback requests samples and the current buffer is exhausted
- **THEN** `SampleBuffer<T>` SHALL attempt to receive a new sample batch from its `SyncReceiver` channel and reset the read position

#### Scenario: Buffer underrun produces silence
- **WHEN** no new samples are available from the channel
- **THEN** `SampleBuffer<T>` SHALL return the default value for type `T` (0 for integer types, 0.0 for f32)

### Requirement: Output backends use shared conversion utilities
All existing output backends (`FifoOutput`, `PipeOutput`, `RecorderOutput`, `CpalOutput`, `DopOutput`) SHALL use the shared conversion functions from `conversion.rs` instead of private duplicates.

#### Scenario: FifoOutput uses shared samples_to_s16le
- **WHEN** `FifoOutput::write()` converts f32 samples to s16le bytes
- **THEN** it SHALL call `conversion::samples_to_s16le()` instead of its private implementation

#### Scenario: CpalOutput uses SampleBuffer
- **WHEN** `CpalOutput` builds a cpal stream callback
- **THEN** it SHALL use `SampleBuffer<T>` for the buffer management pattern instead of inline closure variables
