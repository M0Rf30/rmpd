// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::audio_output::AudioOutput;
use crate::decoder::SymphoniaDecoder;
use crate::dop::DopEncoder;
use crate::dop_output::DopOutput;
use crate::mixer::{MixerError, MixerSet};
use crate::output::CpalOutput;
use crate::output_control::OutputControl;
use parking_lot::Mutex;
use rmpd_core::config::{DopMode, OutputConfig, ReplayGainMode, ResamplerQuality};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::event::{Event, EventBus};
use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

const BUFFER_SIZE: usize = 4096;

/// Songs shorter than this never cross-fade, regardless of the configured
/// crossfade duration. Mirrors mpd `CrossFadeSettings::MIN_TOTAL_TIME`
/// (`src/player/CrossFade.hxx`).
const CROSSFADE_MIN_TOTAL_SECS: u64 = 20;

/// Valid DSD-to-PCM decode rates, ascending. DSD decimates cleanly only by an
/// integer power of two, so every target is 44.1 kHz-family.
const DSD_PCM_RATES: [u32; 4] = [44100, 88200, 176400, 352800];

/// Choose the DSD-to-PCM decode rate for a device running at `device_rate`.
///
/// Returns the SMALLEST DSD-family rate that both covers `device_rate` and is
/// reported as supported, falling back to the largest supported family rate and
/// finally to 88.2 kHz.
///
/// Decoding to the highest rate a device merely *advertises* is harmful:
/// systems like PipeWire advertise enormous ranges (up to ~768 kHz) but
/// resample internally, so an over-high PCM rate (a) gives a punishingly short
/// real-time callback period that underruns on scheduling jitter (audible
/// crackle), and (b) leaves DSD's ultrasonic shaped noise in the PCM, muddying
/// the sound. A moderate rate lets the decimation filter remove that noise and
/// keeps the buffer period comfortable.
fn select_dsd_pcm_rate(device_rate: u32, supports_rate: impl Fn(u32) -> bool) -> u32 {
    DSD_PCM_RATES
        .iter()
        .copied()
        .find(|&r| r >= device_rate && supports_rate(r))
        .or_else(|| {
            DSD_PCM_RATES
                .iter()
                .rev()
                .copied()
                .find(|&r| supports_rate(r))
        })
        .unwrap_or(88200)
}

/// The device rate the cpal output should open at for a DSD-to-PCM stream
/// decoded at `decode_rate` on a device whose native rate is `device_rate`.
///
/// Returns `None` (open the stream at `decode_rate`, no resampling) when the
/// rates already match, or when `native_decode_ok` — the resolved output is an
/// explicitly-configured device that natively supports `decode_rate`, so it is
/// safe to play bit-perfect. Otherwise returns `Some(device_rate)` so rmpd
/// resamples to the device's native rate itself, rather than letting a sound
/// server (e.g. PipeWire) resample a rate it merely advertises — which underruns
/// and leaves DSD ultrasonic noise in-band.
fn dsd_output_target_rate(
    decode_rate: u32,
    device_rate: u32,
    native_decode_ok: bool,
) -> Option<u32> {
    if native_decode_ok || decode_rate == device_rate {
        None
    } else {
        Some(device_rate)
    }
}

/// Commands that can be sent to the playback thread
enum PlaybackCommand {
    /// Seek the decoder to `position` seconds. `reply` (when present)
    /// receives the decoder's outcome so the protocol layer can report a
    /// failed seek to the client (MPD 0.25 "show detailed seek errors",
    /// `PlayerControl::SeekLocked` rethrowing the player error) instead of
    /// answering OK before the decode thread has even looked at it.
    Seek {
        position: f64,
        reply: Option<tokio::sync::oneshot::Sender<Result<()>>>,
    },
    /// No-op wake-up: unblocks a decode thread parked in a blocking
    /// `command_rx.recv()` while paused, so a resume (which otherwise only
    /// touches shared atomics) is noticed immediately instead of on the
    /// next incidental command.
    Wake,
}

/// How long [`PlaybackEngine::seek`] waits for the decode thread to report the
/// outcome of a seek before assuming it went through (a decode thread parked
/// in a slow network read must not wedge the `seek` command forever).
const SEEK_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A seek the decode thread has been asked to carry out, not yet answered.
/// See [`PlaybackEngine::begin_seek`].
#[derive(Debug)]
pub struct SeekPending {
    reply: tokio::sync::oneshot::Receiver<Result<()>>,
    last_failure: Arc<Mutex<Option<String>>>,
}

impl SeekPending {
    /// Wait for the decode thread's verdict, so a failed seek ("Not
    /// seekable", a decode error, ...) reaches the client as an ACK, like
    /// MPD's `PlayerControl::SeekLocked`.
    pub async fn verdict(self) -> Result<()> {
        match tokio::time::timeout(SEEK_REPLY_TIMEOUT, self.reply).await {
            Ok(Ok(result)) => result,
            // The decode thread went away without answering.
            Ok(Err(_)) => Err(seek_unavailable(&self.last_failure)),
            // Decode thread busy (e.g. a stalled network read): assume the
            // seek will go through, as before replies existed.
            Err(_) => Ok(()),
        }
    }
}

/// Why a seek could not be delivered: the failure that killed the decode
/// thread if there was one (`Failed to decode "x": ...`), otherwise the
/// player simply is not playing.
fn seek_unavailable(last_failure: &Mutex<Option<String>>) -> RmpdError {
    match last_failure.lock().clone() {
        Some(message) => RmpdError::Player(message),
        None => RmpdError::InvalidState("Not playing".to_owned()),
    }
}

/// Why the decode thread gave up on a song, classified like MPD's
/// `PlayerError` (`DECODER` vs `OUTPUT`, `src/player/Control.hxx`): the
/// protocol layer reacts differently (an output failure stops playback, a
/// decoder failure skips to the next song).
#[derive(Debug)]
enum PlaybackFailure {
    /// The song could not be opened/probed/decoded.
    Decoder(RmpdError),
    /// The audio output could not be opened, or died mid-stream.
    Output(RmpdError),
}

impl From<RmpdError> for PlaybackFailure {
    /// Plain `?` on a decoder operation classifies as a decoder failure;
    /// output call sites opt in with `map_err(PlaybackFailure::Output)`.
    fn from(e: RmpdError) -> Self {
        Self::Decoder(e)
    }
}

impl PlaybackFailure {
    /// The client-visible `error:` text and whether it is an output error.
    ///
    /// Decoder failures are formatted like MPD's decoder thread
    /// (`src/decoder/Thread.cxx`): `Failed to decode "<uri>": <cause>`, with
    /// any `user:password@` stripped from the URI (`uri_remove_auth`).
    fn message(&self, uri: &str) -> (String, bool) {
        match self {
            Self::Decoder(e) => (
                format!(
                    "Failed to decode {:?}: {}",
                    strip_uri_auth(uri),
                    error_text(e)
                ),
                false,
            ),
            Self::Output(e) => (error_text(e), true),
        }
    }
}

/// The bare message of an [`RmpdError`], without the `"Player error: "`-style
/// category prefix its `Display` adds (MPD error text carries no such prefix).
fn error_text(e: &RmpdError) -> String {
    match e {
        RmpdError::Player(msg) => msg.clone(),
        other => other.to_string(),
    }
}

/// Remove the `user:password@` part of a URL so credentials never reach a
/// client-visible error message (MPD `uri_remove_auth`).
fn strip_uri_auth(uri: &str) -> String {
    if let Some(scheme_end) = uri.find("://") {
        let rest = &uri[scheme_end + 3..];
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        if let Some(at) = rest[..authority_end].rfind('@') {
            return format!("{}{}", &uri[..scheme_end + 3], &rest[at + 1..]);
        }
    }
    uri.to_owned()
}

/// Main playback engine
pub struct PlaybackEngine {
    status: Arc<RwLock<rmpd_core::state::PlayerStatus>>,
    event_bus: EventBus,
    stop_flag: Arc<AtomicBool>,
    atomic_state: Arc<AtomicU8>, // For lock-free state checking in playback thread
    playback_thread: Option<thread::JoinHandle<()>>,
    current_song: Arc<Mutex<Option<Song>>>,
    volume: Arc<AtomicU8>,
    /// Mixers of the enabled outputs (`mixer_type` per output). `volume` above
    /// is only the SOFTWARE gain: it stays at 100 unless a software mixer is
    /// active, so a hardware mixer never stacks with digital attenuation.
    mixers: Arc<MixerSet>,
    /// Last volume requested through `set_volume`/`restore_volume`; applied as
    /// software gain whenever a software mixer is (again) active.
    last_volume: u8,
    command_tx: Option<mpsc::Sender<PlaybackCommand>>,
    outputs: Vec<OutputConfig>,
    replay_gain_mode: ReplayGainMode,
    replay_gain_preamp: f32,
    replay_gain_missing_preamp: f32,
    volume_normalization: bool,
    /// Random (shuffle) mode, mirrored from `PlayerStatus::random`.
    ///
    /// `Arc<AtomicBool>` (not a plain `bool`) because the decode thread reads
    /// this alongside its own copied replay-gain settings to recompute
    /// per-song gain on every track transition; a plain copy would go stale
    /// if `random` is toggled mid-playback (see MPD `ReplayGainMode::AUTO`).
    random: Arc<AtomicBool>,
    resampler_quality: ResamplerQuality,
    dop_mode: DopMode,
    output_slot: Arc<crate::output_slot::OutputSlot>,
    /// Crossfade duration in seconds (0 = disabled / DORMANT).
    crossfade: u32,
    /// MixRamp threshold in dBFS (0.0 = disabled; use time-based crossfade).
    mixramp_db: f32,
    /// Extra delay applied after the MixRamp overlap window (seconds).
    mixramp_delay: f32,
    /// Pre-fetched next song for gapless / crossfade transitions.
    ///
    /// The protocol layer sets this while the current song is playing; the
    /// decode thread claims it atomically with `take()` at the transition
    /// point.  When nothing is fed the slot stays `None` and the engine
    /// behaves exactly as it did before this field was added.
    next_song: Arc<Mutex<Option<rmpd_core::playback::PlaybackSong>>>,
    /// Output buffer time in milliseconds (0 uses a safe default).
    /// Sizes the PCM output's internal ring buffer / sync-channel depth.
    buffer_time_ms: u32,
    /// Lock-free pause/flush-generation/gain/played-frames control block,
    /// shared with every output backend's real-time callback. Owned by the
    /// engine for its ENTIRE lifetime (not per-song) so that `pause()` /
    /// `seek()` / `set_volume()` act on the same block a reused (gapless
    /// next/previous) cached output's callback is already reading.
    control: Arc<OutputControl>,
    /// Per-output control blocks (own software gain each); see
    /// [`crate::output_control::OutputGains`].
    output_gains: Arc<crate::output_control::OutputGains>,
    /// Audible-position base (seconds, f64 bits) for [`Self::get_elapsed_live`]:
    /// the decode thread updates this on every seek and at every natural
    /// song-boundary reset, in lockstep with `control`'s played-frame
    /// counter. Lock-free so a `status`/MPRIS query never blocks on (or is
    /// staled by) the decode thread.
    live_position_base_bits: Arc<AtomicU64>,
    /// Sample rate of the currently active decode-thread's format, or 0
    /// when nothing is loaded/playing. Constant for a given decode thread's
    /// lifetime (gapless/crossfade only advance to same-rate songs).
    live_sample_rate: Arc<AtomicU32>,
    /// Total duration of audio handed to the outputs since the engine was
    /// created, in nanoseconds (MPD `PlayerControl::total_play_time`, reported
    /// as `stats` `playtime`). Counted from the decoded samples actually
    /// written — not song durations — so pauses, seeks and aborted songs
    /// only contribute what really played. Never reset or persisted.
    play_time_ns: Arc<AtomicU64>,
    /// Message of the failure that ended the most recent decode thread, kept
    /// so a `seek` that arrives after the thread already died can still
    /// report the real reason (MPD keeps the error in `PlayerControl` and
    /// rethrows it from `SeekLocked`). Cleared when a new song starts.
    last_failure: Arc<Mutex<Option<String>>>,
    /// Identifies the current playback attempt: bumped by every `play()` and
    /// `stop()`. A [`Event::PlaybackError`] carries the generation of the
    /// decode thread that raised it, so the protocol layer can recognise (and
    /// drop) the report of a song the user has already stopped or replaced.
    generation: u64,
}

