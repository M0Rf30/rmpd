// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Opening songs for playback, and the MPD-style decoder plugin registry.
//!
//! The decoder itself ([`playcore::SymphoniaDecoder`]) lives in `playcore`; this module adds
//! what is rmpd-specific: resolving a song path *or* stream URI to a byte source, and the
//! `decoders` protocol surface (suffix / MIME lookup).

use playcore::decoder::{DecoderOptions, Hint};
use playcore::{SymphoniaDecoder, format};
use rmpd_core::error::{Result, RmpdError};
use std::path::Path;
use std::sync::LazyLock;

/// Open `path` — a local file or a remote stream URI — for decoding.
///
/// Gapless is always on (the `audio.gapless` config key was removed for that reason): the
/// decoder trims the encoder delay / padding frames the demuxer flags on each packet, so the
/// decoded buffers contain exactly the track's frames. Remote streams get their ICY "now
/// playing" title wired to [`SymphoniaDecoder::stream_title`]; local WavPack files pick up a
/// sibling `.wvc` correction file.
pub fn open(path: &Path) -> Result<SymphoniaDecoder> {
    let options = DecoderOptions { gapless: true };
    let Some(uri) = path.to_str().filter(|s| rmpd_stream::is_input_uri(s)) else {
        return Ok(SymphoniaDecoder::open(path, options)?);
    };

    let input = rmpd_stream::open(uri)
        .map_err(|e| RmpdError::Player(format!("Failed to open stream: {e}")))?;
    let mut hint = Hint::new();
    if let Some(ext) = input.extension_hint.as_deref() {
        hint.with_extension(ext);
    }
    let decoder = SymphoniaDecoder::open_source(input.source, hint, options)?;
    Ok(match input.title {
        Some(handle) => decoder.with_stream_title(move || handle.lock().clone()),
        None => decoder,
    })
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
/// Symphonia's probe has registered (see [`playcore::format`]), so this list can never drift
/// from what the probe (and therefore the library scanner) actually accepts.
///
/// Files whose codec has no registered decoder (e.g. Musepack) are rejected by `rmpd-library`'s
/// scan-time decodability check (`get_codecs().make_audio_decoder`), so only formats this decoder
/// can actually play make it into the library.
pub static SYMPHONIA_DECODER: LazyLock<DecoderPlugin> = LazyLock::new(|| DecoderPlugin {
    name: "symphonia",
    suffixes: format::SUPPORTED_EXTENSIONS.clone(),
    mime_types: format::SUPPORTED_MIME_TYPES.clone(),
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
