use crate::audio_output::AudioOutput;
use crate::decoder::SymphoniaDecoder;
use crate::dop::DopEncoder;
use crate::dop_output::DopOutput;
use crate::output::CpalOutput;
use rmpd_core::config::{DopMode, OutputConfig, ReplayGainMode, ResamplerQuality};
use rmpd_core::error::Result;
use rmpd_core::event::{Event, EventBus};
use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

const BUFFER_SIZE: usize = 4096;

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

/// Commands that can be sent to the playback thread
enum PlaybackCommand {
    Seek(f64),
}

/// Main playback engine
pub struct PlaybackEngine {
    status: Arc<RwLock<rmpd_core::state::PlayerStatus>>,
    event_bus: EventBus,
    stop_flag: Arc<AtomicBool>,
    atomic_state: Arc<AtomicU8>, // For lock-free state checking in playback thread
    playback_thread: Option<thread::JoinHandle<()>>,
    current_song: Arc<RwLock<Option<Song>>>,
    volume: Arc<AtomicU8>,
    command_tx: Option<mpsc::Sender<PlaybackCommand>>,
    outputs: Vec<OutputConfig>,
    replay_gain_mode: ReplayGainMode,
    replay_gain_preamp: f32,
    replay_gain_missing_preamp: f32,
    volume_normalization: bool,
    resampler_quality: ResamplerQuality,
    dop_mode: DopMode,
}

impl PlaybackEngine {
    pub fn new(
        event_bus: EventBus,
        status: Arc<RwLock<rmpd_core::state::PlayerStatus>>,
        atomic_state: Arc<AtomicU8>,
    ) -> Self {
        Self {
            status,
            event_bus,
            stop_flag: Arc::new(AtomicBool::new(false)),
            atomic_state,
            playback_thread: None,
            current_song: Arc::new(RwLock::new(None)),
            volume: Arc::new(AtomicU8::new(100)),
            command_tx: None,
            outputs: vec![OutputConfig::cpal_default()],
            replay_gain_mode: ReplayGainMode::default(),
            replay_gain_preamp: 0.0,
            replay_gain_missing_preamp: 0.0,
            volume_normalization: false,
            resampler_quality: ResamplerQuality::default(),
            dop_mode: DopMode::default(),
        }
    }

    pub fn set_outputs(&mut self, outputs: Vec<OutputConfig>) {
        self.outputs = outputs;
    }