impl PlaybackEngine {
    pub fn new(
        event_bus: EventBus,
        status: Arc<RwLock<rmpd_core::state::PlayerStatus>>,
        atomic_state: Arc<AtomicU8>,
    ) -> Self {
        let volume = Arc::new(AtomicU8::new(100));
        let outputs = vec![OutputConfig::cpal_default()];
        let mixers = Arc::new(MixerSet::from_outputs(&outputs, &volume));
        Self {
            status,
            event_bus,
            stop_flag: Arc::new(AtomicBool::new(false)),
            atomic_state,
            playback_thread: None,
            current_song: Arc::new(Mutex::new(None)),
            volume,
            mixers,
            last_volume: 100,
            command_tx: None,
            outputs,
            replay_gain_mode: ReplayGainMode::default(),
            replay_gain_preamp: 0.0,
            replay_gain_missing_preamp: 0.0,
            volume_normalization: false,
            random: Arc::new(AtomicBool::new(false)),
            resampler_quality: ResamplerQuality::default(),
            dop_mode: DopMode::default(),
            output_slot: Arc::new(crate::output_slot::OutputSlot::new()),
            crossfade: 0,
            mixramp_db: 0.0,
            mixramp_delay: 0.0,
            next_song: Arc::new(Mutex::new(None)),
            buffer_time_ms: 500, // matches AudioConfig::default_buffer_time()
            control: Arc::new(OutputControl::new()),
            output_gains: Arc::new(crate::output_control::OutputGains::default()),
            live_position_base_bits: Arc::new(AtomicU64::new(0.0f64.to_bits())),
            live_sample_rate: Arc::new(AtomicU32::new(0)),
            play_time_ns: Arc::new(AtomicU64::new(0)),
            last_failure: Arc::new(Mutex::new(None)),
            generation: 0,
        }
    }

    pub fn set_outputs(&mut self, outputs: Vec<OutputConfig>) {
        self.outputs = outputs;
        self.mixers = Arc::new(MixerSet::from_outputs(&self.outputs, &self.volume));
        self.apply_software_gain();
    }

    /// Keep the software gain stage consistent with the active mixers: the
    /// requested volume while a software mixer is in use, unity (100%)
    /// otherwise (hardware / no mixer).
    fn apply_software_gain(&self) {
        let sw = if self.mixers.has_software() {
            self.last_volume
        } else {
            100
        };
        self.volume.store(sw, Ordering::Release);
        let gain = f32::from(sw) / 100.0;
        self.control.set_gain(gain);
        self.output_gains.set_software_gain(gain);
    }

    /// Whether any enabled output has a usable mixer (`false` when all are
    /// `mixer_type = none`: MPD then omits `volume` from `status`).
    #[must_use]
    pub fn volume_available(&self) -> bool {
        self.mixers.controls_volume()
    }

    /// Volume currently reported by the hardware mixers of the enabled
    /// outputs, or `None` when none uses one (software volume is tracked in
    /// the player status).
    #[must_use]
    pub fn hardware_volume(&self) -> Option<u8> {
        self.mixers.hardware_volume()
    }

    /// The mixer set when at least one enabled output uses a hardware mixer
    /// (for off-thread polling of external volume changes), else `None`.
    #[must_use]
    pub fn hardware_mixers(&self) -> Option<Arc<MixerSet>> {
        self.mixers.has_hardware().then(|| self.mixers.clone())
    }

    /// Apply a persisted volume (state file) to the software mixer. Hardware
    /// mixers keep the level the device already has, like MPD.
    pub fn restore_volume(&mut self, vol: u8) {
        self.last_volume = vol.min(100);
        self.apply_software_gain();
    }

    /// Set the output buffer time in milliseconds. Sizes the PCM ring buffer
    /// depth so playback latency / resilience matches the configured value.
    /// A value of 0 falls back to the safe default (500 ms).
    pub fn set_buffer_time(&mut self, ms: u32) {
        self.buffer_time_ms = if ms == 0 { 500 } else { ms };
    }

    pub fn set_replay_gain(&mut self, mode: ReplayGainMode, preamp: f32, missing_preamp: f32) {
        self.replay_gain_mode = mode;
        self.replay_gain_preamp = preamp;
        self.replay_gain_missing_preamp = missing_preamp;
    }

    /// Set random (shuffle) mode. Mirrors `set_replay_gain` above but uses
    /// `&self` / an atomic store since it must be callable while the decode
    /// thread concurrently reads it for `Auto` replay-gain recomputation.
    pub fn set_random(&self, random: bool) {
        self.random.store(random, Ordering::Relaxed);
    }

    pub fn set_volume_normalization(&mut self, on: bool) {
        self.volume_normalization = on;
    }

    /// Set the resampler quality used when the output device cannot natively
    /// play the decoded stream's rate.
    pub fn set_resampler_quality(&mut self, quality: ResamplerQuality) {
        self.resampler_quality = quality;
    }

    /// Set the DSD-over-PCM (DoP) mode for DSD sources.
    pub fn set_dop_mode(&mut self, mode: DopMode) {
        self.dop_mode = mode;
    }

    /// Set the crossfade duration.  0 = disabled (default).
    pub fn set_crossfade(&mut self, seconds: u32) {
        self.crossfade = seconds;
    }

    /// Set the MixRamp threshold and delay.
    ///
    /// `db` is the dBFS level at which the outgoing/incoming tracks are
    /// blended; `delay` is an additional silence gap in seconds. Both default
    /// to 0.0 (time-based crossfade fallback).
    pub fn set_mixramp(&mut self, db: f32, delay: f32) {
        self.mixramp_db = db;
        self.mixramp_delay = delay;
    }

    /// Feed the next song for a gapless or crossfade transition.
    ///
    /// Callable while playing (`&self`) — uses interior mutability.  The
    /// decode thread will claim the value with a single `take()` at the
    /// appropriate transition point.  Pass `None` to cancel a pre-fed song.
    pub fn set_next_song(&self, next: Option<rmpd_core::playback::PlaybackSong>) {
        *self.next_song.lock() = next;
    }

    /// Seek the playing song to `position` seconds and wait for the decoder's
    /// verdict. Convenience for callers that do not hold the engine lock
    /// across the wait; command handlers use [`Self::begin_seek`] so the
    /// engine lock is released first.
    pub async fn seek(&self, position: f64) -> Result<()> {
        self.begin_seek(position)?.verdict().await
    }

    /// First half of a seek: flush and queue the command (cheap, synchronous),
    /// returning a [`SeekPending`] to await once the engine lock has been
    /// released. The decoder can take seconds to answer (a network read, a
    /// song still opening); awaiting that under the engine `RwLock` would
    /// stall every other engine user (`status`, `stop`, `stats`, ...) behind
    /// it.
    pub fn begin_seek(&self, position: f64) -> Result<SeekPending> {
        let Some(tx) = &self.command_tx else {
            return Err(RmpdError::InvalidState("Not playing".to_owned()));
        };
        // Bump the flush generation BEFORE the decode thread even sees
        // the seek: every backend's real-time callback starts dropping
        // stale (pre-seek) queued/in-flight audio immediately, instead
        // of waiting for the decode thread to notice the command.
        self.control.flush();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if tx
            .send(PlaybackCommand::Seek {
                position,
                reply: Some(reply_tx),
            })
            .is_err()
        {
            // The decode thread already ended.
            return Err(seek_unavailable(&self.last_failure));
        }
        Ok(SeekPending {
            reply: reply_rx,
            last_failure: self.last_failure.clone(),
        })
    }

