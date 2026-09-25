//! Non-blocking fan-out to N simultaneous audio outputs.
//!
//! `MultiOutput` owns one worker thread per backend.  The PRIMARY output
//! (index 0) drives back-pressure: `write` blocks until it accepts a chunk,
//! preserving the real-time clock.  Every secondary output receives chunks
//! via `try_send`; if its channel is full the chunk is dropped so a stalled
//! secondary can never block the primary.
//!
//! Chunks are shared as `Arc<[f32]>` — a single ref-count bump per secondary,
//! no deep copies. Each chunk carries the [`OutputControl`] flush generation
//! it was produced under.
//!
//! ## Pause / flush responsiveness
//!
//! Every output constructed for this `MultiOutput` shares the SAME
//! `Arc<OutputControl>` as the engine, so `pause()`/`seek()`/`stop()` are
//! visible here (and in every backend's own real-time callback) the instant
//! the engine sets them — no round trip through the decode thread required.
//!
//! A backend that reports [`AudioOutput::self_managed`] owns a real-time
//! callback (cpal, PipeWire) that itself holds position on pause and drops
//! stale-generation audio (including a partially-consumed chunk); its
//! worker here just forwards every chunk unconditionally (subject to normal
//! backpressure/drop-on-full for secondaries). A backend WITHOUT its own
//! real-time callback (null/fifo/pipe/recorder/httpd) has pause-hold and
//! flush-drop applied right here: the worker skips (does not call
//! `AudioOutput::write`) a dequeued chunk while `control.is_paused()` or
//! once its generation is stale — draining the backlog in one recv-loop
//! pass rather than in real time, and still gets the legacy write-time
//! [`VolumeFilter`].

use crate::audio_output::AudioOutput;
use crate::filter::{AudioFilter, VolumeFilter};
use crate::output_control::OutputControl;
use rmpd_core::error::{Result, RmpdError};
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use tracing::{debug, warn};

enum OutputMsg {
    /// `(flush_generation, samples)`.
    Samples(u64, Arc<[f32]>),
    Pause,
    Resume,
    Stop,
}

struct Worker {
    tx: SyncSender<OutputMsg>,
    handle: Option<JoinHandle<()>>,
    primary: bool,
}

pub struct MultiOutput {
    workers: Vec<Worker>,
    /// Shared with the engine and every backend's own callback. `write()`
    /// tags outgoing chunks with the live generation.
    control: Arc<OutputControl>,
}

impl MultiOutput {
    /// Spawn one worker thread per output.
    ///
    /// `outputs[0]` is the primary (clock-bearing).  `depth` is the bounded
    /// channel capacity for each worker.  Workers call `start()` on their own
    /// thread; if the primary fails to start, the channel becomes disconnected
    /// and the first `write()` call will return `Err`.  A secondary that fails
    /// to start is logged and dropped.
    pub fn spawn(
        outputs: Vec<Box<dyn AudioOutput>>,
        depth: usize,
        volume: Arc<AtomicU8>,
        control: Arc<OutputControl>,
    ) -> Result<Self> {
        let mut workers = Vec::with_capacity(outputs.len());

        for (idx, mut out) in outputs.into_iter().enumerate() {
            let primary = idx == 0;
            let (tx, rx) = sync_channel::<OutputMsg>(depth);
            let vol_arc = volume.clone();
            let worker_control = control.clone();

            let handle = thread::Builder::new()
                .name(if primary {
                    "rmpd-primary-out".to_owned()
                } else {
                    format!("rmpd-secondary-out-{idx}")
                })
                .spawn(move || {
                    if let Err(e) = out.start() {
                        warn!(
                            "{} output worker failed to start: {}",
                            if primary { "primary" } else { "secondary" },
                            e
                        );
                        return;
                    }
                    debug!(
                        "{} output worker started",
                        if primary { "primary" } else { "secondary" }
                    );
                    let self_managed = out.self_managed();
                    let mut vol = VolumeFilter::new(vol_arc);
                    loop {
                        match rx.recv() {
                            Ok(OutputMsg::Samples(generation, arc)) => {
                                if !self_managed
                                    && (worker_control.is_paused()
                                        || generation != worker_control.generation())
                                {
                                    // Paused/stale: discard rather than play out a
                                    // chunk queued before the transition — see
                                    // module docs. Keeps the backlog drain
                                    // instantaneous instead of real-time-paced.
                                    // Self-managed backends own this decision in
                                    // their own real-time callback instead.
                                    continue;
                                }
                                let mut buf = arc.to_vec();
                                if !self_managed {
                                    vol.apply(&mut buf);
                                }
                                if let Err(e) = out.write(&buf) {
                                    // A persistent write failure (device
                                    // disconnected) must stop the worker so
                                    // the channel disconnects and `write`
                                    // starts returning `Err` — otherwise the
                                    // decode thread races through the rest
                                    // of the queue at full CPU speed instead
                                    // of real-time pace (PLAY-03).
                                    warn!(
                                        "{} output write failed, stopping worker: {}",
                                        if primary { "primary" } else { "secondary" },
                                        e
                                    );
                                    let _ = out.stop();
                                    break;
                                }
                            }
                            Ok(OutputMsg::Pause) => {
                                let _ = out.pause();
                            }
                            Ok(OutputMsg::Resume) => {
                                let _ = out.resume();
                            }
                            Ok(OutputMsg::Stop) => {
                                let _ = out.stop();
                                break;
                            }
                            Err(_) => {
                                // Sender side dropped — clean up and exit.
                                let _ = out.stop();
                                break;
                            }
                        }
                    }
                    debug!(
                        "{} output worker stopped",
                        if primary { "primary" } else { "secondary" }
                    );
                })
                .map_err(|e| RmpdError::Player(format!("failed to spawn output thread: {e}")))?;

            workers.push(Worker {
                tx,
                handle: Some(handle),
                primary,
            });
        }

        Ok(MultiOutput { workers, control })
    }

