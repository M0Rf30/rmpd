// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::path::Path;
use std::sync::LazyLock;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::{
    AudioCodecId, AudioDecoder, AudioDecoderOptions, BitOrder, ChannelDataLayout,
};
use symphonia::core::errors::{Error as SymphoniaError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::well_known::FORMAT_ID_OGG;
use symphonia::core::formats::{
    FormatId, FormatOptions, FormatReader, MediaInfo, SeekMode, SeekTo, SeekedTo, TrackType,
};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
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
    /// ICY "now playing" title handle when decoding a remote stream.
    stream_title: Option<rmpd_stream::TitleHandle>,
    /// Set by a `seek` to (or past) the end of the stream: `read` /
    /// `read_dsd_raw` then report end-of-stream, whatever position the
    /// demuxer was left at. Cleared by the next seek.
    ended: bool,
    /// Pending sample-accurate seek: after a seek the demuxer lands at or before the
    /// requested position (e.g. a packet boundary, or a seek pre-roll in Matroska), and
    /// decoded frames before the requested timestamp are discarded.
    seek_skip: Option<SeekSkip>,
}

impl SymphoniaDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        // Open the media source: a remote stream URL or a local file.
        let mut hint = Hint::new();
        let stream_title;
        // Local WavPack files pick up a sibling `.wvc` (hybrid lossless); streams never do.
        let mut format_opts = FormatOptions::default();
        let mss = if let Some(uri) = path.to_str().filter(|s| rmpd_stream::is_input_uri(s)) {
            let input = rmpd_stream::open(uri)
                .map_err(|e| RmpdError::Player(format!("Failed to open stream: {e}")))?;
            stream_title = input.title;
            if let Some(ext) = input.extension_hint.as_deref() {
                hint.with_extension(ext);
            }
            MediaSourceStream::new(input.source, Default::default())
        } else {
            let file = std::fs::File::open(path)
                .map_err(|e| RmpdError::Player(format!("Failed to open file: {e}")))?;
            stream_title = None;
            format_opts = crate::format_registry::local_file_format_options(path);
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                hint.with_extension(ext);
            }
            MediaSourceStream::new(Box::new(file), Default::default())
        };

        // Probe the media source
        let reader = symphonia::default::get_probe()
            .probe(&hint, mss, format_opts, MetadataOptions::default())
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

        // Calculate total duration. A chained Ogg stream reports the duration of the whole
        // chain at the media level; the track only describes the first link. Otherwise use the
        // track frame count and timebase.
        let total_duration = chain_duration_secs(reader.format_info().format, reader.media_info())
            .or_else(|| match (track.num_frames, time_base) {
                (Some(n_frames), Some(tb)) => tb
                    .calc_time(Timestamp::new(n_frames as i64))
                    .map(|t| t.as_secs_f64()),
                _ => None,
            });

        // Create decoder in pass-through mode (no PCM conversion).
        // PCM conversion can be enabled later if needed.
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio, &AudioDecoderOptions::default())
            .map_err(|e| RmpdError::Player(format!("Failed to create decoder: {e}")))?;

        // The decoder may report a different output rate than the container (e.g. explicitly
        // signalled HE-AAC decodes at twice the AAC core rate the container declares).
        let sample_rate = decoder.codec_params().sample_rate.unwrap_or(sample_rate);

        let mut decoder = Self {
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
            stream_title,
            ended: false,
            seek_skip: None,
        };

        // Some containers don't declare the channel count in the codec
        // header; it is only known once the first packet is decoded. Resolve
        // it eagerly here rather than letting `format()`/`channels()` default
        // to stereo before the first real `read()` call (PLAY-05) — that
        // default would otherwise get latched into the device/output config
        // for the whole track. AAC is always primed: implicitly signalled
        // HE-AAC (SBR/PS found only in the bitstream, common in ADTS radio
        // streams) doubles the output rate and may turn a mono core into
        // stereo, which only the first decoded buffer reveals.
        if decoder.channels.is_none() || codec_id == CODEC_ID_AAC {
            decoder.prime_output_format()?;
        }

        Ok(decoder)
    }

    /// Decode packets until the first non-empty buffer (or the end of the stream), take the
    /// output channel count and sample rate from it, and buffer its audio (rather than
    /// discarding it) so the first real `read()` call still sees it.
    fn prime_output_format(&mut self) -> Result<()> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(()), // EOS with no decodable audio.
                Err(SymphoniaError::ResetRequired) => {
                    self.reinit_after_reset().map_err(|e| {
                        RmpdError::Player(format!(
                            "Failed to reinitialise after stream change: {e}"
                        ))
                    })?;
                    continue;
                }
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(());
                }
                Err(e) => {
                    return Err(RmpdError::Player(format!("Failed to read packet: {e}")));
                }
            };

            if packet.track_id != self.track_id {
                continue;
            }

            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(e) => {
                    return Err(RmpdError::Player(format!("Failed to decode packet: {e}")));
                }
            };

            if decoded.frames() == 0 {
                continue;
            }

            self.channels = Some(decoded.spec().channels().count() as u8);
            // A pass-through DSD buffer's `AudioSpec` rate is `dsd_rate / 8` (one frame = one
            // byte per channel), but the decoder's rate must stay the DSD rate (DoP needs it).
            if !(self.codec_id == CODEC_TYPE_DSD && !self.uses_pcm_conversion) {
                self.sample_rate = decoded.spec().rate();
            }
            decoded.copy_to_vec_interleaved(&mut self.sample_buf);
            self.sample_pos = 0;
            return Ok(());
        }
    }

    /// The current ICY "now playing" title for a remote stream, if any has
    /// been received. Returns `None` for local files or before the first
    /// metadata block arrives.
    #[must_use]
    pub fn stream_title(&self) -> Option<String> {
        self.stream_title.as_ref().and_then(|h| h.lock().clone())
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
        if self.ended {
            return Ok(0);
        }
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
                    // A new link of a chained stream (or a changed stream): the track list
                    // must be re-read and the decoder re-created.
                    self.reinit_after_reset().map_err(|e| {
                        RmpdError::Player(format!(
                            "Failed to reinitialise after stream change: {e}"
                        ))
                    })?;
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

            // Calculate instantaneous bitrate from the packet. Use the full block
            // duration (not the trimmed `dur`): a packet whose head/tail was trimmed
            // after a seek or at stream edges still carries all its bytes, so dividing
            // by the shortened duration would produce transient spikes.
            if let Some(tb) = self.time_base
                && let Some(time) = tb.calc_time(Timestamp::new(packet.block_dur().get() as i64))
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
            let frames = decoded.frames();
            let rate = decoded.spec().rate();
            let channel_count = decoded.spec().channels().count();
            decoded.copy_to_vec_interleaved(&mut self.sample_buf);
            // After a seek, drop the frames in front of the requested position: the demuxer
            // only lands on a packet at or before it (Matroska MP3/AAC/Vorbis even start a
            // seek pre-roll earlier).
            let skip_frames = self.seek_skip_frames(&packet, frames, rate);
            self.sample_pos = (skip_frames * channel_count).min(self.sample_buf.len());
        }

        Ok(samples_written)
    }

    pub fn seek(&mut self, position: f64) -> Result<()> {
        if position < 0.0 {
            return Err(RmpdError::Player("Invalid seek position".to_owned()));
        }

        let time = Time::try_from_secs_f64(position)
            .ok_or_else(|| RmpdError::Player("Invalid seek position".to_owned()))?;

        // A seek to (or past) the end of a stream of known length just ends
        // the song, like MPD (`Player::SeekDecoder` clamps to the song length
        // and the decoder then hits end-of-file). Demuxers disagree on what a
        // target at exactly the end is: isomp4, ape and dsf/dff refuse it as
        // out-of-range, flac runs into the end of the file while searching —
        // and either way the reader is left wherever it was. So on those
        // errors report end of stream from `read` ourselves instead of
        // failing the seek.
        let at_end = self.total_duration.is_some_and(|d| position >= d);

        // The demuxer may need a reset to complete the seek (a time seek into another link of
        // a chained Ogg stream): re-read the tracks, re-create the decoder, repeat the seek.
        self.seek_skip = None;
        let seeked = match seek_with_resets(self, time, MAX_SEEK_RESETS) {
            Ok(seeked) => Some(seeked),
            Err(SymphoniaError::SeekError(SeekErrorKind::OutOfRange)) if at_end => None,
            Err(SymphoniaError::IoError(e))
                if at_end && e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                None
            }
            // MPD's text for an unseekable source (`DecoderControl::Seek`:
            // `throw std::runtime_error("Not seekable")`), which `seek` /
            // `seekcur` now show the client verbatim.
            Err(SymphoniaError::SeekError(SeekErrorKind::Unseekable)) => {
                return Err(RmpdError::Player("Not seekable".to_owned()));
            }
            Err(e) => return Err(RmpdError::Player(format!("Seek failed: {e}"))),
        };

        self.decoder.reset();
        self.sample_buf.clear();
        self.sample_pos = 0;
        self.ended = at_end;

        // Raw (pass-through) DSD is not decoded here, so there is nothing to discard.
        let raw_dsd = self.codec_id == CODEC_TYPE_DSD && !self.uses_pcm_conversion;
        self.seek_skip = seeked.filter(|_| !raw_dsd && !at_end).and_then(|s| {
            SeekSkip::new(s.required_ts, s.actual_ts, self.time_base, self.sample_rate)
        });

        Ok(())
    }

    /// Number of leading frames of the just-decoded `packet` to discard to honour a pending
    /// sample-accurate seek, updating (and eventually clearing) the seek state.
    fn seek_skip_frames(&mut self, packet: &Packet, frames: usize, rate: u32) -> usize {
        let Some(tb) = self.time_base else {
            self.seek_skip = None;
            return 0;
        };
        let Some(skip) = self.seek_skip.as_mut() else {
            return 0;
        };
        // `pts` is the start of the decoded block; `trim_start` frames (encoder delay /
        // pre-roll flagged by the demuxer) are already removed from the decoded buffer.
        let valid_start = packet.pts.saturating_add(packet.trim_start);
        let (n, done) = skip.advance(valid_start, frames, tb, rate);
        if done {
            self.seek_skip = None;
        }
        n
    }

    /// Re-read the track list after the demuxer returned `ResetRequired` (a new link of a
    /// chained Ogg stream, ...) and re-create the decoder for the default audio track.
    fn reinit_after_reset(&mut self) -> std::result::Result<(), SymphoniaError> {
        let (track_id, time_base, audio) =
            {
                let track = self.reader.default_track(TrackType::Audio).ok_or(
                    SymphoniaError::Unsupported("no audio track after stream reset"),
                )?;
                let audio = match track.codec_params.as_ref() {
                    Some(CodecParameters::Audio(audio)) => audio.clone(),
                    _ => {
                        return Err(SymphoniaError::Unsupported(
                            "no audio codec parameters after stream reset",
                        ));
                    }
                };
                (track.id, track.time_base, audio)
            };

        let is_dsd = audio.codec == CODEC_TYPE_DSD;
        let keep_pcm = self.uses_pcm_conversion && is_dsd;
        let mut params = audio.clone();
        if keep_pcm {
            // `sample_rate` is the PCM output rate while DSD-to-PCM conversion is active.
            params.extra_data = Some(self.sample_rate.to_le_bytes().to_vec().into_boxed_slice());
        }
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())?;

        let new_rate = if keep_pcm {
            self.sample_rate
        } else {
            decoder
                .codec_params()
                .sample_rate
                .or(audio.sample_rate)
                .unwrap_or(self.sample_rate)
        };
        let new_channels = audio.channels.as_ref().map(|c| c.count() as u8);
        if new_rate != self.sample_rate || (new_channels.is_some() && new_channels != self.channels)
        {
            // The output was opened for the first link's format and cannot follow.
            tracing::warn!(
                "stream format changed mid-stream: {} Hz/{:?} ch -> {} Hz/{:?} ch",
                self.sample_rate,
                self.channels,
                new_rate,
                new_channels
            );
        }

        self.decoder = decoder;
        self.track_id = track_id;
        self.time_base = time_base;
        self.codec_id = audio.codec;
        self.sample_rate = new_rate;
        self.channels = new_channels;
        self.channel_data_layout = audio.channel_data_layout;
        self.bit_order = audio.bit_order;
        self.uses_pcm_conversion = keep_pcm;
        self.sample_buf.clear();
        self.sample_pos = 0;
        self.seek_skip = None;
        self.current_bitrate = None;
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
        if self.ended {
            return Ok(0);
        }

        // Read next packet
        let packet = match self.reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => return Ok(0),
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(0);
            }
            Err(SymphoniaError::ResetRequired) => {
                self.reinit_after_reset().map_err(|e| {
                    RmpdError::Player(format!("Failed to reinitialise after stream change: {e}"))
                })?;
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

/// How often a seek is repeated after the demuxer returned `ResetRequired` (a time seek that
/// crosses into another link of a chained Ogg stream switches links and asks for a reset; the
/// repeated seek then completes inside the new link). One reset is the normal case; the
/// second retry is slack. A reader still asking for resets after that is broken.
const MAX_SEEK_RESETS: u32 = 2;

/// Something that can seek and be rebuilt after a reset (the decoder; a mock in tests).
trait ResettableSeek {
    /// Seek to `time` on the current default audio track.
    fn try_seek(&mut self, time: Time) -> std::result::Result<SeekedTo, SymphoniaError>;
    /// Re-read the tracks and re-create the decoder after `ResetRequired`.
    fn rebuild_after_reset(&mut self) -> std::result::Result<(), SymphoniaError>;
}

impl ResettableSeek for SymphoniaDecoder {
    fn try_seek(&mut self, time: Time) -> std::result::Result<SeekedTo, SymphoniaError> {
        // The track id is read on every attempt: it changes when the reset switched links.
        self.reader.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(self.track_id),
            },
        )
    }

    fn rebuild_after_reset(&mut self) -> std::result::Result<(), SymphoniaError> {
        self.reinit_after_reset()
    }
}

