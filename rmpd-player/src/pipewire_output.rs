//! Native PipeWire audio output backend.
//!
//! Unlike the cpal backend (which goes through PipeWire's ALSA/JACK emulation
//! and is pinned to whatever rate the graph happens to expose), this is a real
//! `pipewire-rs` client: the stream is opened at the *decoded* format's sample
//! rate and channel count, and PipeWire owns the graph rate. That lets
//! DSD-to-PCM and hi-res PCM play at their proper rate — PipeWire follows and
//! resamples as needed — without rmpd force-downsampling to 48 kHz or
//! double-resampling. The `dsd_target_rate` hint is therefore intentionally
//! ignored for this backend.
//!
//! ## Thread model
//!
//! `MainLoop`, `Context`, `Core`, `Stream` and the stream listener are all
//! `!Send`, so they are created, used, and dropped on a single dedicated loop
//! thread spawned in [`PipeWireOutput::start`]. The struct itself stays `Send`
//! (it crosses into the engine as `Box<dyn AudioOutput>`) by holding only the
//! cross-thread handles:
//!
//! * PCM frames flow over a bounded [`SyncSender<Vec<f32>>`]; the matching
//!   `Receiver` is wrapped in [`crate::conversion::SampleBuffer`] on the loop
//!   thread (it yields `0.0` silence on underrun, exactly like the cpal path).
//! * Termination is signalled over a [`pipewire::channel`] whose `Receiver` is
//!   attached to the loop and calls `MainLoop::quit`.
//! * Startup success/failure is reported back over a one-shot
//!   [`std::sync::mpsc`] channel so `start()` can surface connection errors.

use crate::audio_output::{AudioOutput, PauseState};
use crate::conversion::SampleBuffer;
use pipewire as pw;
use pw::properties::properties;
use rmpd_core::config::OutputConfig;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;

/// Bytes per interleaved F32LE sample.
const SIZE_F32: usize = std::mem::size_of::<f32>();

/// A native PipeWire playback client.
///
/// Construct with [`PipeWireOutput::new`]; the stream is created lazily by
/// [`PipeWireOutput::start`] and torn down by [`PipeWireOutput::stop`] (also on
/// `Drop`).
pub struct PipeWireOutput {
    /// Decoded audio format the stream is opened at (F32LE, PipeWire resamples).
    format: AudioFormat,
    /// PipeWire node name advertised to the graph (`cfg.name`, or `"rmpd"`).
    node_name: String,
    /// Requested output buffer time; sizes the PCM sync-channel depth.
    buffer_time_ms: u32,
    pause_state: PauseState,

    // Runtime handles, populated by `start()` and cleared by `stop()`.
    /// Sends decoded PCM chunks to the loop thread's `SampleBuffer`.
    sample_sender: Option<SyncSender<Vec<f32>>>,
    /// Asks the loop thread to quit its `MainLoop`.
    terminate: Option<pw::channel::Sender<()>>,
    /// Handle to the spawned PipeWire loop thread.
    loop_thread: Option<JoinHandle<()>>,
}

