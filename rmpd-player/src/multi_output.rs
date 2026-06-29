//! Non-blocking fan-out to N simultaneous audio outputs.
//!
//! `MultiOutput` owns one worker thread per backend.  The PRIMARY output
//! (index 0) drives back-pressure: `write` blocks until it accepts a chunk,
//! preserving the real-time clock.  Every secondary output receives chunks
//! via `try_send`; if its channel is full the chunk is dropped so a stalled
//! secondary can never block the primary.
//!
//! Chunks are shared as `Arc<[f32]>` — a single ref-count bump per secondary,
//! no deep copies.

use crate::audio_output::AudioOutput;
use crate::filter::{AudioFilter, VolumeFilter};
use rmpd_core::error::{Result, RmpdError};
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use tracing::{debug, warn};

enum OutputMsg {
    Samples(Arc<[f32]>),
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
    ) -> Result<Self> {
        let mut workers = Vec::with_capacity(outputs.len());

        for (idx, mut out) in outputs.into_iter().enumerate() {
            let primary = idx == 0;
            let (tx, rx) = sync_channel::<OutputMsg>(depth);
            let vol_arc = volume.clone();

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
                    let mut vol = VolumeFilter::new(vol_arc);
                    loop {
                        match rx.recv() {
                            Ok(OutputMsg::Samples(arc)) => {
                                let mut buf = arc.to_vec();
                                vol.apply(&mut buf);
                                let _ = out.write(&buf);
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

        Ok(MultiOutput { workers })
    }

    /// Fan one chunk to all outputs.
    ///
    /// Blocks on the primary for back-pressure; uses `try_send` (drop-on-full)
    /// for every secondary.  Returns `Err` only if the primary worker is gone.
    pub fn write(&self, chunk: Arc<[f32]>) -> Result<()> {
        for w in &self.workers {
            if w.primary {
                w.tx.send(OutputMsg::Samples(chunk.clone()))
                    .map_err(|_| RmpdError::Player("primary output stopped".into()))?;
            } else {
                // Best-effort: silently drop on Full or Disconnected.
                let _ = w.tx.try_send(OutputMsg::Samples(chunk.clone()));
            }
        }
        Ok(())
    }

    /// Pause all outputs (best-effort, non-blocking).
    pub fn pause(&self) {
        for w in &self.workers {
            let _ = w.tx.try_send(OutputMsg::Pause);
        }
    }

    /// Resume all outputs (best-effort, non-blocking).
    pub fn resume(&self) {
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
    pub fn stop(mut self) {
        // Send Stop: blocking for primary (ensures it is received), try for
        // secondaries (their channel may be full if they are stalled).
        for w in &self.workers {
            if w.primary {
                let _ = w.tx.send(OutputMsg::Stop);
            } else {
                let _ = w.tx.try_send(OutputMsg::Stop);
            }
        }
        // Join primary; drop secondary handles (threads detach).
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

impl Drop for MultiOutput {
    fn drop(&mut self) {
        // Mirror stop() — handles may be None if stop() was already called.
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
                w.handle.take();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_output::PauseState;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

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
            Arc::new(std::sync::atomic::AtomicU8::new(100)),
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
}