/// Seek, and on `ResetRequired` rebuild and repeat the same seek, at most `max_resets` times.
/// Any other outcome (success or error) is returned as is.
fn seek_with_resets<T: ResettableSeek + ?Sized>(
    target: &mut T,
    time: Time,
    max_resets: u32,
) -> std::result::Result<SeekedTo, SymphoniaError> {
    let mut resets = 0;
    loop {
        match target.try_seek(time) {
            Err(SymphoniaError::ResetRequired) if resets < max_resets => {
                resets += 1;
                target.rebuild_after_reset()?;
            }
            other => return other,
        }
    }
}

/// Duration in seconds of a whole chained Ogg stream, if the reader describes one.
///
/// symphonia's Ogg reader locates the links of a seekable chained stream up front and then
/// publishes the chain-level duration (sum of all links) in `MediaInfo` with a nanosecond
/// timebase, while `Track` only describes the current link. For anything else (including a
/// single-link Ogg stream, whose media info just mirrors the track) this returns `None` so the
/// per-track duration is used.
#[must_use]
pub fn chain_duration_secs(format: FormatId, media_info: &MediaInfo) -> Option<f64> {
    if format != FORMAT_ID_OGG {
        return None;
    }
    let tb = media_info.time_base?;
    if tb.numer.get() != 1 || tb.denom.get() != 1_000_000_000 {
        return None;
    }
    tb.calc_duration(media_info.duration?)
        .map(|t| t.as_secs_f64())
}