impl PipeWireOutput {
    /// Create an output for `format`, advertising `cfg.name` (or `"rmpd"`) as
    /// the PipeWire node name. No PipeWire objects are created until `start()`.
    pub fn new(format: AudioFormat, cfg: &OutputConfig, buffer_time_ms: u32) -> Result<Self> {
        let node_name = if cfg.name.trim().is_empty() {
            "rmpd".to_owned()
        } else {
            cfg.name.clone()
        };
        Ok(Self {
            format,
            node_name,
            buffer_time_ms,
            pause_state: PauseState::new(),
            sample_sender: None,
            terminate: None,
            loop_thread: None,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.loop_thread.is_some() {
            return Ok(());
        }

        let sample_rate = self.format.sample_rate;
        let channel_count = self.format.channels as usize;
        let stride = channel_count * SIZE_F32;

        // PCM channel sized from buffer_time_ms, matching CpalOutput::start.
        let depth = channel_depth(self.buffer_time_ms, sample_rate, channel_count);
        let (tx, rx) = sync_channel::<Vec<f32>>(depth);

        // Terminate signal (Send) kept here; Receiver moves into the loop thread.
        let (term_tx, term_rx) = pw::channel::channel::<()>();
        // One-shot startup result so we can report connection failures.
        let (startup_tx, startup_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();

        let node_name = self.node_name.clone();

        let handle = std::thread::Builder::new()
            .name("rmpd-pipewire".to_owned())
            .spawn(move || {
                // Build every PipeWire object on this thread; on any failure,
                // report it over the startup channel and bail out before run().
                macro_rules! bail {
                    ($e:expr, $ctx:literal) => {
                        match $e {
                            Ok(v) => v,
                            Err(err) => {
                                let _ = startup_tx.send(Err(format!("{}: {}", $ctx, err)));
                                return;
                            }
                        }
                    };
                }

                pw::init();
                let mainloop =
                    bail!(pw::main_loop::MainLoopRc::new(None), "create pipewire main loop");
                let context = bail!(
                    pw::context::ContextRc::new(&mainloop, None),
                    "create pipewire context"
                );
                let core = bail!(context.connect_rc(None), "connect to pipewire daemon");

                let stream = bail!(
                    pw::stream::StreamBox::new(
                        &core,
                        &node_name,
                        properties! {
                            *pw::keys::MEDIA_TYPE => "Audio",
                            *pw::keys::MEDIA_CATEGORY => "Playback",
                            *pw::keys::MEDIA_ROLE => "Music",
                            *pw::keys::NODE_NAME => node_name.as_str(),
                        },
                    ),
                    "create pipewire stream"
                );

                // The process callback consumes the channel via SampleBuffer.
                // `channel_count`/`stride` are copied into the closure; the RT
                // callback stays allocation- and lock-free (next_sample() is a
                // non-blocking try_recv that returns 0.0 silence on underrun).
                let _listener = bail!(
                    stream
                        .add_local_listener_with_user_data(SampleBuffer::new(rx))
                        .process(move |stream, samples| {
                            let Some(mut buffer) = stream.dequeue_buffer() else {
                                return;
                            };
                            let datas = buffer.datas_mut();
                            if datas.is_empty() {
                                return;
                            }
                            let data = &mut datas[0];
                            // Fill the whole mapped buffer; PipeWire honors
                            // chunk.size. (`requested` is gated behind the
                            // v0_3_49 feature and not available here.)
                            let n_frames = if let Some(slice) = data.data() {
                                let n_frames = slice.len() / stride;
                                for frame in 0..n_frames {
                                    let base = frame * stride;
                                    for ch in 0..channel_count {
                                        let s = samples.next_sample();
                                        let off = base + ch * SIZE_F32;
                                        slice[off..off + SIZE_F32]
                                            .copy_from_slice(&s.to_le_bytes());
                                    }
                                }
                                n_frames
                            } else {
                                0
                            };
                            let chunk = data.chunk_mut();
                            *chunk.offset_mut() = 0;
                            *chunk.stride_mut() = stride as i32;
                            *chunk.size_mut() = (n_frames * stride) as u32;
                        })
                        .register(),
                    "register pipewire process callback"
                );

                // Build the F32LE EnumFormat POD at the decoded rate/channels
                // and connect. Scoped so the serialized buffer is freed before
                // we enter the (long-lived) main loop.
                {
                    let mut audio_info = pw::spa::param::audio::AudioInfoRaw::new();
                    audio_info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
                    audio_info.set_rate(sample_rate);
                    audio_info.set_channels(channel_count as u32);

                    let object = pw::spa::pod::Object {
                        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
                        properties: audio_info.into(),
                    };
                    let values = match pw::spa::pod::serialize::PodSerializer::serialize(
                        std::io::Cursor::new(Vec::new()),
                        &pw::spa::pod::Value::Object(object),
                    ) {
                        Ok((cursor, _)) => cursor.into_inner(),
                        Err(err) => {
                            let _ = startup_tx
                                .send(Err(format!("serialize pipewire format pod: {err:?}")));
                            return;
                        }
                    };
                    let Some(pod) = pw::spa::pod::Pod::from_bytes(&values) else {
                        let _ = startup_tx.send(Err("build pipewire format pod failed".to_owned()));
                        return;
                    };
                    let mut params = [pod];

                    bail!(
                        stream.connect(
                            pw::spa::utils::Direction::Output,
                            None,
                            pw::stream::StreamFlags::AUTOCONNECT
                                | pw::stream::StreamFlags::MAP_BUFFERS
                                | pw::stream::StreamFlags::RT_PROCESS,
                            &mut params,
                        ),
                        "connect pipewire stream"
                    );
                }

                // Quit the loop when the engine stops/drops this output.
                let _term = term_rx.attach(mainloop.loop_(), {
                    let mainloop = mainloop.clone();
                    move |_| mainloop.quit()
                });

                // Stream is connected and the listener is live: signal success.
                let _ = startup_tx.send(Ok(()));

                mainloop.run();
            })
            .map_err(|e| RmpdError::Player(format!("failed to spawn pipewire loop thread: {e}")))?;

        // Block until the loop thread reports startup status.
        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => {
                let _ = handle.join();
                return Err(RmpdError::Player(msg));
            }
            Err(_) => {
                let _ = handle.join();
                return Err(RmpdError::Player(
                    "pipewire loop thread exited before startup".to_owned(),
                ));
            }
        }

        self.sample_sender = Some(tx);
        self.terminate = Some(term_tx);
        self.loop_thread = Some(handle);
        self.pause_state.set_paused(false);

        tracing::info!(
            "pipewire output started: {} Hz, {} channels, node \"{}\"",
            sample_rate,
            self.format.channels,
            self.node_name
        );

        Ok(())
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.pause_state.is_paused() {
            return Ok(());
        }
        match &self.sample_sender {
            Some(sender) => sender
                .send(samples.to_vec())
                .map_err(|_| RmpdError::Player("pipewire output gone".to_owned())),
            None => Err(RmpdError::Player("pipewire output not started".to_owned())),
        }
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(term) = self.terminate.take() {
            // Best-effort: the loop may already be gone.
            let _ = term.send(());
        }
        // Drop the sender so the SampleBuffer sees a disconnected channel.
        self.sample_sender = None;
        if let Some(handle) = self.loop_thread.take() {
            let _ = handle.join();
        }
        self.pause_state.set_paused(false);
        Ok(())
    }

    pub fn is_paused(&self) -> bool {
        self.pause_state.is_paused()
    }
}

/// Compute the PCM sync-channel depth from the requested buffer time, matching
/// [`crate::output::CpalOutput`]. Each chunk the engine sends holds ~4096
/// interleaved samples; we size the channel to hold `buffer_time_ms` worth of
/// audio and clamp to a minimum of 4 so the realtime callback never starves on
/// a cold start. A `buffer_time_ms` of 0 falls back to a safe default depth.
fn channel_depth(buffer_time_ms: u32, sample_rate: u32, channels: usize) -> usize {
    const SAMPLES_PER_CHUNK: u64 = 4096;
    if buffer_time_ms == 0 {
        return 32;
    }
    let samples_needed = buffer_time_ms as u64 * sample_rate as u64 * channels as u64 / 1000;
    samples_needed.div_ceil(SAMPLES_PER_CHUNK).max(4) as usize
}

impl Drop for PipeWireOutput {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl AudioOutput for PipeWireOutput {
    fn start(&mut self) -> Result<()> {
        PipeWireOutput::start(self)
    }
    fn write(&mut self, samples: &[f32]) -> Result<()> {
        PipeWireOutput::write(self, samples)
    }
    fn stop(&mut self) -> Result<()> {
        PipeWireOutput::stop(self)
    }
    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }
    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_depth_scales_with_buffer_time() {
        // 500 ms @ 48 kHz stereo = 48000 samples; 48000 / 4096 -> ceil = 12.
        assert_eq!(channel_depth(500, 48_000, 2), 12);
        // A larger buffer / higher rate scales up proportionally.
        // 1000 ms @ 96 kHz stereo = 192000 samples; / 4096 -> ceil = 47.
        assert_eq!(channel_depth(1_000, 96_000, 2), 47);
    }

    #[test]
    fn channel_depth_zero_uses_safe_default() {
        assert_eq!(channel_depth(0, 48_000, 2), 32);
    }

    #[test]
    fn channel_depth_clamps_to_minimum() {
        // Tiny buffer would round to 1 chunk, but we never go below 4.
        assert_eq!(channel_depth(1, 8_000, 1), 4);
    }

    #[test]
    #[ignore = "requires a running PipeWire server"]
    fn start_write_stop_roundtrip() {
        let format = AudioFormat::new(48_000, 2, 32);
        let cfg = OutputConfig::cpal_default();
        let mut out = PipeWireOutput::new(format, &cfg, 500).expect("construct");
        out.start().expect("start connects to the PipeWire daemon");
        out.write(&[0.0_f32; 4096]).expect("write");
        out.stop().expect("stop");
    }
}
