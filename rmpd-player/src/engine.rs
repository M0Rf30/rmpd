use crate::decoder::SymphoniaDecoder;
use crate::output::CpalOutput;
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

        // Stop current playback if any
        self.stop().await?;

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

        // Update atomic state (caller must update status to avoid deadlock)
        self.atomic_state.store(PlayerState::Play as u8, Ordering::SeqCst);
        self.event_bus.emit(Event::SongChanged(Some(song)));

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

    pub async fn stop(&mut self) -> Result<()> {
        debug!("Stopping playback");

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
        self.event_bus.emit(Event::SongChanged(None));

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
}

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
        }
    }
}