/// Convert a span of `ticks` of timebase `tb` into frames at `rate` Hz (rounded to nearest).
fn ticks_to_frames(ticks: u64, tb: TimeBase, rate: u32) -> u64 {
    let denom = u128::from(tb.denom.get());
    let num = u128::from(ticks) * u128::from(tb.numer.get()) * u128::from(rate);
    u64::try_from((num + denom / 2) / denom).unwrap_or(u64::MAX)
}

/// Frames to drop after a seek so playback starts exactly at the requested timestamp.
///
/// `FormatReader::seek` only lands on a packet at or before the target (`actual_ts <=
/// required_ts`); Matroska MP3/AAC/Vorbis additionally start a 200 ms seek pre-roll earlier. The
/// decoder must decode from there (it needs the pre-roll to warm up) and the frames before
/// `required_ts` are discarded, rather than trusting `actual_ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SeekSkip {
    /// The timestamp playback must start at, in the track timebase.
    required: Timestamp,
    /// Upper bound of frames to discard: the announced distance plus one second. Protects
    /// against a demuxer with unreliable packet timestamps eating the whole stream.
    budget: u64,
}

impl SeekSkip {
    /// `None` when nothing has to be discarded (landed exactly on, or after, the target).
    fn new(
        required: Timestamp,
        actual: Timestamp,
        tb: Option<TimeBase>,
        rate: u32,
    ) -> Option<Self> {
        let tb = tb?;
        let gap = required.duration_from(actual)?;
        if gap.is_zero() {
            return None;
        }
        let budget = ticks_to_frames(gap.get(), tb, rate).saturating_add(u64::from(rate));
        Some(Self { required, budget })
    }

