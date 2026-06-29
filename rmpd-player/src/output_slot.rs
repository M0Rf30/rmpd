//! Persistent output cache for gapless playback.
//!
//! The decode thread `acquire`s a [`MultiOutput`] for the current track's
//! `(sample_rate, channels, bits, output-signature)` key. Consecutive tracks
//! with the SAME key reuse the cached `MultiOutput` — its audio device stays
//! open across the track boundary, so there is no device close/reopen gap
//! (the biggest source of inter-track gaps). A key change (different format or
//! a changed output set) tears the old one down and builds a fresh one.
//!
//! True sample-accurate gapless additionally needs decode look-ahead (opening
//! the next decoder before EOS); that is a separate, larger change. This module
//! delivers the device-persistence half, which removes the audible pop/gap of
//! reopening the sound device between same-format tracks.

use crate::multi_output::MultiOutput;
use parking_lot::Mutex;
use rmpd_core::error::Result;
use std::sync::Arc;

/// Identifies an output configuration for reuse. Two tracks share a cached
/// output only if their keys are equal.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OutputKey {
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    /// Stable description of the enabled output set (type + name per output).
    pub signature: Vec<String>,
}

struct Cached {
    key: OutputKey,
    multi: Arc<MultiOutput>,
}

/// Caches one live [`MultiOutput`] for reuse across same-key tracks.
#[derive(Default)]
pub struct OutputSlot {
    inner: Mutex<Option<Cached>>,
}

impl OutputSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached output if it matches `key`; otherwise tear down any
    /// existing one and build a fresh output via `build`, caching it.
    ///
    /// `build` (which opens audio devices) runs ONLY on a cache miss, so a
    /// same-key consecutive track keeps the device open — the gapless win.
    pub fn acquire(
        &self,
        key: OutputKey,
        build: impl FnOnce() -> Result<Arc<MultiOutput>>,
    ) -> Result<Arc<MultiOutput>> {
        let mut guard = self.inner.lock();
        if let Some(cached) = guard.as_ref()
            && cached.key == key
        {
            return Ok(cached.multi.clone());
        }
        // Miss: drop the old output first (its `Drop` joins the workers and
        // closes the device) so the new device opens cleanly, then build.
        *guard = None;
        let multi = build()?;
        *guard = Some(Cached {
            key,
            multi: multi.clone(),
        });
        Ok(multi)
    }

    /// Tear down the cached output. The device closes once the last user (e.g.
    /// the decode thread) also drops its handle.
    pub fn clear(&self) {
        *self.inner.lock() = None;
    }

    #[cfg(test)]
    fn is_cached(&self) -> bool {
        self.inner.lock().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::null_output::NullOutput;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

    fn build_null() -> Result<Arc<MultiOutput>> {
        let volume = Arc::new(AtomicU8::new(100));
        Ok(Arc::new(MultiOutput::spawn(
            vec![Box::new(NullOutput::new())],
            4,
            volume,
        )?))
    }

    fn key(sample_rate: u32, sig: &str) -> OutputKey {
        OutputKey {
            sample_rate,
            channels: 2,
            bits_per_sample: 16,
            signature: vec![sig.to_owned()],
        }
    }

    #[test]
    fn reuses_cached_output_for_same_key() {
        let slot = OutputSlot::new();
        let builds = AtomicUsize::new(0);
        let a = slot
            .acquire(key(44100, "null|Out"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        let b = slot
            .acquire(key(44100, "null|Out"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "same key must reuse the cached output (gapless: no device reopen)"
        );
        assert!(Arc::ptr_eq(&a, &b), "the same MultiOutput must be returned");
    }

    #[test]
    fn rebuilds_on_format_change() {
        let slot = OutputSlot::new();
        let builds = AtomicUsize::new(0);
        let _ = slot
            .acquire(key(44100, "null|Out"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        let _ = slot
            .acquire(key(96000, "null|Out"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        assert_eq!(
            builds.load(Ordering::SeqCst),
            2,
            "a different format must rebuild the output"
        );
    }

    #[test]
    fn rebuilds_on_output_set_change() {
        let slot = OutputSlot::new();
        let builds = AtomicUsize::new(0);
        let _ = slot
            .acquire(key(44100, "null|A"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        let _ = slot
            .acquire(key(44100, "null|B"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        assert_eq!(
            builds.load(Ordering::SeqCst),
            2,
            "a changed output signature must rebuild"
        );
    }

    #[test]
    fn clear_then_acquire_rebuilds() {
        let slot = OutputSlot::new();
        let builds = AtomicUsize::new(0);
        let _ = slot
            .acquire(key(44100, "null|Out"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        slot.clear();
        assert!(!slot.is_cached(), "clear must drop the cached output");
        let _ = slot
            .acquire(key(44100, "null|Out"), || {
                builds.fetch_add(1, Ordering::SeqCst);
                build_null()
            })
            .unwrap();
        assert_eq!(
            builds.load(Ordering::SeqCst),
            2,
            "clear (e.g. on stop or DoP) must force a rebuild"
        );
    }
}
