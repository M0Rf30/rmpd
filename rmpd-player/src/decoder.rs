use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::path::Path;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

/// Symphonia-based audio decoder
pub struct SymphoniaDecoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u8,
    total_duration: Option<f64>,
    current_frame: u64,
    sample_buf: Option<symphonia::core::audio::SampleBuffer<f32>>,
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

        // Get audio format info
        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| RmpdError::Player("Sample rate not available".to_string()))?;

        let channels = codec_params
            .channels
            .ok_or_else(|| RmpdError::Player("Channel info not available".to_string()))?
            .count() as u8;

        // Calculate total duration
        let total_duration = if let (Some(n_frames), Some(tb)) = (codec_params.n_frames, codec_params.time_base) {
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
            sample_rate,
            channels,
            total_duration,
            current_frame: 0,
            sample_buf: None,
        })
    }

    pub fn read(&mut self, buffer: &mut [f32]) -> Result<usize> {
        let mut samples_written = 0;

        while samples_written < buffer.len() {
            // If we have samples in the buffer, copy them
            if let Some(ref sample_buf) = self.sample_buf {
                let samples_available = sample_buf.len() - (self.current_frame as usize);
                let samples_to_copy = (buffer.len() - samples_written).min(samples_available);

                if samples_to_copy > 0 {
                    let src_offset = self.current_frame as usize;
                    buffer[samples_written..samples_written + samples_to_copy]
                        .copy_from_slice(&sample_buf.samples()[src_offset..src_offset + samples_to_copy]);

                    samples_written += samples_to_copy;
                    self.current_frame += samples_to_copy as u64;
                }

                // If buffer is exhausted, clear it
                if self.current_frame >= sample_buf.len() as u64 {
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
                    // End of stream
                    break;
                }
                Err(SymphoniaError::ResetRequired) => {
                    // Decoder needs reset
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    return Err(RmpdError::Player(format!("Failed to read packet: {}", e)));
                }
            };

            // Skip packets from other tracks
            if packet.track_id() != self.track_id {
                continue;
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
            channels: self.channels,
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
        self.channels
    }
}

/// Trait for audio decoders
pub trait Decoder {
    fn read(&mut self, buffer: &mut [f32]) -> Result<usize>;
    fn seek(&mut self, position: f64) -> Result<()>;
    fn format(&self) -> AudioFormat;
    fn duration(&self) -> Option<f64>;
}