    /// Account for one decoded buffer of `decoded_frames` frames whose first valid frame is at
    /// `valid_start`. Returns how many leading frames to discard and whether the target has
    /// been reached (so no more skipping is needed).
    fn advance(
        &mut self,
        valid_start: Timestamp,
        decoded_frames: usize,
        tb: TimeBase,
        rate: u32,
    ) -> (usize, bool) {
        let gap = match self.required.duration_from(valid_start) {
            Some(gap) if !gap.is_zero() => gap,
            // At or past the target.
            _ => return (0, true),
        };
        let gap_frames = ticks_to_frames(gap.get(), tb, rate);
        let skip = gap_frames.min(decoded_frames as u64).min(self.budget);
        self.budget -= skip;
        let reached = gap_frames <= decoded_frames as u64 || self.budget == 0;
        (skip as usize, reached)
    }
}

/// Trait for audio decoders
pub trait Decoder: Send {
    fn read(&mut self, buffer: &mut [f32]) -> Result<usize>;
    fn seek(&mut self, position: f64) -> Result<()>;
    fn format(&self) -> AudioFormat;
    fn duration(&self) -> Option<f64>;
}

impl Decoder for SymphoniaDecoder {
    fn read(&mut self, buffer: &mut [f32]) -> Result<usize> {
        self.read(buffer)
    }
    fn seek(&mut self, position: f64) -> Result<()> {
        self.seek(position)
    }
    fn format(&self) -> AudioFormat {
        self.format()
    }
    fn duration(&self) -> Option<f64> {
        self.duration()
    }
}