    /// The current playback generation (see the `generation` field): changes
    /// whenever a song is started or playback is stopped.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Total duration of audio played since the engine was created
    /// (MPD `PlayerControl::GetTotalPlayTime`, the `stats` `playtime` field).
    pub fn total_play_time(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.play_time_ns.load(Ordering::Relaxed))
    }

    pub async fn play(&mut self, playback_song: rmpd_core::playback::PlaybackSong) -> Result<()> {
        info!("starting playback: {}", playback_song.song.path);

        // Stop current playback if any (internal stop, no events - caller will emit)
        self.stop_internal().await?;

        // Flush BEFORE spawning the new decode thread: this is the single
        // entry point for both a genuinely fresh song AND a user-initiated
        // change (next/previous/play N) that `OutputSlot` may satisfy by
        // reusing an already-open, same-format `MultiOutput` for gapless
        // device persistence — reusing the device must NOT mean reusing its
        // queued audio. Bumping here drops any such stale backlog instantly
        // while natural (in-thread) gapless/crossfade advances, which never
        // call this, are left untouched.
        self.control.flush();

        // Update current song - clone the song from Arc
        *self.current_song.lock() = Some((*playback_song.song).clone());
        crate::httpd_output::set_now_playing(Some(crate::httpd_output::now_playing_label(
            &playback_song.song,
        )));

        // Reset stop flag
        self.stop_flag.store(false, Ordering::Release);
        // A new song forgets the previous failure (MPD `SeekLocked` ->
        // `ClearError`).
        *self.last_failure.lock() = None;

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel();
        self.command_tx = Some(command_tx);

        // Spawn playback thread
        let song_path = playback_song.resolved_path.clone();
        let event_bus = self.event_bus.clone();
        let stop_flag = self.stop_flag.clone();
        let volume = self.volume.clone();
        let output_gains = self.output_gains.clone();
        let status_clone = self.status.clone();
        let atomic_state_clone = self.atomic_state.clone();
        let outputs = self.outputs.clone();
        let gain_scale = Self::compute_gain_scale(
            &playback_song.song,
            self.replay_gain_mode,
            self.replay_gain_preamp,
            self.replay_gain_missing_preamp,
            self.volume_normalization,
            self.random.load(Ordering::Relaxed),
        );
        let resampler_quality = self.resampler_quality;
        let dop_mode = self.dop_mode;
        let output_slot = self.output_slot.clone();
        let next_song = self.next_song.clone();
        let crossfade_secs = self.crossfade;
        let current_song = self.current_song.clone();
        let random = self.random.clone();
        let replay_gain_mode = self.replay_gain_mode;
        let replay_gain_preamp = self.replay_gain_preamp;
        let replay_gain_missing_preamp = self.replay_gain_missing_preamp;
        let volume_normalization = self.volume_normalization;
        let mixramp_db = self.mixramp_db;
        let mixramp_delay = self.mixramp_delay;
        let range = playback_song.range;
        let buffer_time_ms = self.buffer_time_ms;
        let control = self.control.clone();
        let live_position_base_bits = self.live_position_base_bits.clone();
        let live_sample_rate = self.live_sample_rate.clone();
        let play_time_ns = self.play_time_ns.clone();
        // URI named in the `error:` text of a failure; refreshed from the
        // engine's current song at failure time (an in-thread gapless advance
        // may have moved on from the song this thread started with).
        let song_uri = playback_song.song.path.to_string();
        let current_song_err = self.current_song.clone();
        let last_failure = self.last_failure.clone();
        let generation = self.generation;
        let stop_flag_err = self.stop_flag.clone();

        let handle = thread::spawn(move || {
            let atomic_state_err = atomic_state_clone.clone();
            let event_bus_err = event_bus.clone();
            if let Err(failure) = Self::playback_thread(
                song_path.as_std_path(),
                status_clone,
                atomic_state_clone,
                event_bus,
                stop_flag,
                volume,
                output_gains,
                &command_rx,
                outputs,
                resampler_quality,
                dop_mode,
                gain_scale,
                output_slot,
                next_song,
                crossfade_secs,
                current_song,
                replay_gain_mode,
                replay_gain_preamp,
                replay_gain_missing_preamp,
                volume_normalization,
                random,
                mixramp_db,
                mixramp_delay,
                range,
                buffer_time_ms,
                control,
                live_position_base_bits,
                live_sample_rate,
                play_time_ns,
            ) {
                // The song was torn down by `stop`/`play` (which set the stop
                // flag before joining this thread): whatever made it fail
                // afterwards — a still-pending open, a closed output — is
                // the user's doing, not a playback error to report. Not
                // even `last_failure`/seek replies: `stop_internal` resets
                // the state itself.
                if stop_flag_err.load(Ordering::Acquire) {
                    debug!("playback aborted; ignoring failure: {:?}", failure);
                    return;
                }
                error!("playback error: {:?}", failure);
                // A decode/output failure must not leave the player stuck
                // reporting Play forever with no further events: reset the
                // state and tell the protocol layer what went wrong. It
                // records the `error:` text and then reacts like MPD's
                // `playlist::ResumePlayback` — skip to the next song after a
                // decoder error, stop after an output error (PLAY-01).
                let uri = current_song_err
                    .lock()
                    .as_ref()
                    .map_or_else(|| song_uri.clone(), |s| s.path.to_string());
                let (message, output) = failure.message(&uri);
                *last_failure.lock() = Some(message.clone());
                // A seek already queued behind the failure would otherwise
                // be dropped unanswered: give it the real reason.
                while let Ok(cmd) = command_rx.try_recv() {
                    if let PlaybackCommand::Seek {
                        reply: Some(reply), ..
                    } = cmd
                    {
                        let _ = reply.send(Err(RmpdError::Player(message.clone())));
                    }
                }
                atomic_state_err.store(PlayerState::Stop as u8, Ordering::Release);
                event_bus_err.emit(Event::PlaybackError {
                    message,
                    output,
                    generation,
                });
            }
        });

        self.playback_thread = Some(handle);

        // Update atomic state (caller must update status to avoid deadlock and emit events)
        self.atomic_state
            .store(PlayerState::Play as u8, Ordering::Release);

        Ok(())
    }

    pub async fn pause(&mut self) -> Result<()> {
        // Toggle atomic state - caller must update status to avoid deadlock
        let current = self.atomic_state.load(Ordering::Acquire);
        let new_state = match current {
            1 => PlayerState::Pause as u8, // Play -> Pause
            2 => PlayerState::Play as u8,  // Pause -> Play
            _ => return Ok(()),            // Stop -> do nothing
        };
        self.atomic_state.store(new_state, Ordering::Release);
        // Instant, lock-free: every backend's real-time callback reads this
        // directly and holds/resumes position without waiting for the
        // decode thread's next loop turn.
        self.control
            .set_paused(new_state == PlayerState::Pause as u8);
        self.wake_playback_thread();
        Ok(())
    }

    /// Set pause state explicitly (doesn't toggle)
    pub async fn set_pause(&mut self, should_pause: bool) -> Result<()> {
        let current = self.atomic_state.load(Ordering::Acquire);

        // Only transition if we're playing or paused (not stopped)
        if current == PlayerState::Play as u8 || current == PlayerState::Pause as u8 {
            let new_state = if should_pause {
                PlayerState::Pause as u8
            } else {
                PlayerState::Play as u8
            };
            self.atomic_state.store(new_state, Ordering::Release);
            self.control.set_paused(should_pause);
            self.wake_playback_thread();
        }
        Ok(())
    }

    /// Best-effort nudge for a decode thread that may be parked in a
    /// blocking `command_rx.recv()` while paused (see `playback_thread`'s
    /// pause branch) — a resume only touches shared atomics otherwise, so
    /// without this the thread wouldn't notice until some other command
    /// arrived. No-op if nothing is currently playing.
    fn wake_playback_thread(&self) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(PlaybackCommand::Wake);
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        debug!("stopping playback");
        self.stop_internal().await?;
        // User stop: tear down the cached output/device (song transitions use
        // stop_internal, which keeps it for gapless reuse). Dropping the last
        // `Arc<MultiOutput>` here can run `MultiOutput::drop`'s blocking
        // `join()` inline, so route it through spawn_blocking like the decode
        // thread join above (PLAY-02) — it must never stall a Tokio worker.
        let output_slot = self.output_slot.clone();
        let _ = tokio::task::spawn_blocking(move || output_slot.clear()).await;
        // Emit event to notify clients (external stop)
        self.event_bus.emit(Event::SongChanged(None));
        crate::httpd_output::set_now_playing(None);
        Ok(())
    }

    /// Internal stop - doesn't emit events (used when stopping before playing next song)
    async fn stop_internal(&mut self) -> Result<()> {
        debug!("internal stop (no events)");

        // Flush first: every backend's real-time callback starts dropping
        // queued/in-flight audio (and, while paused, still yields silence)
        // instantly, well before the decode thread notices `stop_flag` and
        // before the teardown handshake below completes.
        self.control.flush();

        // Set stop flag
        self.stop_flag.store(true, Ordering::Release);

        // Clear command channel. If the decode thread is parked in a
        // blocking `command_rx.recv()` (paused), dropping the sender makes
        // that call return `Err` immediately, which it treats as "stop".
        self.command_tx = None;

        // Wait for playback thread to finish. `JoinHandle::join` blocks the
        // calling thread; run it on a blocking-pool thread so it never stalls
        // a Tokio worker (or the `state.engine` write lock held by the async
        // caller) for however long the decode thread takes to notice
        // `stop_flag` and unwind. With the flush above making every
        // backend's real-time callback silence almost instantly, this is
        // now typically fast, but it's still a blocking syscall and must
        // never run inline on the async runtime.
        if let Some(handle) = self.playback_thread.take() {
            let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        }

        // A failure report from the song just torn down (or still in flight
        // on the event bus) must not be mistaken for one of the next song.
        self.generation = self.generation.wrapping_add(1);

        // Update atomic state (caller must update status to avoid deadlock)
        self.atomic_state
            .store(PlayerState::Stop as u8, Ordering::Release);
        // A stopped session must not leave `paused` set: otherwise the NEXT
        // `play()` (which does not touch this flag, only `flush()`s) would
        // start with every self-managed backend's callback silently holding
        // silence forever despite `atomic_state` correctly reporting Play.
        self.control.set_paused(false);
        // No active decode thread anymore: get_elapsed_live() must report
        // None rather than a frozen stale value from the finished song.
        self.live_sample_rate.store(0, Ordering::Release);
        *self.current_song.lock() = None;

        // Clear the look-ahead; the protocol re-feeds it after play().
        *self.next_song.lock() = None;

        Ok(())
    }

    pub async fn get_state(&self) -> PlayerState {
        let status = self.status.read().await;
        status.state
    }

    /// Get current state without locks (atomic, lock-free)
    pub fn get_state_atomic(&self) -> PlayerState {
        PlayerState::from_atomic(self.atomic_state.load(Ordering::Acquire))
    }

    pub async fn get_current_song(&self) -> Option<Song> {
        self.current_song.lock().clone()
    }

    /// Compute the audible elapsed position LIVE, at call time.
    ///
    /// Unlike `status.elapsed` (only refreshed by the decode thread's
    /// ~1s-throttled `PositionChanged` events, so it can read up to nearly a
    /// second stale), this reads `control.played_frames()` — a plain atomic
    /// the real-time callback increments on every frame it hands to the
    /// device — directly, so it is always within one audio chunk's worth of
    /// the true audible position. Stable (non-advancing) while paused, since
    /// `played_frames` itself does not advance then. Returns `None` when
    /// nothing is loaded/playing. Never blocks: every value read here is a
    /// lock-free atomic, never a lock the audio callback touches.
    pub fn get_elapsed_live(&self) -> Option<std::time::Duration> {
        let sample_rate = self.live_sample_rate.load(Ordering::Acquire);
        if sample_rate == 0 {
            return None;
        }
        let base = f64::from_bits(self.live_position_base_bits.load(Ordering::Acquire));
        let elapsed = base + self.control.played_frames() as f64 / f64::from(sample_rate);
        Some(std::time::Duration::from_secs_f64(elapsed.max(0.0)))
    }

    pub async fn set_volume(&mut self, vol: u8) -> Result<()> {
        let vol = vol.min(100);
        let mixers = self.mixers.clone();
        // Hardware mixers talk to the sound card: keep that off the async worker.
        let result = if mixers.has_hardware() {
            tokio::task::spawn_blocking(move || mixers.set_volume(vol))
                .await
                .map_err(|e| RmpdError::Player(format!("mixer task failed: {e}")))?
        } else {
            mixers.set_volume(vol)
        };
        result.map_err(|e| match e {
            MixerError::NoMixer => RmpdError::Player("problems setting volume".to_owned()),
            other => RmpdError::Player(format!("problems setting volume: {other}")),
        })?;
        self.last_volume = vol;
        // Instant: every self-managed backend's real-time callback ramps
        // toward this over a few ms (see `conversion::GainRamp`), instead of
        // the old write-time `VolumeFilter` which could lag by the full
        // queue depth (up to ~1s) before a change reached the device.
        // With only hardware mixers the software gain stays at unity.
        if self.mixers.has_software() {
            let gain = f32::from(vol) / 100.0;
            self.control.set_gain(gain);
            self.output_gains.set_software_gain(gain);
        }
        let reported = self.mixers.volume().unwrap_or(vol);
        self.event_bus.emit(Event::VolumeChanged(reported));
        Ok(())
    }

    pub async fn get_volume(&self) -> u8 {
        self.mixers
            .volume()
            .unwrap_or_else(|| self.volume.load(Ordering::Acquire))
    }

    #[allow(clippy::too_many_arguments)]
    fn playback_thread(
        path: &Path,
        _status: Arc<RwLock<rmpd_core::state::PlayerStatus>>,
        atomic_state: Arc<AtomicU8>,
        event_bus: EventBus,
        stop_flag: Arc<AtomicBool>,
        volume: Arc<AtomicU8>,
        output_gains: Arc<crate::output_control::OutputGains>,
        command_rx: &mpsc::Receiver<PlaybackCommand>,
        outputs: Vec<rmpd_core::config::OutputConfig>,
        resampler_quality: ResamplerQuality,
        dop_mode: DopMode,
        gain_scale: f32,
        output_slot: Arc<crate::output_slot::OutputSlot>,
        next_song: Arc<Mutex<Option<rmpd_core::playback::PlaybackSong>>>,
        crossfade_secs: u32,
        current_song: Arc<Mutex<Option<Song>>>,
        replay_gain_mode: ReplayGainMode,
        replay_gain_preamp: f32,
        replay_gain_missing_preamp: f32,
        volume_normalization: bool,
        random: Arc<AtomicBool>,
        mixramp_db: f32,
        mixramp_delay: f32,
        range: Option<(f64, f64)>,
        buffer_time_ms: u32,
        control: Arc<OutputControl>,
        live_position_base_bits: Arc<AtomicU64>,
        live_sample_rate: Arc<AtomicU32>,
        play_time_ns: Arc<AtomicU64>,
    ) -> std::result::Result<(), PlaybackFailure> {
        // Shadow as mutable so per-song gain can be updated on in-thread advance.
        let mut gain_scale = gain_scale;
        // Open decoder (pass-through mode by default)
        let mut decoder = SymphoniaDecoder::open(path)?;

        // Overrides the cpal stream rate for DSD-to-PCM: drives the device at
        // its native rate and lets rmpd's own StreamResampler bridge the gap,
        // avoiding a sound-server resample that causes underruns and leaves
        // DSD ultrasonic shaped noise in-band.
        let mut dsd_target_rate: Option<u32> = None;

        // DSD: native DoP playback is opt-in (RMPD_DOP=1); default is PCM.
        if decoder.is_dsd() {
            // DoP (1-bit DSD over PCM) only produces sound on a DoP-capable DAC
            // reached over a bit-perfect path. There is no reliable way to detect
            // that support, and selecting DoP for an ordinary DAC yields silence,
            // so DoP is opt-in. Default to PCM conversion, which always plays.
            // Resolve DoP: the `RMPD_DOP` env var overrides; otherwise use the
            // configured mode. `Auto` enables DoP only when an explicit output
            // device is configured (assumed a dedicated, DoP-capable DAC).
            let dop_enabled = match std::env::var("RMPD_DOP") {
                Ok(v) => matches!(v.trim(), "1" | "true" | "yes" | "on"),
                Err(_) => match dop_mode {
                    DopMode::Yes => true,
                    DopMode::No => false,
                    DopMode::Auto => crate::cpal_utils::output_device_configured(),
                },
            };

            if dop_enabled {
                info!("DSD file detected, attempting DoP output");
                // Release any cached PCM output so DoP can open the device.
                output_slot.clear();
                match Self::setup_dop(&decoder, control.clone()) {
                    Ok((dop_encoder, dop_out)) => {
                        info!("DoP output available, using native DSD playback");
                        return Self::run_dsd_dop(
                            decoder,
                            dop_encoder,
                            dop_out,
                            atomic_state,
                            event_bus,
                            stop_flag,
                            command_rx,
                            control,
                            live_position_base_bits,
                            live_sample_rate,
                            play_time_ns,
                        );
                    }
                    Err(e) => {
                        warn!("DoP playback not available: {}; falling back to PCM", e);
                    }
                }
            } else {
                info!(
                    "DSD file detected; using DSD-to-PCM conversion \
                     (set audio.dop=\"yes\" or RMPD_DOP=1 for native DSD on a DoP DAC)"
                );
            }

            // Pick the DSD-to-PCM decode rate sized to the device (see
            // `select_dsd_pcm_rate`), not to the device's huge advertised max.
            let device_rate = CpalOutput::default_output_rate().unwrap_or(48000);
            let decode_rate = select_dsd_pcm_rate(device_rate, CpalOutput::supports_rate);

            decoder.enable_pcm_conversion(decode_rate)?;
            // Play the decoded rate bit-perfect only on an explicitly configured
            // device that natively supports it (a real DAC on a bit-perfect path).
            // For the system default (typically a sound server that advertises
            // rates it actually resamples), open at the device's native rate and
            // let rmpd resample instead — avoiding a server-side resample that
            // underruns and leaves DSD ultrasonic noise in-band.
            let native_decode_ok = crate::cpal_utils::output_device_configured()
                && CpalOutput::supports_rate(decode_rate);
            dsd_target_rate = dsd_output_target_rate(decode_rate, device_rate, native_decode_ok);
            info!(
                "DSD-to-PCM conversion enabled at {} Hz (device {} Hz); cpal stream opens at {}",
                decode_rate,
                device_rate,
                if dsd_target_rate.is_some() {
                    "the device-native rate (rmpd resamples)"
                } else {
                    "the decode rate (no resampling)"
                }
            );
        }

        // Standard PCM playback (works for all formats including DSD with PCM conversion)
        let format = decoder.format();

        debug!(
            "decoder opened: {}Hz, {} channels",
            format.sample_rate, format.channels
        );

        // Build per-output boxes.  Fall back to null when no outputs configured
        // so playback still advances (position/EOS events fire) silently.
        let effective_outputs: Vec<rmpd_core::config::OutputConfig> = if outputs.is_empty() {
            vec![rmpd_core::config::OutputConfig {
                output_type: "null".into(),
                ..rmpd_core::config::OutputConfig::cpal_default()
            }]
        } else {
            outputs
        };

        let signature: Vec<String> = effective_outputs
            .iter()
            .map(|c| {
                // The resolved DSP chain is part of the identity: changing the
                // filter setup rebuilds the outputs. Empty when no filters are
                // configured, which leaves the key (and gapless reuse) as before.
                let fp = crate::filter::chain_fingerprint(c);
                if fp.is_empty() {
                    format!("{}|{}", c.output_type, c.name)
                } else {
                    format!("{}|{}|{fp}", c.output_type, c.name)
                }
            })
            .collect();
        let key = crate::output_slot::OutputKey {
            sample_rate: format.sample_rate,
            channels: format.channels,
            bits_per_sample: format.bits_per_sample,
            signature,
        };
        // Reuse the existing output (and its open device) across consecutive
        // same-key tracks for gapless transitions; rebuild on format/output
        // change. The closure (which opens devices) runs only on a cache miss.
        let multi = output_slot
            .acquire(key, || {
                // Per-output gain: software-mixer outputs follow the requested
                // volume, hardware / `none` ones stay at unity so the device
                // mixer is never stacked with digital attenuation.
                output_gains.clear();
                let mut boxes: Vec<(Box<dyn AudioOutput>, Arc<AtomicU8>)> =
                    Vec::with_capacity(effective_outputs.len());
                let mut chains: Vec<crate::filter::FilterChain> =
                    Vec::with_capacity(effective_outputs.len());
                for (i, cfg) in effective_outputs.iter().enumerate() {
                    let software = crate::mixer::output_uses_software_gain(cfg);
                    let out_control = output_gains.register(&control, software);
                    match Self::create_output(
                        format,
                        cfg,
                        resampler_quality,
                        buffer_time_ms,
                        dsd_target_rate,
                        out_control,
                    ) {
                        Ok(b) => {
                            let filter_volume = if software {
                                volume.clone()
                            } else {
                                Arc::new(AtomicU8::new(100))
                            };
                            boxes.push((b, filter_volume));
                            chains.push(crate::filter::chain_for_output(
                                cfg,
                                format.sample_rate,
                                format.channels,
                            ));
                        }
                        Err(e) => {
                            if i == 0 {
                                // MPD (`Filtered::Open`): `Failed to open "name" (plugin)`.
                                return Err(RmpdError::Player(format!(
                                    "Failed to open \"{}\" ({}): {}",
                                    cfg.name,
                                    cfg.output_type,
                                    error_text(&e)
                                )));
                            }
                            warn!(
                                "secondary output '{}' failed to create: {}; skipping",
                                cfg.name, e
                            );
                        }
                    }
                }
                Ok(Arc::new(
                    crate::multi_output::MultiOutput::spawn_with_filters(
                        boxes,
                        chains,
                        16,
                        control.clone(),
                    )?,
                ))
            })
            .map_err(PlaybackFailure::Output)?;

        // ── Playback state ────────────────────────────────────────────────────
        let mut buffer = vec![0.0f32; BUFFER_SIZE];
        let mut total_samples_played: u64 = 0;
        let samples_per_second = format.sample_rate as u64 * format.channels as u64;
        // Track whether we have sent pause/resume to the workers to avoid
        // spamming the same message every 100 ms.
        let mut multi_paused = false;
        // Audible-position base: `elapsed = position_base_secs +
        // control.played_frames() / format.sample_rate`. Updated on a
        // successful seek (to the seek target) and reset to 0.0 at every
        // natural (non-flushing) song transition below, alongside a soft
        // `control.reset_played_frames()`. Immune to however deep the
        // output queues are, unlike the old decoded-sample-count elapsed.
        let mut position_base_secs: f64 = range.map(|(start, _)| start).unwrap_or(0.0);
        live_sample_rate.store(format.sample_rate, Ordering::Release);
        live_position_base_bits.store(position_base_secs.to_bits(), Ordering::Release);
        // Last ICY "now playing" title emitted, to avoid re-emitting it every
        // throttle tick while it is unchanged (remote streams only).
        let mut last_stream_title: Option<String> = None;

        // Playback range (CUE virtual track / rangeid): seek to the start offset
        // and compute the sample count after which the song ends. `None` plays
        // the whole file. Range honoring is purely additive — when `range` is
        // None nothing below changes.
        let range_limit_samples: Option<u64> = match range {
            Some((start, end)) => {
                if start > 0.0
                    && let Err(e) = decoder.seek(start)
                {
                    warn!("failed to seek to range start {start}s: {e}");
                }
                // An end at or before the start means "play to EOF" (CUE last
                // track, or `rangeid id START:` with no end) — seek only.
                let span = end - start;
                if span > 0.0 {
                    Some((span * samples_per_second as f64).round() as u64)
                } else {
                    None
                }
            }
            None => None,
        };
        // ── Outer song loop ───────────────────────────────────────────────────
        // Each iteration decodes one song.  An in-thread advance (gapless or
        // crossfade) breaks the inner buffer loop, updates `decoder`, and loops
        // back here — the MultiOutput device stays open and audio is continuous.
        'song: loop {
            // Per-buffer inner loop
            'buf: loop {
                if stop_flag.load(Ordering::Acquire) {
                    break 'song;
                }

                // ── Commands ──────────────────────────────────────────────────
                if let Ok(cmd) = command_rx.try_recv() {
                    match cmd {
                        PlaybackCommand::Seek { position, reply } => {
                            debug!("seeking to position: {:.2}s", position);
                            let (result, position) = Self::seek_decoder(&mut decoder, position);
                            Self::apply_seek_result(
                                result,
                                "",
                                position,
                                &mut total_samples_played,
                                samples_per_second,
                                &mut position_base_secs,
                                &live_position_base_bits,
                                &event_bus,
                                reply,
                            );
                        }
                        PlaybackCommand::Wake => {}
                    }
                }

                // ── Pause ─────────────────────────────────────────────────────
                let current_state = PlayerState::from_atomic(atomic_state.load(Ordering::Acquire));
                if current_state == PlayerState::Pause {
                    if !multi_paused {
                        multi.pause();
                        multi_paused = true;
                    }
                    // Block until a command wakes us (resume/seek) or the
                    // sender is dropped (stop), instead of busy-polling
                    // every 100ms. The audible pause itself is already
                    // instant (`control.paused`, set directly by
                    // `PlaybackEngine::pause`/`set_pause` and read by every
                    // backend's real-time callback) — this just stops the
                    // decode thread from spinning while nothing can be
                    // played anyway.
                    match command_rx.recv() {
                        Ok(PlaybackCommand::Seek { position, reply }) => {
                            debug!("seeking to position: {:.2}s (while paused)", position);
                            let (result, position) = Self::seek_decoder(&mut decoder, position);
                            Self::apply_seek_result(
                                result,
                                " (while paused)",
                                position,
                                &mut total_samples_played,
                                samples_per_second,
                                &mut position_base_secs,
                                &live_position_base_bits,
                                &event_bus,
                                reply,
                            );
                        }
                        Ok(PlaybackCommand::Wake) | Err(_) => {}
                    }
                    continue 'buf;
                } else if multi_paused {
                    multi.resume();
                    multi_paused = false;
                }

                // ── Crossfade look-ahead ──────────────────────────────────────
                // DORMANT when crossfade_secs == 0 (the default): this entire
                // block is skipped, so behaviour is byte-identical to the
                // pre-look-ahead engine.
                let cf_end_samples: Option<u64> = if crossfade_secs > 0 {
                    match range_limit_samples {
                        // CUE/rangeid virtual track: the overlap window must
                        // end at the range boundary, not the underlying
                        // file's end (PLAY-06) — otherwise look-ahead never
                        // triggers before `reached_range_end` cuts the song
                        // off.
                        Some(limit) => Some(limit),
                        None => decoder
                            .duration()
                            .map(|d| (d * samples_per_second as f64) as u64),
                    }
                } else {
                    None
                };
                if let Some(end_samples) = cf_end_samples {
                    // Sample offset at which the overlap window begins
                    let cf_window = crossfade_secs as u64 * samples_per_second;
                    let cf_start = end_samples.saturating_sub(cf_window);
                    // mpd refuses to cross-fade a track too short to fit the
                    // window (mirrors `CrossFadeSettings::CanCrossFadeSong`,
                    // src/player/CrossFade.cxx): otherwise the fade would
                    // cover the whole song starting at sample 0.
                    let cf_eligible =
                        Self::can_cross_fade_song(end_samples, cf_window, samples_per_second);

                    if cf_eligible && total_samples_played >= cf_start {
                        // Claim the pre-fetched next song (destructive take —
                        // only the first crossing of cf_start ever finds a value).
                        let cf_next = next_song.lock().take().and_then(|ps| {
                            // Same rule applied to the incoming track: mpd
                            // requires both songs to individually satisfy
                            // `CanCrossFadeSong`. Unknown duration is treated
                            // as ineligible, matching the current-track arm
                            // above (`decoder.duration()` being `None` also
                            // disables crossfade entirely).
                            let next_ok = ps
                                .song
                                .duration
                                .map(|d| {
                                    let next_samples =
                                        (d.as_secs_f64() * samples_per_second as f64) as u64;
                                    Self::can_cross_fade_song(
                                        next_samples,
                                        cf_window,
                                        samples_per_second,
                                    )
                                })
                                .unwrap_or(false);
                            if !next_ok {
                                return None;
                            }
                            SymphoniaDecoder::open(ps.resolved_path.as_std_path())
                                .ok()
                                .filter(|dec| {
                                    !dec.is_dsd()
                                        && dec.format().sample_rate == format.sample_rate
                                        && dec.format().channels == format.channels
                                })
                                .map(|dec| (dec, ps))
                        });

                        if let Some((mut next_dec, ps)) = cf_next {
                            // ── Crossfade overlap loop ────────────────────
                            // Compute per-song gain for the incoming track.
                            let next_gain_scale = Self::compute_gain_scale(
                                &ps.song,
                                replay_gain_mode,
                                replay_gain_preamp,
                                replay_gain_missing_preamp,
                                volume_normalization,
                                random.load(Ordering::Relaxed),
                            );
                            // MixRamp: derive overlap window from tags; fall
                            // back to time-based crossfade if either tag is
                            // absent or the threshold is not crossed.
                            let cur_end_tag: Option<String> = current_song
                                .lock()
                                .as_ref()
                                .and_then(|s| s.tag("mixramp_end").map(str::to_owned));
                            let next_start_tag: Option<String> =
                                ps.song.tag("mixramp_start").map(str::to_owned);
                            let cur_rg_db = 20.0_f32 * gain_scale.max(1e-9_f32).log10();
                            let next_rg_db = 20.0_f32 * next_gain_scale.max(1e-9_f32).log10();
                            let window_secs: f32 = crate::crossfade::mixramp_overlap_seconds(
                                next_start_tag.as_deref(),
                                cur_end_tag.as_deref(),
                                mixramp_db,
                                cur_rg_db,
                                next_rg_db,
                                mixramp_delay,
                            )
                            .filter(|&s| s > 0.0)
                            .unwrap_or(crossfade_secs as f32);
                            let window = crate::crossfade::window_samples_secs(
                                format.sample_rate,
                                format.channels,
                                window_secs,
                            );
                            // Pre-allocated mixing buffers (no per-iteration alloc)
                            let mut cf_cur = vec![0.0f32; BUFFER_SIZE];
                            let mut cf_nxt = vec![0.0f32; BUFFER_SIZE];
                            let mut overlap_done: usize = 0;
                            // Tracks how many samples were consumed from next_dec
                            // during the overlap (becomes total_samples_played after
                            // the transition).
                            let mut next_pos: u64 = 0;
                            let mut transitioned = false;

                            'cf: loop {
                                if stop_flag.load(Ordering::Acquire) {
                                    break 'song;
                                }

                                // Pause inside crossfade
                                let st =
                                    PlayerState::from_atomic(atomic_state.load(Ordering::Acquire));
                                if st == PlayerState::Pause {
                                    if !multi_paused {
                                        multi.pause();
                                        multi_paused = true;
                                    }
                                    // Same blocking-wait treatment as the
                                    // main pause branch above — see its
                                    // comment.
                                    match command_rx.recv() {
                                        Ok(PlaybackCommand::Seek {
                                            position: pos,
                                            reply,
                                        }) => {
                                            let (result, pos) =
                                                Self::seek_decoder(&mut decoder, pos);
                                            Self::apply_seek_result(
                                                result,
                                                " during crossfade (while paused)",
                                                pos,
                                                &mut total_samples_played,
                                                samples_per_second,
                                                &mut position_base_secs,
                                                &live_position_base_bits,
                                                &event_bus,
                                                reply,
                                            );
                                            // next_dec is dropped here; next_song
                                            // slot is already empty so the
                                            // protocol must re-feed.
                                            break 'cf;
                                        }
                                        Ok(PlaybackCommand::Wake) | Err(_) => {}
                                    }
                                    continue 'cf;
                                } else if multi_paused {
                                    multi.resume();
                                    multi_paused = false;
                                }

                                // Seek during crossfade: seek current decoder and
                                // abandon the blend so the user hears the new
                                // position without the incoming track underneath.
                                if let Ok(PlaybackCommand::Seek {
                                    position: pos,
                                    reply,
                                }) = command_rx.try_recv()
                                {
                                    let (result, pos) = Self::seek_decoder(&mut decoder, pos);
                                    Self::apply_seek_result(
                                        result,
                                        " during crossfade",
                                        pos,
                                        &mut total_samples_played,
                                        samples_per_second,
                                        &mut position_base_secs,
                                        &live_position_base_bits,
                                        &event_bus,
                                        reply,
                                    );
                                    // next_dec is dropped here; next_song slot is
                                    // already empty so the protocol must re-feed.
                                    break 'cf;
                                }

                                // Window exhausted: outgoing is fully faded out
                                if overlap_done >= window {
                                    transitioned = true;
                                    break 'cf;
                                }

                                // Read from outgoing decoder
                                let n_cur = decoder.read(&mut cf_cur)?;
                                if n_cur == 0 {
                                    // Outgoing ended inside window → switch fully
                                    transitioned = true;
                                    break 'cf;
                                }

                                // Read the same count from incoming, keeping both
                                // decoders aligned (format is guaranteed to match).
                                let n_nxt = next_dec.read(&mut cf_nxt[..n_cur])?;
                                if n_nxt == 0 {
                                    // Edge case: next song shorter than the crossfade
                                    // window.  Play the remaining outgoing samples
                                    // unmodified and abandon the blend.
                                    for s in cf_cur[..n_cur].iter_mut() {
                                        *s *= gain_scale;
                                    }
                                    if multi.write(Arc::from(&cf_cur[..n_cur])).is_err() {
                                        warn!("output disconnected (crossfade/next-eof)");
                                        if stop_flag.load(Ordering::Acquire) {
                                            break 'song;
                                        }
                                        return Err(Self::output_gone());
                                    }
                                    Self::add_play_time(
                                        &play_time_ns,
                                        n_cur as u64,
                                        samples_per_second,
                                    );
                                    total_samples_played += n_cur as u64;
                                    // Continue with current decoder; next_dec dropped.
                                    break 'cf;
                                }

                                // Equal-power blend into cf_cur (n_nxt ≤ n_cur)
                                let n_mix = n_nxt;
                                let progress =
                                    (overlap_done as f32 / window as f32).clamp(0.0, 1.0);
                                let (g_out, g_in) = crate::crossfade::equal_power_gains(progress);

                                // In-place: cf_cur = cur*gain*g_out + nxt*gain*g_in
                                for s in cf_cur[..n_mix].iter_mut() {
                                    *s *= gain_scale * g_out;
                                }
                                crate::crossfade::mix_into(
                                    &mut cf_cur[..n_mix],
                                    &cf_nxt[..n_mix],
                                    1.0,
                                    next_gain_scale * g_in,
                                );

                                // If the incoming track ran out mid-chunk
                                // (n_mix < n_cur), its tail has no next-track
                                // counterpart to blend with; apply the
                                // outgoing gain unmixed rather than silently
                                // dropping those samples (PLAY-10).
                                if n_mix < n_cur {
                                    for s in cf_cur[n_mix..n_cur].iter_mut() {
                                        *s *= gain_scale * g_out;
                                    }
                                }

                                if multi.write(Arc::from(&cf_cur[..n_cur])).is_err() {
                                    warn!("output disconnected during crossfade");
                                    if stop_flag.load(Ordering::Acquire) {
                                        break 'song;
                                    }
                                    return Err(Self::output_gone());
                                }
                                Self::add_play_time(
                                    &play_time_ns,
                                    n_cur as u64,
                                    samples_per_second,
                                );

                                overlap_done += n_mix;
                                next_pos += n_mix as u64;
                                total_samples_played += n_cur as u64;

                                // Position/bitrate events (~1 s throttle)
                                if total_samples_played % samples_per_second < (n_mix as u64) {
                                    let elapsed = position_base_secs
                                        + control.played_frames() as f64
                                            / format.sample_rate as f64;
                                    event_bus.emit(Event::PositionChanged(
                                        std::time::Duration::from_secs_f64(elapsed),
                                    ));
                                    event_bus
                                        .emit(Event::BitrateChanged(decoder.current_bitrate()));
                                }
                            } // end 'cf

                            if transitioned {
                                decoder = next_dec;
                                total_samples_played = next_pos;
                                *current_song.lock() = Some((*ps.song).clone());
                                event_bus.emit(Event::AdvancedToNext);
                                // Update gain for the now-active next song.
                                gain_scale = next_gain_scale;
                                // Natural (non-flushing) transition: restart the
                                // audible-elapsed base at 0 for the new song
                                // without discarding any queued audio (a
                                // `control.flush()` would audibly interrupt the
                                // crossfade that just played).
                                position_base_secs = 0.0;
                                live_position_base_bits.store(0.0f64.to_bits(), Ordering::Release);
                                control.reset_played_frames();
                                // Break inner loop; 'song iterates with new decoder.
                                break 'buf;
                            }
                            // Crossfade was abandoned (seek or next-eof).
                            // Continue the inner loop with the current decoder;
                            // next_song is already empty so subsequent iterations
                            // find nothing and fall through to normal decode.
                            continue 'buf;
                        }
                        // cf_next was None (no valid next available yet) → fall through
                    }
                }

                // ── Normal decode ─────────────────────────────────────────────
                let samples_read = decoder.read(&mut buffer)?;

                if samples_read == 0 {
                    debug!(
                        "End of stream reached, total samples decoded: {}",
                        total_samples_played
                    );

                    // DORMANCY: when next_song is empty (the default until the
                    // protocol feeds it), `next_song.lock().take()` returns None
                    // and we always take the SongFinished branch — byte-identical
                    // to the pre-look-ahead engine.  Only when the protocol has
                    // pre-fed a format-compatible next song does the gapless path
                    // activate.
                    let gapless_next = next_song.lock().take().and_then(|ps| {
                        SymphoniaDecoder::open(ps.resolved_path.as_std_path())
                            .ok()
                            .filter(|dec| {
                                !dec.is_dsd()
                                    && dec.format().sample_rate == format.sample_rate
                                    && dec.format().channels == format.channels
                            })
                            .map(|dec| (dec, ps))
                    });

                    match gapless_next {
                        Some((next_dec, ps)) => {
                            // In-thread gapless advance: same MultiOutput stays
                            // open, audio is continuous with no gap.
                            decoder = next_dec;
                            total_samples_played = 0;
                            *current_song.lock() = Some((*ps.song).clone());
                            event_bus.emit(Event::AdvancedToNext);
                            // Recompute gain for the new song (it has its own tags).
                            gain_scale = Self::compute_gain_scale(
                                &ps.song,
                                replay_gain_mode,
                                replay_gain_preamp,
                                replay_gain_missing_preamp,
                                volume_normalization,
                                random.load(Ordering::Relaxed),
                            );
                            // Natural (non-flushing) transition: see the
                            // crossfade transition above for rationale.
                            position_base_secs = 0.0;
                            live_position_base_bits.store(0.0f64.to_bits(), Ordering::Release);
                            control.reset_played_frames();
                            break 'buf; // continue 'song
                        }
                        None => {
                            // Default (dormant) path — identical to today.
                            event_bus.emit(Event::SongFinished);
                            break 'song;
                        }
                    }
                }

                // Range cutoff (CUE virtual track / rangeid): truncate the final
                // chunk at the end boundary, then finish the song below.
                let mut samples_read = samples_read;
                let mut reached_range_end = false;
                if let Some(limit) = range_limit_samples
                    && total_samples_played + samples_read as u64 >= limit
                {
                    samples_read = limit.saturating_sub(total_samples_played) as usize;
                    reached_range_end = true;
                }

                if samples_read < buffer.len() {
                    debug!(
                        "partial read: {} samples (buffer size: {})",
                        samples_read,
                        buffer.len()
                    );
                }

                // Apply ReplayGain source-side (lock-free); volume is applied
                // per-output in each MultiOutput worker via VolumeFilter.
                for sample in buffer[..samples_read].iter_mut() {
                    *sample *= gain_scale;
                }

                // Fan the chunk out to all outputs.
                let chunk: Arc<[f32]> = Arc::from(&buffer[..samples_read]);
                if multi.write(chunk).is_err() {
                    warn!("primary output disconnected; stopping playback");
                    if stop_flag.load(Ordering::Acquire) {
                        break 'song;
                    }
                    return Err(Self::output_gone());
                }
                Self::add_play_time(&play_time_ns, samples_read as u64, samples_per_second);

                // Update elapsed time
                total_samples_played += samples_read as u64;

                if reached_range_end {
                    debug!("reached range end at {total_samples_played} samples");
                    event_bus.emit(Event::SongFinished);
                    break 'song;
                }

                // Emit position update event every ~1 second of audio (throttled)
                if total_samples_played % samples_per_second < (samples_read as u64) {
                    let elapsed_seconds = position_base_secs
                        + control.played_frames() as f64 / format.sample_rate as f64;
                    event_bus.emit(Event::PositionChanged(std::time::Duration::from_secs_f64(
                        elapsed_seconds,
                    )));

                    // Also emit current bitrate (for VBR files this changes during playback)
                    let current_bitrate = decoder.current_bitrate();
                    event_bus.emit(Event::BitrateChanged(current_bitrate));

                    // Surface ICY "now playing" title changes for remote streams.
                    let title = decoder.stream_title();
                    if title != last_stream_title {
                        last_stream_title = title.clone();
                        event_bus.emit(Event::StreamTitleChanged(title));
                    }
                    // Feed the live title to httpd ICY output: prefer the upstream
                    // ICY stream title for internet radio; fall back to song tags.
                    let now = decoder.stream_title().or_else(|| {
                        current_song
                            .lock()
                            .as_ref()
                            .map(crate::httpd_output::now_playing_label)
                    });
                    crate::httpd_output::set_now_playing(now);
                }
            }
            // 'buf exited normally (in-thread advance) → 'song loops with the new decoder
        }

        Ok(())
    }

    /// Seek `decoder` to `position` seconds, clamped to the song's duration
    /// when known. Returns the outcome together with the position actually
    /// targeted.
    ///
    /// MPD clamps an out-of-range seek to the end of the song
    /// (`Player::SeekDecoder`: `if (seek_time > total_time) seek_time =
    /// total_time`) — the song then simply ends — rather than failing.
    fn seek_decoder(decoder: &mut SymphoniaDecoder, position: f64) -> (Result<()>, f64) {
        let position = match decoder.duration() {
            Some(duration) if position > duration => duration,
            _ => position,
        };
        (decoder.seek(position), position)
    }

    /// The failure reported when the output stopped accepting audio while the
    /// song was still playing (the worker died, e.g. the device went away).
    fn output_gone() -> PlaybackFailure {
        PlaybackFailure::Output(RmpdError::Player("Audio output disconnected".to_owned()))
    }

    /// Add `samples` interleaved samples (at `samples_per_second`) to the
    /// engine-wide played-time counter behind `stats` `playtime`.
    fn add_play_time(counter: &AtomicU64, samples: u64, samples_per_second: u64) {
        if let Some(nanos) = samples
            .saturating_mul(1_000_000_000)
            .checked_div(samples_per_second)
        {
            counter.fetch_add(nanos, Ordering::Relaxed);
        }
    }

    /// Apply a `PlaybackCommand::Seek` outcome to the running sample counter
    /// and (re)broadcast the resulting position.
    ///
    /// On success, `counter` is resynchronised to `target_secs`. On failure,
    /// `counter` is left untouched: `Decoder` exposes no position/timestamp
    /// getter to recover the decoder's actual read position, and a failed
    /// accurate seek can leave the underlying stream scanned past its
    /// pre-seek location without rewinding it, so guessing a new value would
    /// just trade one wrong position for another. Either way
    /// `Event::PositionChanged` is (re)emitted with whatever `counter` now
    /// says, so a client that already assumed the seek succeeded is
    /// corrected immediately instead of drifting until the next throttled
    /// tick. The outcome is also sent to `reply` (if any) so the `seek`
    /// command can answer with the decoder's actual error.
    #[allow(clippy::too_many_arguments)]
    fn apply_seek_result(
        result: Result<()>,
        log_suffix: &str,
        target_secs: f64,
        counter: &mut u64,
        units_per_second: u64,
        position_base_secs: &mut f64,
        live_position_base_bits: &Arc<AtomicU64>,
        event_bus: &EventBus,
        reply: Option<tokio::sync::oneshot::Sender<Result<()>>>,
    ) {
        match &result {
            Ok(()) => {
                *counter = (target_secs * units_per_second as f64) as u64;
                // Audible-elapsed base resynchronises to the seek target;
                // the flush the engine issued before sending this command
                // already reset `control.played_frames()` to 0, so the two
                // stay in lockstep from here.
                *position_base_secs = target_secs;
                live_position_base_bits.store(target_secs.to_bits(), Ordering::Release);
            }
            Err(e) => error!("seek failed{log_suffix}: {e}"),
        }
        let elapsed = *counter as f64 / units_per_second as f64;
        event_bus.emit(Event::PositionChanged(std::time::Duration::from_secs_f64(
            elapsed,
        )));
        if let Some(reply) = reply {
            let _ = reply.send(result);
        }
    }

    fn create_output(
        format: rmpd_core::song::AudioFormat,
        cfg: &OutputConfig,
        quality: ResamplerQuality,
        buffer_time_ms: u32,
        dsd_target_rate: Option<u32>,
        control: Arc<OutputControl>,
    ) -> Result<Box<dyn AudioOutput>> {
        crate::output_registry::create_output(
            format,
            quality,
            cfg,
            buffer_time_ms,
            dsd_target_rate,
            control,
        )
    }

    fn compute_gain_scale(
        song: &Song,
        mode: ReplayGainMode,
        preamp: f32,
        missing_preamp: f32,
        normalization: bool,
        random: bool,
    ) -> f32 {
        if mode == ReplayGainMode::Off {
            return 1.0;
        }
        let (gain_opt, peak_opt) = match mode {
            ReplayGainMode::Off => unreachable!(),
            ReplayGainMode::Track => (song.replay_gain_track_gain, song.replay_gain_track_peak),
            ReplayGainMode::Album => (song.replay_gain_album_gain, song.replay_gain_album_peak),
            // MPD `ReplayGainMode::AUTO`: random mode breaks album context (the
            // songs no longer play in album order), so use track gain while
            // shuffled and album gain otherwise. See mpd `src/ReplayGainMode.hxx`
            // and `doc/user.rst` ("replaygain auto"). No cross-fallback to the
            // other gain when the selected one is missing — that's MPD's
            // behaviour too, it just falls through to `missing_preamp` below.
            ReplayGainMode::Auto => {
                if random {
                    (song.replay_gain_track_gain, song.replay_gain_track_peak)
                } else {
                    (song.replay_gain_album_gain, song.replay_gain_album_peak)
                }
            }
        };
        let (db, peak) = if let Some(gain) = gain_opt {
            (gain + preamp, peak_opt)
        } else {
            (missing_preamp, None)
        };
        let mut scale = 10f32.powf(db / 20.0);
        if normalization
            && let Some(pk) = peak
            && pk > 0.0
            && scale * pk > 1.0
        {
            scale = 1.0 / pk;
        }
        scale
    }

    /// Whether a song of `total_samples` length is eligible to cross-fade
    /// with a `cf_window`-sample overlap. Mirrors mpd
    /// `CrossFadeSettings::CanCrossFadeSong` (`src/player/CrossFade.cxx`):
    /// the song must be at least `CROSSFADE_MIN_TOTAL_SECS` long, and the
    /// crossfade window must fit strictly within it. `total_samples` and
    /// `cf_window` are both interleaved sample counts at `samples_per_second`.
    fn can_cross_fade_song(total_samples: u64, cf_window: u64, samples_per_second: u64) -> bool {
        let min_total_samples = CROSSFADE_MIN_TOTAL_SECS * samples_per_second;
        total_samples >= min_total_samples && cf_window < total_samples
    }

    /// Build the DoP encoder and start the DoP output for `decoder`. Building and
    /// starting the stream here means any failure (configured device can't do the
    /// DoP rate, device busy, no DoP DAC) surfaces as an error so the caller can
    /// cleanly revert to PCM instead of aborting playback.
    fn setup_dop(
        decoder: &SymphoniaDecoder,
        control: Arc<OutputControl>,
    ) -> Result<(DopEncoder, DopOutput)> {
        let dsd_sample_rate = decoder.sample_rate();
        let channels = decoder.channels();
        let channel_layout = decoder
            .channel_data_layout()
            .unwrap_or(symphonia::core::codecs::audio::ChannelDataLayout::Planar);
        let bit_order = decoder
            .bit_order()
            .unwrap_or(symphonia::core::codecs::audio::BitOrder::LsbFirst);

        let dop_encoder = DopEncoder::new(
            dsd_sample_rate,
            channels as usize,
            channel_layout,
            bit_order,
        )?;
        let pcm_sample_rate = dop_encoder.pcm_sample_rate();

        info!(
            "dsd playback: {} Hz, {} channels",
            dsd_sample_rate, channels
        );
        info!(
            "dsd format: channel_layout={:?}, bit_order={:?}",
            channel_layout, bit_order
        );
        info!(
            "DoP encoding: DSD {} Hz -> PCM {} Hz",
            dsd_sample_rate, pcm_sample_rate
        );

        let mut output = DopOutput::new(pcm_sample_rate, channels, control)?;
        output.start()?;

        Ok((dop_encoder, output))
    }

    /// DSD playback loop over an already-started DoP output.
    #[allow(clippy::too_many_arguments)]
    fn run_dsd_dop(
        mut decoder: SymphoniaDecoder,
        mut dop_encoder: DopEncoder,
        mut output: DopOutput,
        atomic_state: Arc<AtomicU8>,
        event_bus: EventBus,
        stop_flag: Arc<AtomicBool>,
        command_rx: &mpsc::Receiver<PlaybackCommand>,
        control: Arc<OutputControl>,
        live_position_base_bits: Arc<AtomicU64>,
        live_sample_rate: Arc<AtomicU32>,
        play_time_ns: Arc<AtomicU64>,
    ) -> std::result::Result<(), PlaybackFailure> {
        let dsd_sample_rate = decoder.sample_rate();
        let channels = decoder.channels();
        let pcm_sample_rate = dop_encoder.pcm_sample_rate();

        let mut dsd_buffer = Vec::new();
        let mut dop_i32_buffer = Vec::new();
        let mut total_dsd_bytes: u64 = 0;
        let dsd_bytes_per_second = (dsd_sample_rate / 8) as u64 * channels as u64;
        // Track whether pause() has been called so we only call it once on
        // entry (matching the multi_paused pattern in the PCM path).
        let mut dsd_paused = false;
        // Audible-position base, mirroring the PCM path's
        // `position_base_secs`: `elapsed = position_base_secs +
        // control.played_frames() / pcm_sample_rate`. `played_frames` is
        // maintained by `DopOutput`'s own real-time callback (it shares this
        // SAME `control`), so it reflects frames actually handed to the
        // device, immune to the DoP channel's queue depth.
        let mut position_base_secs: f64 = 0.0;
        live_sample_rate.store(pcm_sample_rate, Ordering::Release);
        live_position_base_bits.store(0.0f64.to_bits(), Ordering::Release);

        'dsd: while !stop_flag.load(Ordering::Acquire) {
            // Check for commands
            if let Ok(cmd) = command_rx.try_recv() {
                match cmd {
                    PlaybackCommand::Seek { position, reply } => {
                        debug!("seeking to position: {:.2}s", position);
                        let (result, position) = Self::seek_decoder(&mut decoder, position);
                        if let Err(e) = &result {
                            error!("seek failed: {}", e);
                        } else {
                            total_dsd_bytes = (position * dsd_bytes_per_second as f64) as u64;
                            position_base_secs = position;
                            live_position_base_bits.store(position.to_bits(), Ordering::Release);
                            event_bus.emit(Event::PositionChanged(
                                std::time::Duration::from_secs_f64(position),
                            ));
                        }
                        if let Some(reply) = reply {
                            let _ = reply.send(result);
                        }
                    }
                    PlaybackCommand::Wake => {}
                }
            }

            // Check if paused
            let current_state = PlayerState::from_atomic(atomic_state.load(Ordering::Acquire));

            if current_state == PlayerState::Pause {
                if !dsd_paused {
                    let _ = output.pause();
                    dsd_paused = true;
                }
                // Block until a command wakes us (resume/seek) or the
                // sender is dropped (stop), instead of busy-polling every
                // 100ms — same treatment as the PCM path's pause branch.
                // The audible pause is already instant: `output`'s
                // real-time callback reads `control.paused` directly.
                match command_rx.recv() {
                    Ok(PlaybackCommand::Seek { position, reply }) => {
                        debug!("seeking to position: {:.2}s (while paused)", position);
                        let (result, position) = Self::seek_decoder(&mut decoder, position);
                        if let Err(e) = &result {
                            error!("seek failed (while paused): {}", e);
                        } else {
                            total_dsd_bytes = (position * dsd_bytes_per_second as f64) as u64;
                            position_base_secs = position;
                            live_position_base_bits.store(position.to_bits(), Ordering::Release);
                            event_bus.emit(Event::PositionChanged(
                                std::time::Duration::from_secs_f64(position),
                            ));
                        }
                        if let Some(reply) = reply {
                            let _ = reply.send(result);
                        }
                    }
                    Ok(PlaybackCommand::Wake) | Err(_) => {}
                }
                continue 'dsd;
            } else if dsd_paused {
                let _ = output.resume();
                dsd_paused = false;
            }

            // Read raw DSD data
            let bytes_read = decoder.read_dsd_raw(&mut dsd_buffer)?;

            if bytes_read == 0 {
                debug!("end of DSD stream reached");
                event_bus.emit(Event::SongFinished);
                break;
            }

            // Encode to DoP
            dop_encoder.encode(&dsd_buffer, &mut dop_i32_buffer);

            // Write DoP samples (i32 to preserve marker precision)
            output
                .write(&dop_i32_buffer)
                .map_err(PlaybackFailure::Output)?;
            play_time_ns.fetch_add(
                (bytes_read as u64).saturating_mul(1_000_000_000) / dsd_bytes_per_second.max(1),
                Ordering::Relaxed,
            );

            // Update elapsed time (decoded-position throttle trigger only;
            // see below for the emitted, audible-position value).
            total_dsd_bytes += bytes_read as u64;

            // Emit position update every ~1 second
            if total_dsd_bytes % dsd_bytes_per_second < (bytes_read as u64) {
                let elapsed_seconds =
                    position_base_secs + control.played_frames() as f64 / pcm_sample_rate as f64;
                event_bus.emit(Event::PositionChanged(std::time::Duration::from_secs_f64(
                    elapsed_seconds,
                )));

                let current_bitrate = decoder.current_bitrate();
                event_bus.emit(Event::BitrateChanged(current_bitrate));
            }
        }

        output.stop().map_err(PlaybackFailure::Output)?;

        Ok(())
    }
}

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.playback_thread.take() {
            // Never join inline: Drop can run on a Tokio worker (e.g. the
            // last `Arc<RwLock<PlaybackEngine>>` clone dropped from an async
            // handler), and joining would stall it until the decode thread
            // notices `stop_flag` and unwinds (PLAY-04). Detach the join to
            // a dedicated OS thread instead.
            thread::spawn(move || {
                let _ = handle.join();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipewire_like_device_picks_moderate_rate_not_advertised_max() {
        // PipeWire advertises everything (huge range) and defaults to 48 kHz.
        // We must NOT pick 352.8 kHz; the smallest family rate covering 48 kHz
        // is 88.2 kHz.
        let rate = select_dsd_pcm_rate(48000, |_| true);
        assert_eq!(rate, 88200);
    }

    #[test]
    fn device_at_44100_picks_44100() {
        let rate = select_dsd_pcm_rate(44100, |_| true);
        assert_eq!(rate, 44100);
    }

    #[test]
    fn device_at_96000_picks_176400() {
        // 88.2 kHz does not cover 96 kHz; the smallest family rate >= 96 kHz is
        // 176.4 kHz.
        let rate = select_dsd_pcm_rate(96000, |_| true);
        assert_eq!(rate, 176400);
    }

    #[test]
    fn device_at_192000_picks_352800() {
        let rate = select_dsd_pcm_rate(192000, |_| true);
        assert_eq!(rate, 352800);
    }

    #[test]
    fn strict_48k_device_falls_back_to_largest_supported_family_rate() {
        // A device that only natively supports 44.1 kHz (e.g. some hw-locked
        // ALSA devices) while running at 48 kHz: no family rate >= 48 kHz is
        // supported, so fall back to the largest supported one (44.1 kHz). The
        // output layer then resamples 44.1 -> 48 kHz.
        let rate = select_dsd_pcm_rate(48000, |r| r == 44100);
        assert_eq!(rate, 44100);
    }

    #[test]
    fn no_support_info_falls_back_to_default() {
        let rate = select_dsd_pcm_rate(48000, |_| false);
        assert_eq!(rate, 88200);
    }

    // ── dsd_output_target_rate tests ─────────────────────────────────────────

    #[test]
    fn dsd_output_target_rate_default_device_resamples() {
        // System default (PipeWire, not bit-perfect): DSD decoded at 88200 Hz,
        // device natively 48000 Hz. rmpd must open cpal at 48000 and resample
        // internally, not let PipeWire resample the advertised-but-non-native
        // 88200 Hz stream.
        assert_eq!(dsd_output_target_rate(88200, 48000, false), Some(48000));
    }

    #[test]
    fn dsd_output_target_rate_configured_dac_is_bit_perfect() {
        // Explicit DAC that natively supports 88200 Hz: open at the decode rate
        // (no override) so playback is bit-perfect, even though the device's
        // default rate is 48000 Hz.
        assert_eq!(dsd_output_target_rate(88200, 48000, true), None);
    }

    #[test]
    fn dsd_output_target_rate_same_returns_none() {
        // Device natively 44100 Hz, decode rate 44100 Hz: no extra resample needed.
        assert_eq!(dsd_output_target_rate(44100, 44100, false), None);
    }

    #[test]
    fn select_dsd_pcm_rate_unchanged_by_output_fix() {
        // Decode-rate selection is not affected by the output-rate fix.
        // A PipeWire-like device at 48000 Hz that advertises 88200 Hz support
        // should still decode to 88200 Hz; only the cpal stream opens at 48000.
        let rate = select_dsd_pcm_rate(48000, |r| r == 88200);
        assert_eq!(rate, 88200);
    }

    // ── compute_gain_scale tests (Finding C1: ReplayGain `auto` mode) ───────

    #[test]
    fn compute_gain_scale_auto_random_uses_track_gain() {
        let mut song = rmpd_core::test_utils::create_test_song(1, "auto-random");
        song.replay_gain_track_gain = Some(-3.0);
        song.replay_gain_album_gain = Some(-6.0);
        // Random mode: `auto` must select track gain, not album gain, even
        // though both are present (mpd `ReplayGainMode::AUTO`, random on).
        let scale =
            PlaybackEngine::compute_gain_scale(&song, ReplayGainMode::Auto, 0.0, 0.0, false, true);
        let expected = 10f32.powf(-3.0 / 20.0);
        assert!((scale - expected).abs() < 1e-6);
    }

    #[test]
    fn compute_gain_scale_auto_not_random_uses_album_gain() {
        let mut song = rmpd_core::test_utils::create_test_song(2, "auto-sequential");
        song.replay_gain_track_gain = Some(-3.0);
        song.replay_gain_album_gain = Some(-6.0);
        // Sequential (non-random) mode: `auto` must select album gain.
        let scale =
            PlaybackEngine::compute_gain_scale(&song, ReplayGainMode::Auto, 0.0, 0.0, false, false);
        let expected = 10f32.powf(-6.0 / 20.0);
        assert!((scale - expected).abs() < 1e-6);
    }

    #[test]
    fn compute_gain_scale_auto_not_random_missing_album_gain_falls_back_to_missing_preamp() {
        let mut song = rmpd_core::test_utils::create_test_song(3, "auto-no-album-gain");
        song.replay_gain_track_gain = Some(-3.0);
        song.replay_gain_album_gain = None;
        // No cross-fallback to track gain: a missing album gain in
        // non-random `auto` mode uses `missing_preamp`, matching MPD.
        let scale = PlaybackEngine::compute_gain_scale(
            &song,
            ReplayGainMode::Auto,
            0.0,
            -9.0,
            false,
            false,
        );
        let expected = 10f32.powf(-9.0 / 20.0);
        assert!((scale - expected).abs() < 1e-6);
    }

    #[test]
    fn crossfade_normal_length_track_is_eligible() {
        // 3 minute track at 44.1kHz stereo, 5s crossfade window: comfortably
        // longer than both `CROSSFADE_MIN_TOTAL_SECS` and the fade window.
        let samples_per_second = 44100u64 * 2;
        let total_samples = 180 * samples_per_second;
        let cf_window = 5 * samples_per_second;
        assert!(PlaybackEngine::can_cross_fade_song(
            total_samples,
            cf_window,
            samples_per_second
        ));
    }

    #[test]
    fn crossfade_too_short_track_is_not_eligible() {
        // A 3s track can't fit a 5s crossfade window, and is also below
        // mpd's 20s `MIN_TOTAL_TIME` floor either way — mirrors
        // `CrossFadeSettings::CanCrossFadeSong` (src/player/CrossFade.cxx).
        let samples_per_second = 44100u64 * 2;
        let total_samples = 3 * samples_per_second;
        let cf_window = 5 * samples_per_second;
        assert!(!PlaybackEngine::can_cross_fade_song(
            total_samples,
            cf_window,
            samples_per_second
        ));
    }

    #[test]
    fn crossfade_track_above_min_time_but_shorter_than_window_is_not_eligible() {
        // 15s track (over no minimum by itself) with a 20s crossfade window:
        // the window would cover the entire song, which mpd also refuses
        // (`duration < total_time` must hold strictly).
        let samples_per_second = 44100u64 * 2;
        let total_samples = 25 * samples_per_second;
        let cf_window = 25 * samples_per_second;
        assert!(!PlaybackEngine::can_cross_fade_song(
            total_samples,
            cf_window,
            samples_per_second
        ));
    }
    // ── playback failure reporting / play-time accounting ────────────────────

    #[test]
    fn strip_uri_auth_removes_credentials_only_from_the_authority() {
        assert_eq!(
            strip_uri_auth("http://user:secret@radio.example/stream?x=a@b"),
            "http://radio.example/stream?x=a@b"
        );
        assert_eq!(
            strip_uri_auth("https://tok@host:8000/a"),
            "https://host:8000/a"
        );
        // Nothing to strip: plain paths, URLs without userinfo, and an `@`
        // that only appears in the path.
        assert_eq!(
            strip_uri_auth("Artist/Album/01 a@b.flac"),
            "Artist/Album/01 a@b.flac"
        );
        assert_eq!(strip_uri_auth("http://host/path@x"), "http://host/path@x");
    }

    #[test]
    fn decoder_failure_message_follows_mpd_decoder_thread_format() {
        // src/decoder/Thread.cxx: `Failed to decode {:?}` nested over the cause,
        // flattened by GetFullMessage with ": ".
        let failure = PlaybackFailure::Decoder(RmpdError::Player(
            "Failed to open file: No such file or directory (os error 2)".to_owned(),
        ));
        let (message, output) = failure.message("Artist/01.flac");
        assert_eq!(
            message,
            "Failed to decode \"Artist/01.flac\": Failed to open file: No such file or directory (os error 2)"
        );
        assert!(!output);

        // Credentials never reach the client.
        let (message, _) = failure.message("http://u:p@host/live.mp3");
        assert!(
            message.starts_with("Failed to decode \"http://host/live.mp3\": "),
            "{message}"
        );
    }

    #[test]
    fn output_failure_message_is_the_bare_cause_and_flagged_as_output() {
        let failure = PlaybackFailure::Output(RmpdError::Player(
            "Failed to open \"Default Output\" (cpal): no device".to_owned(),
        ));
        let (message, output) = failure.message("x.flac");
        assert_eq!(
            message,
            "Failed to open \"Default Output\" (cpal): no device"
        );
        assert!(output);
    }

    #[test]
    fn add_play_time_counts_audio_duration_not_sample_counts() {
        let counter = AtomicU64::new(0);
        // 44.1 kHz stereo: 88_200 interleaved samples per second.
        PlaybackEngine::add_play_time(&counter, 88_200, 88_200);
        PlaybackEngine::add_play_time(&counter, 44_100, 88_200);
        assert_eq!(counter.load(Ordering::Relaxed), 1_500_000_000);
        // A zero rate (nothing decoded yet) must not divide by zero.
        PlaybackEngine::add_play_time(&counter, 10, 0);
        assert_eq!(counter.load(Ordering::Relaxed), 1_500_000_000);
    }

    /// Write `seconds` of 8 kHz mono 16-bit silence as a WAV file.
    fn write_silent_wav(path: &std::path::Path, seconds: u32) {
        let sample_rate: u32 = 8000;
        let data_len = sample_rate * seconds * 2;
        let mut buf = Vec::with_capacity(44 + data_len as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_len).to_le_bytes());
        buf.extend_from_slice(b"WAVEfmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        buf.extend_from_slice(&2u16.to_le_bytes()); // block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_len.to_le_bytes());
        buf.resize(44 + data_len as usize, 0);
        std::fs::write(path, buf).unwrap();
    }

    fn test_engine(output_type: &str) -> (PlaybackEngine, EventBus, Arc<AtomicU8>) {
        let bus = EventBus::new();
        let status = Arc::new(RwLock::new(rmpd_core::state::PlayerStatus::default()));
        let atomic_state = Arc::new(AtomicU8::new(PlayerState::Stop as u8));
        let mut engine = PlaybackEngine::new(bus.clone(), status, atomic_state.clone());
        engine.set_outputs(vec![OutputConfig {
            output_type: output_type.to_owned(),
            ..OutputConfig::cpal_default()
        }]);
        (engine, bus, atomic_state)
    }

    fn playback_song(uri: &str, file: &std::path::Path) -> rmpd_core::playback::PlaybackSong {
        let mut song = rmpd_core::test_utils::create_test_song(1, "x");
        song.path = uri.into();
        rmpd_core::playback::PlaybackSong {
            song: Arc::new(song),
            resolved_path: file.to_str().unwrap().into(),
            range: None,
        }
    }

    /// Wait for the first event `pick` accepts (5 s cap).
    async fn wait_event<T>(
        rx: &mut tokio::sync::broadcast::Receiver<Event>,
        pick: impl Fn(&Event) -> Option<T>,
    ) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let event = rx.recv().await.expect("event bus closed");
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
        })
        .await
        .expect("timed out waiting for an engine event")
    }

    #[tokio::test]
    async fn undecodable_song_reports_a_decoder_playback_error() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.wav");
        std::fs::write(&bad, b"this is not audio").unwrap();

        let (mut engine, bus, atomic_state) = test_engine("null");
        let mut rx = bus.subscribe();
        engine.play(playback_song("bad.wav", &bad)).await.unwrap();

        let (message, output) = wait_event(&mut rx, |e| match e {
            Event::PlaybackError {
                message, output, ..
            } => Some((message.clone(), *output)),
            _ => None,
        })
        .await;
        assert!(
            message.starts_with("Failed to decode \"bad.wav\": "),
            "unexpected message: {message}"
        );
        assert!(!output, "a decoder failure is not an output error");
        assert_eq!(
            atomic_state.load(Ordering::Acquire),
            PlayerState::Stop as u8,
            "a failed song must not leave the player reporting play"
        );
    }

    #[tokio::test]
    async fn unopenable_output_reports_an_output_playback_error() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("ok.wav");
        write_silent_wav(&wav, 1);

        let (mut engine, bus, _) = test_engine("no-such-output-plugin");
        let mut rx = bus.subscribe();
        engine.play(playback_song("ok.wav", &wav)).await.unwrap();

        let (message, output) = wait_event(&mut rx, |e| match e {
            Event::PlaybackError {
                message, output, ..
            } => Some((message.clone(), *output)),
            _ => None,
        })
        .await;
        // MPD (`Filtered::Open`): Failed to open "<name>" (<plugin>): <cause>
        assert!(
            message.starts_with("Failed to open \"Default Output\" (no-such-output-plugin): "),
            "unexpected message: {message}"
        );
        assert!(output, "an output failure must be flagged as such");
    }

    #[tokio::test]
    async fn seek_reports_the_decoders_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("long.wav");
        write_silent_wav(&wav, 60);

        let (mut engine, _bus, _) = test_engine("null");
        // No song yet: nothing to seek in.
        assert!(matches!(
            engine.seek(1.0).await,
            Err(RmpdError::InvalidState(_))
        ));

        engine.play(playback_song("long.wav", &wav)).await.unwrap();
        // A real seek succeeds, an invalid one comes back as the decoder's error.
        engine.seek(2.0).await.expect("valid seek");
        match engine.seek(-1.0).await {
            Err(RmpdError::Player(msg)) => assert_eq!(msg, "Invalid seek position"),
            other => panic!("expected the decoder's error, got {other:?}"),
        }
        // Past the end clamps to the end of the song (MPD `SeekDecoder`).
        engine.seek(1.0e9).await.expect("out-of-range seek clamps");
        engine.stop().await.unwrap();
    }

    #[tokio::test]
    async fn seek_after_the_decoder_failed_reports_that_failure() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.wav");
        std::fs::write(&bad, b"this is not audio").unwrap();

        let (mut engine, bus, _) = test_engine("null");
        let mut rx = bus.subscribe();
        engine.play(playback_song("bad.wav", &bad)).await.unwrap();
        // Whether the seek lands before or after the decode thread died, the
        // client must learn why: the failure, not a generic "cannot send".
        let err = engine.seek(5.0).await.expect_err("seek on a dead song");
        wait_event(&mut rx, |e| {
            matches!(e, Event::PlaybackError { .. }).then_some(())
        })
        .await;
        match err {
            RmpdError::Player(msg) => {
                assert!(msg.starts_with("Failed to decode \"bad.wav\": "), "{msg}")
            }
            other => panic!("expected the decode failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn total_play_time_counts_samples_played_and_ignores_pauses() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("long.wav");
        write_silent_wav(&wav, 60);

        let (mut engine, _bus, _) = test_engine("null");
        assert_eq!(engine.total_play_time(), std::time::Duration::ZERO);

        engine.play(playback_song("long.wav", &wav)).await.unwrap();
        let mut playing = engine.total_play_time();
        for _ in 0..100 {
            if playing > std::time::Duration::ZERO {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            playing = engine.total_play_time();
        }
        assert!(
            playing > std::time::Duration::ZERO,
            "audio handed to the output must count"
        );

        // While paused the decode thread writes nothing. It only notices the
        // pause once the write it is blocked in (backpressure from the
        // real-time paced output, up to one 0.512 s chunk) returns, so settle
        // first: poll at an interval longer than one chunk until it holds.
        engine.pause().await.unwrap();
        let mut paused = engine.total_play_time();
        let settle_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            let now = engine.total_play_time();
            if now == paused {
                break;
            }
            assert!(
                std::time::Instant::now() < settle_deadline,
                "play time never settled after the pause ({paused:?} -> {now:?})"
            );
            paused = now;
        }
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        assert_eq!(
            engine.total_play_time(),
            paused,
            "time spent paused must not count as played"
        );

        // Stopping keeps the accumulated total (never reset, MPD semantics).
        engine.stop().await.unwrap();
        assert!(engine.total_play_time() >= playing);
    }

    #[tokio::test]
    async fn total_play_time_of_a_finished_song_is_its_duration() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("one.wav");
        write_silent_wav(&wav, 1);

        let (mut engine, bus, _) = test_engine("null");
        let mut rx = bus.subscribe();
        engine.play(playback_song("one.wav", &wav)).await.unwrap();
        wait_event(&mut rx, |e| matches!(e, Event::SongFinished).then_some(())).await;

        let played = engine.total_play_time();
        assert!(
            (played.as_secs_f64() - 1.0).abs() < 0.001,
            "a 1 s song must account for 1 s of play time, got {played:?}"
        );
    }
    /// A song torn down by `stop` while its open is still in flight must not
    /// report a playback error afterwards: that stale `PlaybackError` would
    /// stop or advance whatever the user did next.
    #[cfg(unix)]
    #[tokio::test]
    async fn song_aborted_by_stop_never_reports_a_playback_error() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("blocked.wav");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );

        let (mut engine, bus, _) = test_engine("null");
        let mut rx = bus.subscribe();
        let before = engine.generation();
        engine
            .play(playback_song("blocked.wav", &fifo))
            .await
            .unwrap();
        assert_ne!(engine.generation(), before, "play starts a new generation");
        let started = engine.generation();

        // The decode thread now sits in `open(2)` on the FIFO, which has no
        // writer. Give a writer only after `stop` has raised the stop flag:
        // the open then "succeeds" with an empty stream and the song fails
        // to probe — a failure that happens strictly after the abort.
        let writer_fifo = fifo.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            drop(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(writer_fifo)
                    .unwrap(),
            );
        });
        engine.stop().await.unwrap(); // joins the decode thread
        writer.join().unwrap();

        assert_ne!(engine.generation(), started, "stop ends the generation");
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(event, Event::PlaybackError { .. }),
                "an aborted song must not report a playback error: {event:?}"
            );
        }
    }

    #[tokio::test]
    async fn playback_error_carries_the_generation_of_its_play() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.wav");
        std::fs::write(&bad, b"this is not audio").unwrap();

        let (mut engine, bus, _) = test_engine("null");
        let mut rx = bus.subscribe();
        engine.play(playback_song("bad.wav", &bad)).await.unwrap();
        let expected = engine.generation();

        let generation = wait_event(&mut rx, |e| match e {
            Event::PlaybackError { generation, .. } => Some(*generation),
            _ => None,
        })
        .await;
        assert_eq!(generation, expected);
        // Nothing has happened since, so the engine still agrees…
        assert_eq!(engine.generation(), generation);
        // …and a later play makes that report stale.
        engine.play(playback_song("bad.wav", &bad)).await.unwrap();
        assert_ne!(engine.generation(), generation);
    }

    #[tokio::test]
    async fn begin_seek_hands_back_the_verdict_without_borrowing_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("long.wav");
        write_silent_wav(&wav, 60);

        let (mut engine, _bus, _) = test_engine("null");
        assert!(matches!(
            engine.begin_seek(1.0),
            Err(RmpdError::InvalidState(_))
        ));

        engine.play(playback_song("long.wav", &wav)).await.unwrap();
        // The pending seek owns everything it needs and borrows nothing from
        // the engine: the engine stays usable (here: mutably) while the
        // verdict is still outstanding, which is what lets the command
        // handlers release the engine lock before waiting.
        let pending = engine.begin_seek(3.0).expect("seek queued");
        let bad = engine.begin_seek(-1.0).expect("seek queued");
        engine.set_volume(50).await.unwrap();
        pending.verdict().await.expect("valid seek");
        match bad.verdict().await {
            Err(RmpdError::Player(msg)) => assert_eq!(msg, "Invalid seek position"),
            other => panic!("expected the decoder's error, got {other:?}"),
        }
    }

    fn sample_fixture(name: &str) -> Option<std::path::PathBuf> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/samples")
            .join(name);
        path.exists().then_some(path)
    }

    /// `seekcur 99999` ends the song like MPD, whatever the demuxer does with
    /// a seek to exactly the end of the stream: ogg reports it out-of-range,
    /// flac runs into the end of the file, wav/mp3/m4a accept it.
    #[tokio::test]
    async fn seek_past_the_end_ends_the_song_for_every_format() {
        for name in [
            "sine_1khz.ogg",
            "sine_440hz.flac",
            "sine_1khz.m4a",
            "sine_1khz.mp3",
            "sine_1khz.wav",
        ] {
            let Some(file) = sample_fixture(name) else {
                eprintln!("Skipping {name}: fixture not found");
                continue;
            };
            let (mut engine, bus, _) = test_engine("null");
            let mut rx = bus.subscribe();
            engine.play(playback_song(name, &file)).await.unwrap();

            engine
                .seek(1.0e9)
                .await
                .unwrap_or_else(|e| panic!("{name}: an out-of-range seek must not fail: {e}"));
            wait_event(&mut rx, |e| match e {
                Event::SongFinished => Some(true),
                Event::PlaybackError { message, .. } => {
                    panic!("{name}: seeking to the end must not fail playback: {message}")
                }
                _ => None,
            })
            .await;
        }
    }
}