    /// Fan one chunk to all outputs, tagged with the live flush generation.
    ///
    /// Blocks on the primary for back-pressure; uses `try_send` (drop-on-full)
    /// for every secondary.  Returns `Err` only if the primary worker is gone.
    pub fn write(&self, chunk: Arc<[f32]>) -> Result<()> {
        let generation = self.control.generation();
        for w in &self.workers {
            if w.primary {
                w.tx.send(OutputMsg::Samples(generation, chunk.clone()))
                    .map_err(|_| RmpdError::Player("primary output stopped".into()))?;
            } else {
                // Best-effort: silently drop on Full or Disconnected.
                let _ = w.tx.try_send(OutputMsg::Samples(generation, chunk.clone()));
            }
        }
        Ok(())
    }

    /// The shared control block (pause / flush-generation / gain / played-frames).
    pub fn control(&self) -> &Arc<OutputControl> {
        &self.control
    }

    /// Pause all outputs: sets the shared control flag (instant, read
    /// directly by every self-managed backend's real-time callback and by
    /// non-self-managed workers above) and best-effort notifies the
    /// hardware layer (e.g. `cpal::Stream::pause`).
    pub fn pause(&self) {
        self.control.set_paused(true);
        for w in &self.workers {
            let _ = w.tx.try_send(OutputMsg::Pause);
        }
    }

    /// Resume all outputs (instant control flag + best-effort hardware call).
    pub fn resume(&self) {
        self.control.set_paused(false);
        for w in &self.workers {
            let _ = w.tx.try_send(OutputMsg::Resume);
        }
    }

    /// Send `Stop` to all workers and join cleanly.
    ///
    /// The primary is joined so the caller knows it has fully drained.
    /// Secondaries are sent `Stop` on a best-effort basis (the channel may be
    /// full if the secondary is stalled) and their threads are detached — they
    /// will exit on their own once any blocking write returns.
    pub fn stop(self) {
        // Send Stop: blocking for primary (ensures it is received), try for
        // secondaries (their channel may be full if they are stalled).
        Self::send_stop_and_join(&self.workers);
    }

    fn send_stop_and_join(workers: &[Worker]) {
        for w in workers {
            if w.primary {
                let _ = w.tx.send(OutputMsg::Stop);
            } else {
                let _ = w.tx.try_send(OutputMsg::Stop);
            }
        }
    }
}