// ---------------------------------------------------------------------------
// Decoder plugin SPI (MPD-style DecoderPlugin registry)
// ---------------------------------------------------------------------------

/// A decoder plugin descriptor: how to recognise files it can decode.
pub struct DecoderPlugin {
    pub name: &'static str,
    pub suffixes: Vec<&'static str>,
    pub mime_types: Vec<&'static str>,
}

/// The Symphonia-backed decoder handles all of rmpd's playable formats.
///
/// Suffixes and MIME types are derived at runtime from every container/format reader
/// Symphonia's probe has registered (see `crate::format_registry`), so this list can never drift
/// from what the probe (and therefore the library scanner) actually accepts.
///
/// Files whose codec has no registered decoder (e.g. Musepack) are rejected by `rmpd-library`'s
/// scan-time decodability check (`get_codecs().make_audio_decoder`), so only formats this decoder
/// can actually play make it into the library.
pub static SYMPHONIA_DECODER: LazyLock<DecoderPlugin> = LazyLock::new(|| DecoderPlugin {
    name: "symphonia",
    suffixes: crate::format_registry::SUPPORTED_EXTENSIONS.clone(),
    mime_types: crate::format_registry::SUPPORTED_MIME_TYPES.clone(),
});

/// All compiled-in decoder plugins (runtime registry, MPD-style).
pub static DECODER_PLUGINS: LazyLock<Vec<&'static DecoderPlugin>> =
    LazyLock::new(|| vec![&*SYMPHONIA_DECODER]);

