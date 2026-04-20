//! Shared sample format conversion utilities for audio output backends.

use std::sync::mpsc::Receiver;

/// Convert f32 samples to s16le bytes, writing into the provided buffer.
/// The buffer is cleared and filled with the converted bytes.
pub fn samples_to_s16le_into(samples: &[f32], buf: &mut Vec<u8>) {
    buf.clear();
    buf.reserve(samples.len() * 2);
    for &s in samples {
        let v = f32_to_i16(s);
        buf.extend_from_slice(&v.to_le_bytes());
    }
}

/// Convert interleaved f32 PCM samples (range −1.0…+1.0) to little-endian
/// signed 16-bit bytes.
pub fn samples_to_s16le(samples: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(samples.len() * 2);
    samples_to_s16le_into(samples, &mut buf);
    buf
}

/// Clamp and scale a single f32 sample to `i16` range.
#[inline]
pub fn f32_to_i16(val: f32) -> i16 {
    (val.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Clamp and scale a single f32 sample to `i32` range.
#[inline]
pub fn f32_to_i32(val: f32) -> i32 {
    (val.clamp(-1.0, 1.0) * i32::MAX as f32) as i32
}

/// A bounded sample buffer fed from a `SyncSender`/`Receiver` channel.
///
/// Used inside cpal output callbacks to decouple the decoder thread from the
/// real-time audio thread.  When the current buffer is exhausted the next
/// chunk is pulled from the channel; if no data is available the buffer
/// produces silence (the `Default` value for `T`).
pub struct SampleBuffer<T> {
    rx: Receiver<Vec<T>>,
    buffer: Vec<T>,
    pos: usize,
}

impl<T: Default + Copy> SampleBuffer<T> {
    /// Create a new buffer backed by the receiving end of a `sync_channel`.
    pub fn new(rx: Receiver<Vec<T>>) -> Self {
        Self {
            rx,
            buffer: Vec::new(),
            pos: 0,
        }
    }

    /// Return the next sample, refilling from the channel when the current
    /// chunk is exhausted.  Returns `T::default()` (silence) on underrun.
    #[inline]
    pub fn next_sample(&mut self) -> T {
        if self.pos >= self.buffer.len() && let Ok(new_samples) = self.rx.try_recv() {
            self.buffer = new_samples;
            self.pos = 0;
        }
        if self.pos < self.buffer.len() {
            let val = self.buffer[self.pos];
            self.pos += 1;
            val
        } else {
            T::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn samples_to_s16le_clamps() {
        let samples = [0.0_f32, 1.0, -1.0, 1.5, -1.5];
        let bytes = samples_to_s16le(&samples);
        assert_eq!(bytes.len(), 10);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 0);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), -i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[6], bytes[7]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[8], bytes[9]]), -i16::MAX);
    }

    #[test]
    fn samples_to_s16le_into_reusable() {
        let samples = [0.0_f32, 1.0, -1.0, 1.5, -1.5];
        let mut buf = Vec::new();

        // First call
        samples_to_s16le_into(&samples, &mut buf);
        assert_eq!(buf.len(), 10);
        assert_eq!(i16::from_le_bytes([buf[0], buf[1]]), 0);
        assert_eq!(i16::from_le_bytes([buf[2], buf[3]]), i16::MAX);

        // Second call should reuse and clear the buffer
        let samples2 = [0.5_f32, -0.5];
        samples_to_s16le_into(&samples2, &mut buf);
        assert_eq!(buf.len(), 4);
        assert!(buf.capacity() >= 4);
    }

    #[test]
    fn f32_to_i16_basic() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert_eq!(f32_to_i16(-1.0), -i16::MAX);
    }

    #[test]
    fn f32_to_i32_basic() {
        assert_eq!(f32_to_i32(0.0), 0);
        assert!((f32_to_i32(1.0) - i32::MAX).unsigned_abs() < 256);
        assert!((f32_to_i32(-1.0) + i32::MAX).unsigned_abs() < 256);
    }

    #[test]
    fn sample_buffer_refill() {
        let (tx, rx) = sync_channel::<Vec<f32>>(2);
        let mut buf = SampleBuffer::new(rx);

        tx.send(vec![1.0, 2.0]).unwrap();
        tx.send(vec![3.0]).unwrap();

        assert_eq!(buf.next_sample(), 1.0);
        assert_eq!(buf.next_sample(), 2.0);
        assert_eq!(buf.next_sample(), 3.0);
    }

    #[test]
    fn sample_buffer_underrun_returns_silence() {
        let (_tx, rx) = sync_channel::<Vec<f32>>(1);
        let mut buf = SampleBuffer::new(rx);

        assert_eq!(buf.next_sample(), 0.0);
    }

    #[test]
    fn sample_buffer_i32_silence() {
        let (_tx, rx) = sync_channel::<Vec<i32>>(1);
        let mut buf: SampleBuffer<i32> = SampleBuffer::new(rx);

        assert_eq!(buf.next_sample(), 0);
    }
}
