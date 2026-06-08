use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::path::Path;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{
    AudioCodecId, AudioDecoder, AudioDecoderOptions, BitOrder, ChannelDataLayout,
};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};
// DSD codec type (from Symphonia with DSD support)
use symphonia::default::formats::CODEC_TYPE_DSD;

/// Symphonia-based audio decoder
pub struct SymphoniaDecoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    codec_id: AudioCodecId,
    sample_rate: u32,
    channels: Option<u8>,
    total_duration: Option<f64>,
    sample_buf: Vec<f32>,
    sample_pos: usize,
    current_bitrate: Option<u32>,
    time_base: Option<TimeBase>,
    channel_data_layout: Option<ChannelDataLayout>,
    bit_order: Option<BitOrder>,
    uses_pcm_conversion: bool,
}

impl SymphoniaDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        // Open the media source
        let file = std::fs::File::open(path)
            .map_err(|e| RmpdError::Player(format!("Failed to open file: {e}")))?;

        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        // Create a hint to help the format registry guess the format
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        // Probe the media source
        let reader = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| RmpdError::Player(format!("Failed to probe format: {e}")))?;

        // Find the default audio track
        let track = reader
            .default_track(TrackType::Audio)
            .ok_or_else(|| RmpdError::Player("No audio tracks found".to_owned()))?;

        let track_id = track.id;
        let time_base = track.time_base;

        // Get the audio codec parameters.
        let audio = match track.codec_params.as_ref() {
            Some(CodecParameters::Audio(audio)) => audio,
            _ => return Err(RmpdError::Player("No audio codec parameters".to_owned())),
        };

        // Store codec id for DSD detection.
        let codec_id = audio.codec;

        let sample_rate = audio
            .sample_rate
            .ok_or_else(|| RmpdError::Player("Sample rate not available".to_owned()))?;

        // Channels might not be available until after decoding starts.
        let channels = audio.channels.as_ref().map(|ch| ch.count() as u8);

        // DSD metadata if available.
        let channel_data_layout = audio.channel_data_layout;
        let bit_order = audio.bit_order;

        // Calculate total duration from the track frame count and timebase.
        let total_duration = match (track.num_frames, time_base) {
            (Some(n_frames), Some(tb)) => tb
                .calc_time(Timestamp::new(n_frames as i64))
                .map(|t| t.as_secs_f64()),
            _ => None,
        };

        // Create decoder in pass-through mode (no PCM conversion).
        // PCM conversion can be enabled later if needed.
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio, &AudioDecoderOptions::default())
            .map_err(|e| RmpdError::Player(format!("Failed to create decoder: {e}")))?;

        Ok(Self {
            reader,
            decoder,
            track_id,
            codec_id,
            sample_rate,
            channels,
            total_duration,
            sample_buf: Vec::new(),
            sample_pos: 0,
            current_bitrate: None,
            time_base,
            channel_data_layout,
            bit_order,
            uses_pcm_conversion: false,
        })
    }

    /// Check if this is a DSD file
    pub fn is_dsd(&self) -> bool {
        self.codec_id == CODEC_TYPE_DSD
    }

    /// Enable PCM conversion for DSD (can be called multiple times with different rates)
    pub fn enable_pcm_conversion(&mut self, output_rate: u32) -> Result<()> {
        if self.codec_id != CODEC_TYPE_DSD {
            return Ok(()); // Not DSD, nothing to do
        }

        // If already enabled at the same rate, nothing to do
        if self.uses_pcm_conversion && self.sample_rate == output_rate {
            return Ok(());
        }

        // Get the current track's audio codec parameters.
        let track = self
            .reader
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)
            .ok_or_else(|| RmpdError::Player("Track not found".to_owned()))?;

        let audio = match track.codec_params.as_ref() {
            Some(CodecParameters::Audio(audio)) => audio,
            _ => return Err(RmpdError::Player("No audio codec parameters".to_owned())),
        };
        let input_rate = audio
            .sample_rate
            .ok_or_else(|| RmpdError::Player("Sample rate not available".to_owned()))?;

        // Clone params and add PCM conversion mode via extra_data
        let mut params_with_pcm = audio.clone();
        params_with_pcm.extra_data = Some(output_rate.to_le_bytes().to_vec().into_boxed_slice());

        tracing::info!(
            "enabling DSD-to-PCM conversion: {} Hz DSD -> {} Hz PCM",
            input_rate,
            output_rate
        );

        // Create new decoder with PCM conversion
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params_with_pcm, &AudioDecoderOptions::default())
            .map_err(|e| RmpdError::Player(format!("Failed to create PCM decoder: {e}")))?;

        // Get actual output sample rate from decoder
        let actual_sample_rate = decoder
            .codec_params()
            .sample_rate
            .ok_or_else(|| RmpdError::Player("Decoder sample rate not available".to_owned()))?;

        // Replace decoder
        self.decoder = decoder;
        self.sample_rate = actual_sample_rate;
        self.uses_pcm_conversion = true;

        Ok(())
    }

    pub fn read(&mut self, buffer: &mut [f32]) -> Result<usize> {
        let mut samples_written = 0;

        while samples_written < buffer.len() {
            // Drain any buffered interleaved samples first.
            if self.sample_pos < self.sample_buf.len() {
                let available = self.sample_buf.len() - self.sample_pos;
                let to_copy = (buffer.len() - samples_written).min(available);
                buffer[samples_written..samples_written + to_copy]
                    .copy_from_slice(&self.sample_buf[self.sample_pos..self.sample_pos + to_copy]);
                samples_written += to_copy;
                self.sample_pos += to_copy;
                if samples_written >= buffer.len() {
                    break;
                }
            }

            // Read the next packet.
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => break, // End of stream.
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => {
                    tracing::error!("failed to read packet: {}", e);
                    return Err(RmpdError::Player(format!("Failed to read packet: {e}")));
                }
            };

            // Skip packets from other tracks.
            if packet.track_id != self.track_id {
                continue;
            }

            // Calculate instantaneous bitrate from the packet.
            if let Some(tb) = self.time_base
                && let Some(time) = tb.calc_time(Timestamp::new(packet.dur.get() as i64))
            {
                let duration_secs = time.as_secs_f64();
                if duration_secs > 0.0 {
                    let bitrate_bps = (packet.data.len() as f64 * 8.0) / duration_secs;
                    self.current_bitrate = Some((bitrate_bps / 1000.0) as u32);
                }
            }

            // Decode the packet.
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(e) => {
                    return Err(RmpdError::Player(format!("Failed to decode packet: {e}")));
                }
            };

            // For DSD with PCM conversion, the decoder must return F32.
            if self.uses_pcm_conversion && !matches!(decoded, GenericAudioBufferRef::F32(_)) {
                tracing::error!("DSD-to-PCM decoder returned a non-F32 buffer");
                return Err(RmpdError::Player(
                    "DSD decoder returned wrong sample format".to_owned(),
                ));
            }

            // Skip empty packets (can happen with metadata or padding).
            if decoded.frames() == 0 {
                continue;
            }

            // Update channels if not yet known.
            if self.channels.is_none() {
                self.channels = Some(decoded.spec().channels().count() as u8);
            }

            // Copy decoded audio as interleaved f32 into the reusable buffer.
            decoded.copy_to_vec_interleaved(&mut self.sample_buf);
            self.sample_pos = 0;
        }

        Ok(samples_written)
    }

    pub fn seek(&mut self, position: f64) -> Result<()> {
        if position < 0.0 {
            return Err(RmpdError::Player("Invalid seek position".to_owned()));
        }

        let time = Time::try_from_secs_f64(position)
            .ok_or_else(|| RmpdError::Player("Invalid seek position".to_owned()))?;

        self.reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| RmpdError::Player(format!("Seek failed: {e}")))?;

        self.decoder.reset();
        self.sample_buf.clear();
        self.sample_pos = 0;

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
            Ok(Some(packet)) => packet,
            Ok(None) => return Ok(0),
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(0);
            }
            Err(SymphoniaError::ResetRequired) => {
                self.decoder.reset();
                return self.read_dsd_raw(buffer);
            }
            Err(e) => {
                return Err(RmpdError::Player(format!("Failed to read DSD packet: {e}")));
            }
        };

        // Skip packets from other tracks
        if packet.track_id != self.track_id {
            return self.read_dsd_raw(buffer);
        }

        // For DSD, the packet buffer contains raw DSD data.
        // Copy it directly without decoding.
        buffer.extend_from_slice(&packet.data);

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