/// Find a decoder plugin that lists `suffix` (case-insensitive, no leading dot).
#[must_use]
pub fn decoder_for_suffix(suffix: &str) -> Option<&'static DecoderPlugin> {
    let s = suffix.trim_start_matches('.').to_ascii_lowercase();
    DECODER_PLUGINS
        .iter()
        .copied()
        .find(|p| p.suffixes.iter().any(|x| *x == s))
}

/// Whether any compiled-in decoder supports `suffix`.
#[must_use]
pub fn is_supported_suffix(suffix: &str) -> bool {
    decoder_for_suffix(suffix).is_some()
}

#[cfg(test)]
mod seek_tests {
    use super::*;
    use std::collections::VecDeque;

    type SeekResult = std::result::Result<SeekedTo, SymphoniaError>;

    fn seeked(required: i64, actual: i64) -> SeekedTo {
        SeekedTo {
            track_id: 0,
            required_ts: Timestamp::new(required),
            actual_ts: Timestamp::new(actual),
        }
    }

    /// Scripted reader: each seek pops the next result; records the targets and rebuilds.
    struct Mock {
        results: VecDeque<SeekResult>,
        seeks: Vec<Time>,
        rebuilds: u32,
        rebuild_fails: bool,
    }

    impl Mock {
        fn new(results: Vec<SeekResult>) -> Self {
            Self {
                results: results.into(),
                seeks: Vec::new(),
                rebuilds: 0,
                rebuild_fails: false,
            }
        }
    }

    impl ResettableSeek for Mock {
        fn try_seek(&mut self, time: Time) -> SeekResult {
            self.seeks.push(time);
            self.results.pop_front().expect("unexpected extra seek")
        }

