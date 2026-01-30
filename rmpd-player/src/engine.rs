use crate::decoder::SymphoniaDecoder;
use crate::output::CpalOutput;
use crate::dop::DopEncoder;
use crate::dop_output::DopOutput;
use rmpd_core::error::Result;
use rmpd_core::event::{Event, EventBus};
use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

const BUFFER_SIZE: usize = 4096;

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
    volume: Arc<RwLock<u8>>,
    command_tx: Option<mpsc::Sender<PlaybackCommand>>,
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
            volume: Arc::new(RwLock::new(100)),
            command_tx: None,
        }
    }

    pub async fn seek(&self, position: f64) -> Result<()> {
        if let Some(ref tx) = self.command_tx {
            tx.send(PlaybackCommand::Seek(position))
                .map_err(|_| rmpd_core::error::RmpdError::Player("Failed to send seek command".to_string()))?;
            Ok(())
        } else {
            Err(rmpd_core::error::RmpdError::Player("No active playback".to_string()))
        }
    }

    pub async fn play(&mut self, song: Song) -> Result<()> {
        info!("Starting playback: {}", song.path);

        // Stop current playback if any (internal stop, no events - caller will emit)
        self.stop_internal().await?;

        // Update current song
        *self.current_song.write().await = Some(song.clone());

        // Reset stop flag
        self.stop_flag.store(false, Ordering::SeqCst);

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel();
        self.command_tx = Some(command_tx);

        // Spawn playback thread
        let song_path = song.path.clone();
        let event_bus = self.event_bus.clone();
        let stop_flag = self.stop_flag.clone();
        let volume = self.volume.clone();
        let status_clone = self.status.clone();
        let atomic_state_clone = self.atomic_state.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = Self::playback_thread(song_path.as_std_path(), status_clone, atomic_state_clone, event_bus, stop_flag, volume, command_rx) {
                error!("Playback error: {}", e);
            }
        });

        self.playback_thread = Some(handle);

        // Update atomic state (caller must update status to avoid deadlock and emit events)
        self.atomic_state.store(PlayerState::Play as u8, Ordering::SeqCst);

        Ok(())
    }

    pub async fn pause(&mut self) -> Result<()> {
        // Toggle atomic state - caller must update status to avoid deadlock
        let current = self.atomic_state.load(Ordering::SeqCst);
        let new_state = match current {
            1 => PlayerState::Pause as u8, // Play -> Pause
            2 => PlayerState::Play as u8,   // Pause -> Play
            _ => return Ok(()),             // Stop -> do nothing
        };
        self.atomic_state.store(new_state, Ordering::SeqCst);
        Ok(())
    }

    /// Set pause state explicitly (doesn't toggle)
    pub async fn set_pause(&mut self, should_pause: bool) -> Result<()> {
        let current = self.atomic_state.load(Ordering::SeqCst);

        // Only transition if we're playing or paused (not stopped)
        if current == PlayerState::Play as u8 || current == PlayerState::Pause as u8 {
            let new_state = if should_pause {
                PlayerState::Pause as u8
            } else {
                PlayerState::Play as u8
            };
            self.atomic_state.store(new_state, Ordering::SeqCst);
        }
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        debug!("Stopping playback");
        self.stop_internal().await?;
        // Emit event to notify clients (external stop)
        self.event_bus.emit(Event::SongChanged(None));
        Ok(())
    }

    /// Internal stop - doesn't emit events (used when stopping before playing next song)
    async fn stop_internal(&mut self) -> Result<()> {
        debug!("Internal stop (no events)");

        // Set stop flag
        self.stop_flag.store(true, Ordering::SeqCst);

        // Clear command channel
        self.command_tx = None;

        // Wait for playback thread to finish
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
        }

        // Update atomic state (caller must update status to avoid deadlock)
        self.atomic_state.store(PlayerState::Stop as u8, Ordering::SeqCst);
        *self.current_song.write().await = None;

        Ok(())
    }

    pub async fn get_state(&self) -> PlayerState {
        let status = self.status.read().await;
        status.state
    }

    /// Get current state without locks (atomic, lock-free)
    pub fn get_state_atomic(&self) -> PlayerState {
        match self.atomic_state.load(Ordering::SeqCst) {
            0 => PlayerState::Stop,
            1 => PlayerState::Play,
            2 => PlayerState::Pause,
            _ => PlayerState::Stop,
        }
    }

    pub async fn get_current_song(&self) -> Option<Song> {
        self.current_song.read().await.clone()
    }

    pub async fn set_volume(&mut self, vol: u8) -> Result<()> {
        *self.volume.write().await = vol;
        self.event_bus.emit(Event::VolumeChanged(vol));
        Ok(())
    }

    pub async fn get_volume(&self) -> u8 {
        *self.volume.read().await
    }

    fn playback_thread(
        path: &Path,
        _status: Arc<RwLock<rmpd_core::state::PlayerStatus>>,
        atomic_state: Arc<AtomicU8>,
        event_bus: EventBus,
        stop_flag: Arc<AtomicBool>,
        volume: Arc<RwLock<u8>>,
        command_rx: mpsc::Receiver<PlaybackCommand>,
    ) -> Result<()> {
        // Open decoder
        let mut decoder = SymphoniaDecoder::open(path)?;

        // Check if this is a DSD file
        let is_dsd = decoder.is_dsd();

        if is_dsd {
            // Handle DSD playback with DoP encoding
            return Self::playback_thread_dsd(decoder, atomic_state, event_bus, stop_flag, volume, command_rx);
        }

        // Standard PCM playback
        let format = decoder.format();

        debug!(
            "Decoder opened: {}Hz, {} channels",
            format.sample_rate, format.channels
        );

        // Create output
        let mut output = CpalOutput::new(format)?;
        output.start()?;

        debug!("Output started");

        // Playback loop
        let mut buffer = vec![0.0f32; BUFFER_SIZE];
        let mut total_samples_played: u64 = 0;
        let samples_per_second = format.sample_rate as u64 * format.channels as u64;

        while !stop_flag.load(Ordering::SeqCst) {
            // Check for commands (non-blocking)
            if let Ok(cmd) = command_rx.try_recv() {
                match cmd {
                    PlaybackCommand::Seek(position) => {
                        debug!("Seeking to position: {:.2}s", position);
                        if let Err(e) = decoder.seek(position) {
                            error!("Seek failed: {}", e);
                        } else {
                            // Reset sample counter after seek
                            total_samples_played = (position * samples_per_second as f64) as u64;
                            // Emit position change event
                            event_bus.emit(Event::PositionChanged(
                                std::time::Duration::from_secs_f64(position)
                            ));
                        }
                    }
                }
            }

            // Check if paused - read from atomic state (no locks needed)
            let state_value = atomic_state.load(Ordering::SeqCst);
            let current_state = match state_value {
                0 => PlayerState::Stop,
                1 => PlayerState::Play,
                2 => PlayerState::Pause,
                _ => PlayerState::Stop,
            };

            if current_state == PlayerState::Pause {
                output.pause()?;
                thread::sleep(StdDuration::from_millis(100));
                continue;
            } else if current_state == PlayerState::Play && output.is_paused() {
                output.resume()?;
            }

            // Read from decoder
            let samples_read = decoder.read(&mut buffer)?;

            if samples_read == 0 {
                // End of stream
                debug!("End of stream reached, total samples decoded: {}", total_samples_played);
                event_bus.emit(Event::SongFinished);
                break;
            }

            if samples_read < buffer.len() {
                debug!("Partial read: {} samples (buffer size: {})", samples_read, buffer.len());
            }

            // Apply volume - read and release lock immediately
            let volume_factor = {
                let vol = futures::executor::block_on(volume.read());
                (*vol as f32) / 100.0
            }; // Lock released here
            for sample in buffer[..samples_read].iter_mut() {
                *sample *= volume_factor;
            }

            // Write to output
            output.write(&buffer[..samples_read])?;

            // Update elapsed time
            total_samples_played += samples_read as u64;

            // Emit position update event every ~1 second of audio (throttled)
            if total_samples_played % samples_per_second < (samples_read as u64) {
                let elapsed_seconds = total_samples_played as f64 / samples_per_second as f64;
                event_bus.emit(Event::PositionChanged(
                    std::time::Duration::from_secs_f64(elapsed_seconds)
                ));

                // Also emit current bitrate (for VBR files this changes during playback)
                let current_bitrate = decoder.current_bitrate();
                event_bus.emit(Event::BitrateChanged(current_bitrate));
            }
        }

        // Stop output
        output.stop()?;

        debug!("Playback thread finished");
        Ok(())
    }

    /// Playback thread for DSD files with DoP encoding
    fn playback_thread_dsd(
        mut decoder: SymphoniaDecoder,
        atomic_state: Arc<AtomicU8>,
        event_bus: EventBus,
        stop_flag: Arc<AtomicBool>,
        _volume: Arc<RwLock<u8>>,  // Unused: DoP volume controlled by system mixer
        command_rx: mpsc::Receiver<PlaybackCommand>,
    ) -> Result<()> {
        let dsd_sample_rate = decoder.sample_rate();
        let channels = decoder.channels();

        // Get DSD metadata for proper encoding
        let channel_layout = decoder.channel_data_layout()
            .unwrap_or(symphonia::core::codecs::ChannelDataLayout::Planar);
        let bit_order = decoder.bit_order()
            .unwrap_or(symphonia::core::codecs::BitOrder::LsbFirst);

        info!("DSD playback: {} Hz, {} channels", dsd_sample_rate, channels);
        info!("DSD format: channel_layout={:?}, bit_order={:?}", channel_layout, bit_order);

        // Create DoP encoder
        let mut dop_encoder = DopEncoder::new(dsd_sample_rate, channels as usize, channel_layout, bit_order)?;
        let pcm_sample_rate = dop_encoder.pcm_sample_rate();

        info!("DoP encoding: DSD {} Hz -> PCM {} Hz", dsd_sample_rate, pcm_sample_rate);

        info!("Creating DoP output with {} Hz", pcm_sample_rate);

        // Create DoP output (uses integer samples to preserve marker precision)
        let mut output = match DopOutput::new(pcm_sample_rate, channels) {
            Ok(output) => {
                info!("DoP output created successfully");
                output
            }
            Err(e) => {
                error!("Failed to create DoP output: {}", e);
                return Err(e);
            }
        };

        info!("Starting DoP output...");
        output.start()?;

        info!("DoP output started successfully");

        // Playback loop
        let mut dsd_buffer = Vec::new();
        let mut dop_i32_buffer = Vec::new();
        let mut total_dsd_bytes: u64 = 0;
        let dsd_bytes_per_second = (dsd_sample_rate / 8) as u64 * channels as u64;

        info!("Entering DSD playback loop");

        while !stop_flag.load(Ordering::SeqCst) {
            // Log first iteration
            if total_dsd_bytes == 0 {
                info!("First iteration of DSD playback loop");
            }
            // Check for commands
            if let Ok(cmd) = command_rx.try_recv() {
                match cmd {
                    PlaybackCommand::Seek(position) => {
                        debug!("Seeking to position: {:.2}s", position);
                        if let Err(e) = decoder.seek(position) {
                            error!("Seek failed: {}", e);
                        } else {
                            total_dsd_bytes = (position * dsd_bytes_per_second as f64) as u64;
                            event_bus.emit(Event::PositionChanged(
                                std::time::Duration::from_secs_f64(position)
                            ));
                        }
                    }
                }
            }

            // Check if paused
            let state_value = atomic_state.load(Ordering::SeqCst);
            let current_state = match state_value {
                0 => PlayerState::Stop,
                1 => PlayerState::Play,
                2 => PlayerState::Pause,
                _ => PlayerState::Stop,
            };

            if current_state == PlayerState::Pause {
                output.pause()?;
                thread::sleep(StdDuration::from_millis(100));
                continue;
            } else if current_state == PlayerState::Play && output.is_paused() {
                output.resume()?;
            }

            // Read raw DSD data
            if total_dsd_bytes == 0 {
                info!("Reading first DSD packet...");
            }
            let bytes_read = decoder.read_dsd_raw(&mut dsd_buffer)?;
            if total_dsd_bytes == 0 {
                info!("Read {} bytes of DSD data", bytes_read);
            }

            if bytes_read == 0 {
                info!("End of DSD stream reached");
                event_bus.emit(Event::SongFinished);
                break;
            }

            // Encode to DoP
            if total_dsd_bytes == 0 {
                info!("Encoding {} bytes to DoP...", dsd_buffer.len());
            }
            dop_encoder.encode(&dsd_buffer, &mut dop_i32_buffer);
            if total_dsd_bytes == 0 {
                info!("Encoded to {} DoP samples", dop_i32_buffer.len());
                // Log first few samples to verify DoP markers
                if dop_i32_buffer.len() >= 4 {
                    info!("First DoP samples (left-aligned): 0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}",
                          dop_i32_buffer[0], dop_i32_buffer[1], dop_i32_buffer[2], dop_i32_buffer[3]);
                    info!("Marker bytes (MSB): 0x{:02x}, 0x{:02x}, 0x{:02x}, 0x{:02x}",
                          (dop_i32_buffer[0] >> 24) & 0xFF,
                          (dop_i32_buffer[1] >> 24) & 0xFF,
                          (dop_i32_buffer[2] >> 24) & 0xFF,
                          (dop_i32_buffer[3] >> 24) & 0xFF);
                }
            }

            // Write i32 DoP samples directly to preserve marker precision
            // Note: Volume control for DoP is handled by the system mixer
            if total_dsd_bytes == 0 {
                info!("Writing {} i32 DoP samples to output...", dop_i32_buffer.len());
            }
            match output.write(&dop_i32_buffer) {
                Ok(_) => {
                    if total_dsd_bytes == 0 {
                        info!("First DoP write complete");
                    }
                }
                Err(e) => {
                    error!("Failed to write DoP samples: {}", e);
                    return Err(e);
                }
            }

            // Update elapsed time
            total_dsd_bytes += bytes_read as u64;

            // Log every few seconds
            if total_dsd_bytes % (dsd_bytes_per_second * 5) < (bytes_read as u64) {
                info!("DSD playback progress: {:.1}s, {} bytes processed",
                      total_dsd_bytes as f64 / dsd_bytes_per_second as f64,
                      total_dsd_bytes);
            }

            // Emit position update every ~1 second
            if total_dsd_bytes % dsd_bytes_per_second < (bytes_read as u64) {
                let elapsed_seconds = total_dsd_bytes as f64 / dsd_bytes_per_second as f64;
                event_bus.emit(Event::PositionChanged(
                    std::time::Duration::from_secs_f64(elapsed_seconds)
                ));

                // DSD has fixed bitrate
                let bitrate_kbps = (dsd_sample_rate / 1000) * channels as u32;
                event_bus.emit(Event::BitrateChanged(Some(bitrate_kbps)));
            }
        }

        info!("DSD playback loop exited, total bytes: {}", total_dsd_bytes);

        // Stop output
        output.stop()?;

        debug!("DSD playback thread finished");
        Ok(())
    }
}

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
        }
    }
}