impl Drop for MultiOutput {
    fn drop(&mut self) {
        // Mirror stop(): a flush just before this (engine-side, on seek/stop/
        // song-change) already makes self-managed callbacks silence almost
        // instantly; this still sends Stop so workers exit promptly.
        for w in &self.workers {
            if w.primary {
                let _ = w.tx.send(OutputMsg::Stop);
            } else {
                let _ = w.tx.try_send(OutputMsg::Stop);
            }
        }
        for w in &mut self.workers {
            if w.primary {
                if let Some(h) = w.handle.take() {
                    let _ = h.join();
                }
            } else {
                w.handle.take(); // detach
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_output::PauseState;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn control() -> Arc<OutputControl> {
        Arc::new(OutputControl::new())
    }

    // ── Test outputs ──────────────────────────────────────────────────────────

    struct CountingOutput {
        count: Arc<AtomicUsize>,
        state: PauseState,
    }

    impl AudioOutput for CountingOutput {
        fn start(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn write(&mut self, _samples: &[f32]) -> rmpd_core::error::Result<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn pause_state(&self) -> &PauseState {
            &self.state
        }
        fn pause_state_mut(&mut self) -> &mut PauseState {
            &mut self.state
        }
    }

    /// Records the first sample of every buffer it's asked to write, so a
    /// test can distinguish "old" from "new" generation audio by content.
    struct RecordingOutput {
        log: Arc<Mutex<Vec<f32>>>,
        state: PauseState,
    }

    impl AudioOutput for RecordingOutput {
        fn start(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn write(&mut self, samples: &[f32]) -> rmpd_core::error::Result<()> {
            if let Some(&first) = samples.first() {
                self.log.lock().push(first);
            }
            Ok(())
        }
        fn stop(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn pause_state(&self) -> &PauseState {
            &self.state
        }
        fn pause_state_mut(&mut self) -> &mut PauseState {
            &mut self.state
        }
    }

    /// An output whose `write` blocks for ~1 hour, simulating a stalled sink.
    struct BlockingOutput {
        state: PauseState,
    }

    impl AudioOutput for BlockingOutput {
        fn start(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn write(&mut self, _samples: &[f32]) -> rmpd_core::error::Result<()> {
            std::thread::sleep(Duration::from_secs(3600));
            Ok(())
        }
        fn stop(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn pause_state(&self) -> &PauseState {
            &self.state
        }
        fn pause_state_mut(&mut self) -> &mut PauseState {
            &mut self.state
        }
    }

    /// An output whose `write` takes a fixed, non-trivial time — simulates
    /// real-time playback pacing so a test can tell "played out" apart from
    /// "discarded".
    struct SlowOutput {
        count: Arc<AtomicUsize>,
        delay: Duration,
        state: PauseState,
    }

    impl AudioOutput for SlowOutput {
        fn start(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn write(&mut self, _samples: &[f32]) -> rmpd_core::error::Result<()> {
            std::thread::sleep(self.delay);
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn pause_state(&self) -> &PauseState {
            &self.state
        }
        fn pause_state_mut(&mut self) -> &mut PauseState {
            &mut self.state
        }
    }

    /// An output whose `write` always fails, simulating a disconnected
    /// device.
    struct FailingOutput {
        state: PauseState,
    }

    impl AudioOutput for FailingOutput {
        fn start(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn write(&mut self, _samples: &[f32]) -> rmpd_core::error::Result<()> {
            Err(RmpdError::Player("device disconnected".into()))
        }
        fn stop(&mut self) -> rmpd_core::error::Result<()> {
            Ok(())
        }
        fn pause_state(&self) -> &PauseState {
            &self.state
        }
        fn pause_state_mut(&mut self) -> &mut PauseState {
            &mut self.state
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// The primary output must receive every chunk even when the secondary is
    /// completely stalled.  `write` must never block on the secondary.
    #[test]
    fn fan_out_does_not_block_on_stalled_secondary() {
        let count = Arc::new(AtomicUsize::new(0));

        let primary = CountingOutput {
            count: Arc::clone(&count),
            state: PauseState::new(),
        };
        let secondary = BlockingOutput {
            state: PauseState::new(),
        };

        // depth=4: secondary's channel fills after 4 chunks; try_send drops the rest.
        let multi = MultiOutput::spawn(
            vec![Box::new(primary), Box::new(secondary)],
            4,
            Arc::new(AtomicU8::new(100)),
            control(),
        )
        .expect("spawn failed");

        // 100 writes should all succeed and complete quickly regardless of the
        // stalled secondary.
        let chunk: Arc<[f32]> = Arc::from(vec![0.0f32; 64].as_slice());
        for _ in 0..100 {
            assert!(
                multi.write(Arc::clone(&chunk)).is_ok(),
                "write must not fail"
            );
        }

        // Let the primary worker drain its channel before we join it via stop().
        std::thread::sleep(Duration::from_millis(100));

        // stop() joins the primary (not the stalled secondary) so it returns fast.
        multi.stop();

        // Primary processed exactly 100 chunks.
        assert_eq!(
            count.load(Ordering::SeqCst),
            100,
            "primary must have received all 100 chunks"
        );
    }

    /// Pausing must not wait for already-queued chunks to be *played*: it
    /// should discard them so the audible effect is near-instant instead of
    /// paced out over `depth` chunks worth of real time.
    #[test]
    fn pause_discards_queued_backlog_instead_of_playing_it_out() {
        let count = Arc::new(AtomicUsize::new(0));
        let depth = 16;

        let primary = SlowOutput {
            count: Arc::clone(&count),
            delay: Duration::from_millis(50),
            state: PauseState::new(),
        };

        let multi = MultiOutput::spawn(
            vec![Box::new(primary)],
            depth,
            Arc::new(AtomicU8::new(100)),
            control(),
        )
        .expect("spawn failed");

        // Fill the channel to capacity, then pause immediately. If pause
        // waited for the backlog to be *played*, draining `depth` chunks at
        // 50ms each would take ~800ms.
        let chunk: Arc<[f32]> = Arc::from(vec![0.0f32; 64].as_slice());
        for _ in 0..depth {
            multi
                .write(Arc::clone(&chunk))
                .expect("write must not fail");
        }
        multi.pause();

        // Give the worker enough time to drain the backlog if it's discarding
        // (near-instant) but nowhere near enough to have played it all out
        // for real (~800ms).
        std::thread::sleep(Duration::from_millis(150));

        let played = count.load(Ordering::SeqCst);
        assert!(
            played < depth,
            "pause should have discarded most of the backlog instead of playing it \
             out (played {played} of {depth} queued chunks within 150ms at 50ms/chunk)"
        );

        multi.stop();
    }

    /// A flush (generation bump) must drop chunks tagged with the OLD
    /// generation still sitting in a non-self-managed output's queue, even
    /// though it isn't paused — the same mechanism seek/stop/song-change
    /// relies on for backends with no real-time callback of their own.
    #[test]
    fn flush_drops_stale_generation_chunks_for_non_self_managed_outputs() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let ctl = control();

        let primary = RecordingOutput {
            log: Arc::clone(&log),
            state: PauseState::new(),
        };

        let multi = MultiOutput::spawn(
            vec![Box::new(primary)],
            16,
            Arc::new(AtomicU8::new(100)),
            Arc::clone(&ctl),
        )
        .expect("spawn failed");

        // Queue a batch of old-generation ("stale") chunks, each starting
        // with a distinctive 1.0 sample.
        for _ in 0..8 {
            let chunk: Arc<[f32]> = Arc::from(vec![1.0f32; 64].as_slice());
            multi.write(chunk).expect("write must not fail");
        }

        // Flush: bump the generation as the engine does before a
        // seek/stop/song-change, THEN queue fresh audio (2.0-tagged).
        ctl.flush();
        for _ in 0..2 {
            let chunk: Arc<[f32]> = Arc::from(vec![2.0f32; 64].as_slice());
            multi.write(chunk).expect("write must not fail");
        }

        multi.stop();

        let seen = log.lock();
        assert!(
            !seen.contains(&1.0),
            "stale (pre-flush) chunks must never reach the backend, saw: {seen:?}"
        );
        assert_eq!(
            seen.iter().filter(|&&s| s == 2.0).count(),
            2,
            "the fresh post-flush chunks must all reach the backend, saw: {seen:?}"
        );
    }

    /// A persistent `write()` failure (disconnected device) must stop the
    /// worker rather than loop forever discarding chunks — otherwise the
    /// decode thread races through the queue at full CPU speed instead of
    /// real-time pace (PLAY-03), with nothing pinning the contract (PLAY-11).
    #[test]
    fn persistent_write_failure_stops_the_worker() {
        let primary = FailingOutput {
            state: PauseState::new(),
        };

        let multi = MultiOutput::spawn(
            vec![Box::new(primary)],
            4,
            Arc::new(AtomicU8::new(100)),
            control(),
        )
        .expect("spawn failed");

        let chunk: Arc<[f32]> = Arc::from(vec![0.0f32; 64].as_slice());
        // First write may or may not observe the worker exit yet (channel
        // send can succeed before the worker processes it); keep writing
        // until the primary channel disconnects.
        let mut saw_err = false;
        for _ in 0..depth_iters() {
            if multi.write(Arc::clone(&chunk)).is_err() {
                saw_err = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            saw_err,
            "MultiOutput::write must start returning Err once the worker exits \
             after a persistent write failure"
        );
    }

    fn depth_iters() -> usize {
        50
    }
}