        fn rebuild_after_reset(&mut self) -> std::result::Result<(), SymphoniaError> {
            self.rebuilds += 1;
            if self.rebuild_fails {
                Err(SymphoniaError::Unsupported("no track"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn seek_without_reset_does_not_rebuild() {
        let mut m = Mock::new(vec![Ok(seeked(10, 8))]);
        let t = Time::from_millis(1500);
        let got = seek_with_resets(&mut m, t, MAX_SEEK_RESETS).unwrap();
        assert_eq!(got.actual_ts, Timestamp::new(8));
        assert_eq!(m.rebuilds, 0);
        assert_eq!(m.seeks, vec![t]);
    }

    #[test]
    fn reset_rebuilds_and_repeats_the_same_seek() {
        let mut m = Mock::new(vec![Err(SymphoniaError::ResetRequired), Ok(seeked(10, 10))]);
        let t = Time::from_millis(90_000);
        assert!(seek_with_resets(&mut m, t, MAX_SEEK_RESETS).is_ok());
        assert_eq!(m.rebuilds, 1);
        assert_eq!(m.seeks, vec![t, t], "the very same target must be retried");
    }

    #[test]
    fn two_resets_are_tolerated() {
        let mut m = Mock::new(vec![
            Err(SymphoniaError::ResetRequired),
            Err(SymphoniaError::ResetRequired),
            Ok(seeked(1, 1)),
        ]);
        assert!(seek_with_resets(&mut m, Time::from_millis(1), 2).is_ok());
        assert_eq!(m.rebuilds, 2);
    }

    #[test]
    fn endless_resets_are_bounded() {
        let mut m = Mock::new(vec![
            Err(SymphoniaError::ResetRequired),
            Err(SymphoniaError::ResetRequired),
            Err(SymphoniaError::ResetRequired),
        ]);
        let err = seek_with_resets(&mut m, Time::from_millis(1), 2).unwrap_err();
        assert!(matches!(err, SymphoniaError::ResetRequired));
        assert_eq!(m.rebuilds, 2);
        assert_eq!(m.seeks.len(), 3);
    }

    #[test]
    fn other_errors_pass_through_without_rebuild() {
        let mut m = Mock::new(vec![Err(SymphoniaError::SeekError(
            SeekErrorKind::OutOfRange,
        ))]);
        let err = seek_with_resets(&mut m, Time::from_millis(1), 2).unwrap_err();
        assert!(matches!(
            err,
            SymphoniaError::SeekError(SeekErrorKind::OutOfRange)
        ));
        assert_eq!(m.rebuilds, 0);
    }

    #[test]
    fn rebuild_failure_aborts_the_seek() {
        let mut m = Mock::new(vec![Err(SymphoniaError::ResetRequired)]);
        m.rebuild_fails = true;
        let err = seek_with_resets(&mut m, Time::from_millis(1), 2).unwrap_err();
        assert!(matches!(err, SymphoniaError::Unsupported(_)));
        assert_eq!(m.seeks.len(), 1);
    }

    fn tb(denom: u32) -> TimeBase {
        TimeBase::try_new(1, denom).unwrap()
    }

    /// Feed `packets` back-to-back (each `len` frames from `start`) through the skip state and
    /// return (total frames skipped, index of the packet where the target was reached).
    fn run(
        mut skip: SeekSkip,
        start: i64,
        len: usize,
        packets: usize,
        tb: TimeBase,
        rate: u32,
    ) -> (usize, Option<usize>) {
        let mut skipped = 0;
        for i in 0..packets {
            let pts = Timestamp::new(start + (i * len) as i64);
            let (n, done) = skip.advance(pts, len, tb, rate);
            skipped += n;
            if done {
                return (skipped, Some(i));
            }
        }
        (skipped, None)
    }

    #[test]
    fn skips_the_seek_pre_roll_exactly() {
        // Matroska audio (timebase 1/rate): 200 ms pre-roll before the requested timestamp.
        let rate = 48_000;
        let required = 5 * 48_000 + 123;
        let actual = required - 9_600;
        let skip = SeekSkip::new(
            Timestamp::new(required),
            Timestamp::new(actual),
            Some(tb(rate)),
            rate,
        )
        .unwrap();
        let (skipped, reached) = run(skip, actual, 1152, 64, tb(rate), rate);
        assert_eq!(skipped, 9_600, "exactly required - actual frames");
        assert!(reached.is_some());
    }

    #[test]
    fn skips_within_a_packet_after_a_coarse_landing() {
        // FLAC-like: packet of 4096 frames starts 1000 frames before the target.
        let rate = 44_100;
        let mut skip = SeekSkip::new(
            Timestamp::new(10_000),
            Timestamp::new(9_000),
            Some(tb(rate)),
            rate,
        )
        .unwrap();
        assert_eq!(
            skip.advance(Timestamp::new(9_000), 4096, tb(rate), rate),
            (1_000, true)
        );
    }

    #[test]
    fn converts_between_timebase_and_output_rate() {
        // A 1/1000 timebase (legacy Matroska audio) with 44.1 kHz output: 500 ms = 22050.
        let mut skip = SeekSkip::new(
            Timestamp::new(1_500),
            Timestamp::new(1_000),
            Some(tb(1000)),
            44_100,
        )
        .unwrap();
        assert_eq!(
            skip.advance(Timestamp::new(1_000), 100_000, tb(1000), 44_100),
            (22_050, true)
        );
    }

    #[test]
    fn nothing_to_skip_when_the_seek_landed_on_or_after_the_target() {
        let t = Some(tb(48_000));
        assert!(SeekSkip::new(Timestamp::new(100), Timestamp::new(100), t, 48_000).is_none());
        assert!(SeekSkip::new(Timestamp::new(100), Timestamp::new(200), t, 48_000).is_none());
        assert!(SeekSkip::new(Timestamp::new(100), Timestamp::new(0), None, 48_000).is_none());
    }

    #[test]
    fn packet_trim_start_moves_the_valid_start() {
        // The first valid frame of a trimmed packet is at pts + trim_start: only that distance
        // to the target remains.
        let rate = 48_000;
        let mut skip = SeekSkip::new(
            Timestamp::new(0),
            Timestamp::new(-312),
            Some(tb(rate)),
            rate,
        )
        .unwrap();
        let valid_start =
            Timestamp::new(-312).saturating_add(symphonia::core::units::Duration::new(312));
        assert_eq!(skip.advance(valid_start, 1024, tb(rate), rate), (0, true));
    }

    #[test]
    fn unreliable_timestamps_cannot_swallow_the_stream() {
        // Every packet claims to start at 0: skipping stops at the announced distance + 1 s.
        let rate = 48_000;
        let mut skip = SeekSkip::new(
            Timestamp::new(48_000),
            Timestamp::new(38_400),
            Some(tb(rate)),
            rate,
        )
        .unwrap();
        let mut skipped = 0usize;
        for _ in 0..1000 {
            let (n, done) = skip.advance(Timestamp::new(0), 1024, tb(rate), rate);
            skipped += n;
            if done {
                break;
            }
        }
        assert!(skipped as u64 <= 9_600 + u64::from(rate));
    }

    #[test]
    fn tick_conversion_rounds_to_nearest_frame() {
        // DSD64 timeline (1/2822400) to 44.1 kHz PCM: 64 DSD samples per PCM frame.
        assert_eq!(ticks_to_frames(64, tb(2_822_400), 44_100), 1);
        assert_eq!(ticks_to_frames(95, tb(2_822_400), 44_100), 1);
        assert_eq!(ticks_to_frames(97, tb(2_822_400), 44_100), 2);
        // Pass-through DSD frames (rate / 8): 8 DSD samples per frame.
        assert_eq!(ticks_to_frames(2_822_400, tb(2_822_400), 352_800), 352_800);
    }

    fn ns_media_info(secs: u64) -> MediaInfo {
        let mut mi = MediaInfo::new();
        mi.with_time_base(TimeBase::try_new(1, 1_000_000_000).unwrap());
        mi.with_duration(symphonia::core::units::Duration::new(secs * 1_000_000_000));
        mi
    }

    #[test]
    fn ogg_chain_duration_comes_from_media_info() {
        let d = chain_duration_secs(FORMAT_ID_OGG, &ns_media_info(95)).unwrap();
        assert!((d - 95.0).abs() < 1e-9);
    }

    #[test]
    fn chain_duration_is_ogg_and_ns_timebase_only() {
        use symphonia::core::formats::well_known::FORMAT_ID_FLAC;
        assert!(chain_duration_secs(FORMAT_ID_FLAC, &ns_media_info(95)).is_none());
        // Single-link Ogg: media info mirrors the track (sample-rate timebase).
        let mut mi = MediaInfo::new();
        mi.with_time_base(TimeBase::try_new(1, 48_000).unwrap());
        mi.with_duration(symphonia::core::units::Duration::new(48_000));
        assert!(chain_duration_secs(FORMAT_ID_OGG, &mi).is_none());
        assert!(chain_duration_secs(FORMAT_ID_OGG, &MediaInfo::new()).is_none());
    }
}