    pub fn set_replay_gain(&mut self, mode: ReplayGainMode, preamp: f32, missing_preamp: f32) {
        self.replay_gain_mode = mode;
        self.replay_gain_preamp = preamp;
        self.replay_gain_missing_preamp = missing_preamp;
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

    pub async fn seek(&self, position: f64) -> Result<()> {
        if let Some(ref tx) = self.command_tx {
            tx.send(PlaybackCommand::Seek(position)).map_err(|_| {
                rmpd_core::error::RmpdError::Player("Failed to send seek command".to_owned())
            })?;
            Ok(())
        } else {
            Err(rmpd_core::error::RmpdError::Player(
                "No active playback".to_owned(),
            ))
        }
    }

    pub async fn play(&mut self, playback_song: rmpd_core::playback::PlaybackSong) -> Result<()> {
        info!("starting playback: {}", playback_song.resolved_path);

        // Stop current playback if any (internal stop, no events - caller will emit)
        self.stop_internal().await?;

        // Update current song - clone the song from Arc
        *self.current_song.write().await = Some((*playback_song.song).clone());

        // Reset stop flag
        self.stop_flag.store(false, Ordering::Release);

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel();
        self.command_tx = Some(command_tx);

        // Spawn playback thread
        let song_path = playback_song.resolved_path.clone();
        let event_bus = self.event_bus.clone();
        let stop_flag = self.stop_flag.clone();
        let volume = self.volume.clone();
        let status_clone = self.status.clone();
        let atomic_state_clone = self.atomic_state.clone();
        let outputs = self.outputs.clone();
        let gain_scale = Self::compute_gain_scale(
            &playback_song.song,
            self.replay_gain_mode,
            self.replay_gain_preamp,
            self.replay_gain_missing_preamp,
            self.volume_normalization,
        );
        let resampler_quality = self.resampler_quality;
        let dop_mode = self.dop_mode;

        let handle = thread::spawn(move || {
            if let Err(e) = Self::playback_thread(
                song_path.as_std_path(),
                status_clone,
                atomic_state_clone,
                event_bus,
                stop_flag,
                volume,
                command_rx,
                outputs,
                resampler_quality,
                dop_mode,
                gain_scale,
            ) {
                error!("playback error: {}", e);
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
        }
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        debug!("stopping playback");
        self.stop_internal().await?;
        // Emit event to notify clients (external stop)
        self.event_bus.emit(Event::SongChanged(None));
        Ok(())
    }

    /// Internal stop - doesn't emit events (used when stopping before playing next song)
    async fn stop_internal(&mut self) -> Result<()> {
        debug!("internal stop (no events)");

        // Set stop flag
        self.stop_flag.store(true, Ordering::Release);

        // Clear command channel
        self.command_tx = None;

        // Wait for playback thread to finish
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
        }

        // Update atomic state (caller must update status to avoid deadlock)
        self.atomic_state
            .store(PlayerState::Stop as u8, Ordering::Release);
        *self.current_song.write().await = None;

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
        self.current_song.read().await.clone()
    }

    pub async fn set_volume(&mut self, vol: u8) -> Result<()> {
        self.volume.store(vol, Ordering::Release);
        self.event_bus.emit(Event::VolumeChanged(vol));
        Ok(())
    }

    pub async fn get_volume(&self) -> u8 {
        self.volume.load(Ordering::Acquire)
    }

    fn playback_thread(
        path: &Path,
        _status: Arc<RwLock<rmpd_core::state::PlayerStatus>>,
        atomic_state: Arc<AtomicU8>,
        event_bus: EventBus,
        stop_flag: Arc<AtomicBool>,
        volume: Arc<AtomicU8>,
        command_rx: mpsc::Receiver<PlaybackCommand>,
        outputs: Vec<rmpd_core::config::OutputConfig>,
        resampler_quality: ResamplerQuality,
        dop_mode: DopMode,
        gain_scale: f32,
    ) -> Result<()> {
        // Open decoder (pass-through mode by default)
        let mut decoder = SymphoniaDecoder::open(path)?;

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
                match Self::setup_dop(&decoder) {
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
            info!(
                "DSD-to-PCM conversion enabled at {} Hz (device {} Hz)",
                decode_rate, device_rate
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

        let mut boxes: Vec<Box<dyn AudioOutput>> = Vec::with_capacity(effective_outputs.len());
        for (i, cfg) in effective_outputs.iter().enumerate() {
            match Self::create_output(format, cfg, resampler_quality) {
                Ok(b) => boxes.push(b),
                Err(e) => {
                    if i == 0 {
                        return Err(e);
                    }
                    warn!(
                        "secondary output '{}' failed to create: {}; skipping",
                        cfg.name, e
                    );
                }
            }
        }

        let multi = crate::multi_output::MultiOutput::spawn(boxes, 16, volume.clone())?;

        // Playback loop
        let mut buffer = vec![0.0f32; BUFFER_SIZE];
        let mut total_samples_played: u64 = 0;
        let samples_per_second = format.sample_rate as u64 * format.channels as u64;
        // Track whether we have sent pause/resume to the workers to avoid
        // spamming the same message every 100 ms.
        let mut multi_paused = false;

        while !stop_flag.load(Ordering::Acquire) {
            // Check for commands (non-blocking)
            if let Ok(cmd) = command_rx.try_recv() {
                match cmd {
                    PlaybackCommand::Seek(position) => {
                        debug!("seeking to position: {:.2}s", position);
                        if let Err(e) = decoder.seek(position) {
                            error!("seek failed: {}", e);
                        } else {
                            // Reset sample counter after seek
                            total_samples_played = (position * samples_per_second as f64) as u64;
                            // Emit position change event
                            event_bus.emit(Event::PositionChanged(
                                std::time::Duration::from_secs_f64(position),
                            ));
                        }
                    }
                }
            }

            // Check if paused — read from atomic state (no locks needed)
            let current_state = PlayerState::from_atomic(atomic_state.load(Ordering::Acquire));

            if current_state == PlayerState::Pause {
                if !multi_paused {
                    multi.pause();
                    multi_paused = true;
                }
                thread::sleep(StdDuration::from_millis(100));
                continue;
            } else if multi_paused {
                multi.resume();
                multi_paused = false;
            }

            // Read from decoder
            let samples_read = decoder.read(&mut buffer)?;

            if samples_read == 0 {
                // End of stream
                debug!(
                    "End of stream reached, total samples decoded: {}",
                    total_samples_played
                );
                event_bus.emit(Event::SongFinished);
                break;
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
                break;
            }

            // Update elapsed time
            total_samples_played += samples_read as u64;

            // Emit position update event every ~1 second of audio (throttled)
            if total_samples_played % samples_per_second < (samples_read as u64) {
                let elapsed_seconds = total_samples_played as f64 / samples_per_second as f64;
                event_bus.emit(Event::PositionChanged(std::time::Duration::from_secs_f64(
                    elapsed_seconds,
                )));

                // Also emit current bitrate (for VBR files this changes during playback)
                let current_bitrate = decoder.current_bitrate();
                event_bus.emit(Event::BitrateChanged(current_bitrate));
            }
        }

        multi.stop();

        Ok(())
    }

    fn create_output(
        format: rmpd_core::song::AudioFormat,
        cfg: &OutputConfig,
        quality: ResamplerQuality,
    ) -> Result<Box<dyn AudioOutput>> {
        crate::output_registry::create_output(format, quality, cfg)
    }

    fn compute_gain_scale(
        song: &Song,
        mode: ReplayGainMode,
        preamp: f32,
        missing_preamp: f32,
        normalization: bool,
    ) -> f32 {
        if mode == ReplayGainMode::Off {
            return 1.0;
        }
        let (gain_opt, peak_opt) = match mode {
            ReplayGainMode::Off => unreachable!(),
            ReplayGainMode::Track => (song.replay_gain_track_gain, song.replay_gain_track_peak),
            ReplayGainMode::Album => (song.replay_gain_album_gain, song.replay_gain_album_peak),
            ReplayGainMode::Auto => {
                if song.replay_gain_album_gain.is_some() {
                    (song.replay_gain_album_gain, song.replay_gain_album_peak)
                } else {
                    (song.replay_gain_track_gain, song.replay_gain_track_peak)
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

    /// Build the DoP encoder and start the DoP output for `decoder`. Building and
    /// starting the stream here means any failure (configured device can't do the
    /// DoP rate, device busy, no DoP DAC) surfaces as an error so the caller can
    /// cleanly revert to PCM instead of aborting playback.
    fn setup_dop(decoder: &SymphoniaDecoder) -> Result<(DopEncoder, DopOutput)> {
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

        let mut output = DopOutput::new(pcm_sample_rate, channels)?;
        output.start()?;

        Ok((dop_encoder, output))
    }

    /// DSD playback loop over an already-started DoP output.
    fn run_dsd_dop(
        mut decoder: SymphoniaDecoder,
        mut dop_encoder: DopEncoder,
        mut output: DopOutput,
        atomic_state: Arc<AtomicU8>,
        event_bus: EventBus,
        stop_flag: Arc<AtomicBool>,
        command_rx: mpsc::Receiver<PlaybackCommand>,
    ) -> Result<()> {
        let dsd_sample_rate = decoder.sample_rate();
        let channels = decoder.channels();

        let mut dsd_buffer = Vec::new();
        let mut dop_i32_buffer = Vec::new();
        let mut total_dsd_bytes: u64 = 0;
        let dsd_bytes_per_second = (dsd_sample_rate / 8) as u64 * channels as u64;

        while !stop_flag.load(Ordering::Acquire) {
            // Check for commands
            if let Ok(cmd) = command_rx.try_recv() {
                match cmd {
                    PlaybackCommand::Seek(position) => {
                        debug!("seeking to position: {:.2}s", position);
                        if let Err(e) = decoder.seek(position) {
                            error!("seek failed: {}", e);
                        } else {
                            total_dsd_bytes = (position * dsd_bytes_per_second as f64) as u64;
                            event_bus.emit(Event::PositionChanged(
                                std::time::Duration::from_secs_f64(position),
                            ));
                        }
                    }
                }
            }

            // Check if paused
            let current_state = PlayerState::from_atomic(atomic_state.load(Ordering::Acquire));

            if current_state == PlayerState::Pause {
                output.pause()?;
                thread::sleep(StdDuration::from_millis(100));
                continue;
            } else if current_state == PlayerState::Play && output.is_paused() {
                output.resume()?;
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
            output.write(&dop_i32_buffer)?;

            // Update elapsed time
            total_dsd_bytes += bytes_read as u64;

            // Emit position update every ~1 second
            if total_dsd_bytes % dsd_bytes_per_second < (bytes_read as u64) {
                let elapsed_seconds = total_dsd_bytes as f64 / dsd_bytes_per_second as f64;
                event_bus.emit(Event::PositionChanged(std::time::Duration::from_secs_f64(
                    elapsed_seconds,
                )));

                let current_bitrate = decoder.current_bitrate();
                event_bus.emit(Event::BitrateChanged(current_bitrate));
            }
        }

        output.stop()?;

        Ok(())
    }
}

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
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
}
