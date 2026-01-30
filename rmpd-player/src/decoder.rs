use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::path::Path;
use symphonia::core::codecs::{BitOrder, ChannelDataLayout, CodecType, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};
// DSD codec type (from Symphonia with DSD support)
use symphonia::default::formats::CODEC_TYPE_DSD;

/// Symphonia-based audio decoder
pub struct SymphoniaDecoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    codec_type: CodecType,
    sample_rate: u32,
    channels: Option<u8>,
    total_duration: Option<f64>,
    current_frame: u64,
    sample_buf: Option<symphonia::core::audio::SampleBuffer<f32>>,
    current_bitrate: Option<u32>,
    time_base: Option<TimeBase>,
    channel_data_layout: Option<ChannelDataLayout>,
    bit_order: Option<BitOrder>,
}

impl SymphoniaDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        // Open the media source
        let file = std::fs::File::open(path)
            .map_err(|e| RmpdError::Player(format!("Failed to open file: {}", e)))?;

        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        // Create a hint to help the format registry guess the format
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        // Probe the media source
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| RmpdError::Player(format!("Failed to probe format: {}", e)))?;

        let reader = probed.format;

        // Find the first audio track
        let track = reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| RmpdError::Player("No audio tracks found".to_string()))?;

        let track_id = track.id;
        let codec_params = &track.codec_params;

        // Store codec type for DSD detection
        let codec_type = codec_params.codec;

        // Get audio format info
        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| RmpdError::Player("Sample rate not available".to_string()))?;

        // Channels might not be available until after decoding starts
        let channels = codec_params
            .channels
            .map(|ch| ch.count() as u8);

        // Store time base for bitrate calculation
        let time_base = codec_params.time_base;

        // Get DSD metadata if available
        let channel_data_layout = codec_params.channel_data_layout;
        let bit_order = codec_params.bit_order;

        // Calculate total duration
        let total_duration = if let (Some(n_frames), Some(tb)) = (codec_params.n_frames, time_base) {
            let time = tb.calc_time(n_frames);
            Some(time.seconds as f64 + time.frac)
        } else {
            None
        };

        // Create decoder
        let decoder = symphonia::default::get_codecs()
            .make(codec_params, &DecoderOptions::default())
            .map_err(|e| RmpdError::Player(format!("Failed to create decoder: {}", e)))?;

        Ok(Self {
            reader,
            decoder,
            track_id,
            codec_type,
            sample_rate,
            channels,
            total_duration,
            current_frame: 0,
            sample_buf: None,
            current_bitrate: None,
            time_base,
            channel_data_layout,
            bit_order,
        })
    }

    /// Check if this is a DSD file
    pub fn is_dsd(&self) -> bool {
        self.codec_type == CODEC_TYPE_DSD
    }

    pub fn read(&mut self, buffer: &mut [f32]) -> Result<usize> {
        let mut samples_written = 0;

        while samples_written < buffer.len() {
            // If we have samples in the buffer, copy them
            if let Some(ref sample_buf) = self.sample_buf {
                // sample_buf.samples() returns interleaved samples
                // sample_buf.len() returns number of frames
                // For stereo, samples().len() == len() * 2
                let total_samples = sample_buf.samples().len();
                let samples_available = total_samples - (self.current_frame as usize);
                let samples_to_copy = (buffer.len() - samples_written).min(samples_available);

                if samples_to_copy > 0 {
                    let src_offset = self.current_frame as usize;
                    buffer[samples_written..samples_written + samples_to_copy]
                        .copy_from_slice(&sample_buf.samples()[src_offset..src_offset + samples_to_copy]);

                    samples_written += samples_to_copy;
                    self.current_frame += samples_to_copy as u64;
                }

                // If buffer is exhausted, clear it
                if self.current_frame >= total_samples as u64 {
                    self.sample_buf = None;
                    self.current_frame = 0;
                }

                if samples_written >= buffer.len() {
                    break;
                }
            }

            // Read next packet
            let packet = match self.reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Only treat "end of stream" as actual EOF (per Symphonia docs)
                    if e.to_string().contains("end of stream") {
                        break;
                    } else {
                        // Other UnexpectedEof errors - continue reading
                        continue;
                    }
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    tracing::error!("Failed to read packet: {}", e);
                    return Err(RmpdError::Player(format!("Failed to read packet: {}", e)));
                }
            };

            // Skip packets from other tracks
            if packet.track_id() != self.track_id {
                continue;
            }

            // Calculate instantaneous bitrate from packet
            if let Some(tb) = self.time_base {
                let packet_bytes = packet.buf().len();
                let packet_dur = packet.dur();

                // Convert duration from TimeBase units to seconds
                let time = tb.calc_time(packet_dur);
                let duration_secs = time.seconds as f64 + time.frac;

                if duration_secs > 0.0 {
                    // Calculate bitrate: (bytes * 8 bits/byte) / duration_secs = bits/sec
                    // Then convert to kbps
                    let bitrate_bps = (packet_bytes as f64 * 8.0) / duration_secs;
                    let bitrate_kbps = (bitrate_bps / 1000.0) as u32;
                    self.current_bitrate = Some(bitrate_kbps);
                }
            }

            // Decode packet
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) => {
                    // Skip decode errors
                    continue;
                }
                Err(e) => {
                    return Err(RmpdError::Player(format!("Failed to decode packet: {}", e)));
                }
            };

            // Convert to f32 samples
            let spec = *decoded.spec();
            let duration = decoded.capacity() as u64;

            // Update channels if not yet known
            if self.channels.is_none() {
                self.channels = Some(spec.channels.count() as u8);
            }

            let mut new_sample_buf = symphonia::core::audio::SampleBuffer::<f32>::new(duration, spec);
            new_sample_buf.copy_interleaved_ref(decoded);

            self.sample_buf = Some(new_sample_buf);
            self.current_frame = 0;
        }

        Ok(samples_written)
    }

    pub fn seek(&mut self, position: f64) -> Result<()> {
        if position < 0.0 {
            return Err(RmpdError::Player("Invalid seek position".to_string()));
        }

        let time_base = TimeBase::new(1, self.sample_rate);
        let time = Time {
            seconds: position as u64,
            frac: position.fract(),
        };

        let ts = time_base.calc_timestamp(time);

        self.reader
            .seek(symphonia::core::formats::SeekMode::Accurate, symphonia::core::formats::SeekTo::TimeStamp { ts, track_id: self.track_id })
            .map_err(|e| RmpdError::Player(format!("Seek failed: {}", e)))?;

        self.decoder.reset();
        self.sample_buf = None;
        self.current_frame = 0;

        Ok(())
    }

    pub fn format(&self) -> AudioFormat {
        AudioFormat {
            sample_rate: self.sample_rate,
            channels: self.channels.unwrap_or(2), // Default to stereo if not yet known
            bits_per_sample: 16, // Symphonia decodes to f32, we report 16-bit for MPD compatibility
        }
    }

    pub fn duration(&self) -> Option<f64> {
        self.total_duration
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u8 {
        self.channels.unwrap_or(2) // Default to stereo if not yet known
    }

    /// Get the current instantaneous bitrate in kbps (for VBR files this changes during playback)
    pub fn current_bitrate(&self) -> Option<u32> {
        self.current_bitrate
    }

    /// Get channel data layout (planar vs interleaved) for DSD files
    pub fn channel_data_layout(&self) -> Option<ChannelDataLayout> {
        self.channel_data_layout
    }

    /// Get bit order (LSB-first vs MSB-first) for DSD files
    pub fn bit_order(&self) -> Option<BitOrder> {
        self.bit_order
    }

    /// Read raw DSD data (for DoP encoding)
    /// Returns raw DSD bytes without conversion
    pub fn read_dsd_raw(&mut self, buffer: &mut Vec<u8>) -> Result<usize> {
        buffer.clear();

        // Read next packet
        let packet = match self.reader.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                if e.to_string().contains("end of stream") {
                    return Ok(0);
                } else {
                    return self.read_dsd_raw(buffer); // Try again
                }
            }
            Err(SymphoniaError::ResetRequired) => {
                self.decoder.reset();
                return self.read_dsd_raw(buffer);
            }
            Err(e) => {
                return Err(RmpdError::Player(format!("Failed to read DSD packet: {}", e)));
            }
        };

        // Skip packets from other tracks
        if packet.track_id() != self.track_id {
            return self.read_dsd_raw(buffer);
        }

        // For DSD, the packet buffer contains raw DSD data
        // Copy it directly without decoding
        buffer.extend_from_slice(packet.buf());

        Ok(buffer.len())
    }
}

/// Trait for audio decoders
pub trait Decoder {
    fn read(&mut self, buffer: &mut [f32]) -> Result<usize>;
    fn seek(&mut self, position: f64) -> Result<()>;
    fn format(&self) -> AudioFormat;
    fn duration(&self) -> Option<f64>;
}
