// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure-Rust DSP filter plugins for the [`crate::filter`] registry.
//!
//! | Registry name         | Type                | Purpose                                    |
//! |-----------------------|---------------------|--------------------------------------------|
//! | `normalize`           | [`Normalize`]       | AGC / volume normalization with limiter    |
//! | `equalizer`           | [`Equalizer`]       | N-band peaking EQ (RBJ biquads) + preamp   |
//! | `route`, `channels`   | [`Route`]           | stereo→mono downmix, swap, channel remap   |
//!
//! All filters work in place on interleaved `f32` samples and keep the channel
//! count unchanged (so they can sit in front of any output without
//! renegotiating its format). Every filter is told the stream format through
//! [`AudioFilter::set_format`]; the equalizer recomputes its biquad
//! coefficients whenever the sample rate changes.

use crate::filter::{AudioFilter, FilterError, FilterParams};
use std::f64::consts::PI;

// ── Settings helpers ─────────────────────────────────────────────────────────

fn num(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Float(f) if f.is_finite() => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn cfg_err(msg: impl Into<String>) -> FilterError {
    FilterError::Config(msg.into())
}

/// Read an optional numeric setting, requiring it to lie in `range`.
fn opt_num(
    settings: &toml::Table,
    key: &str,
    default: f64,
    range: std::ops::RangeInclusive<f64>,
) -> Result<f64, FilterError> {
    let Some(v) = settings.get(key) else {
        return Ok(default);
    };
    let n = num(v).ok_or_else(|| cfg_err(format!("`{key}` must be a number")))?;
    if range.contains(&n) {
        Ok(n)
    } else {
        Err(cfg_err(format!(
            "`{key}` must be between {} and {}",
            range.start(),
            range.end()
        )))
    }
}

fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

// ── Biquad ───────────────────────────────────────────────────────────────────

/// Normalised biquad coefficients (`a0 == 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl BiquadCoeffs {
    /// Pass-through section.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// RBJ "peaking EQ" section. A gain of (effectively) 0 dB yields
    /// [`Self::IDENTITY`] exactly. `freq` is clamped below Nyquist.
    #[must_use]
    pub fn peaking(sample_rate: f64, freq: f64, gain_db: f64, q: f64) -> Self {
        if gain_db.abs() < 1e-9 {
            return Self::IDENTITY;
        }
        Self::peaking_unchecked(sample_rate, freq, gain_db, q)
    }

    /// Same as [`Self::peaking`] but always evaluates the cookbook formula
    /// (used to prove that a 0 dB band is mathematically an identity).
    #[must_use]
    pub fn peaking_unchecked(sample_rate: f64, freq: f64, gain_db: f64, q: f64) -> Self {
        if sample_rate < 100.0 || q <= 0.0 || !q.is_finite() {
            return Self::IDENTITY;
        }
        let f = freq.clamp(1.0, sample_rate * 0.499);
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    /// Whether this section is the exact pass-through.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// Linear magnitude response at `freq` Hz for the given sample rate.
    #[must_use]
    pub fn magnitude_at(&self, sample_rate: f64, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        (num_re.hypot(num_im)) / (den_re.hypot(den_im))
    }
}

/// Per-channel biquad memory (transposed direct form II).
#[derive(Debug, Clone, Copy, Default)]
pub struct BiquadState {
    z1: f64,
    z2: f64,
}

impl BiquadState {
    /// Run one sample through `c`.
    #[inline]
    pub fn process(&mut self, c: &BiquadCoeffs, x: f64) -> f64 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        // Flush denormals so a long silence does not become expensive.
        if self.z1.abs() < 1e-30 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-30 {
            self.z2 = 0.0;
        }
        y
    }
}

// ── Equalizer ────────────────────────────────────────────────────────────────

/// Upper bound on the number of bands of one equalizer.
pub const MAX_EQ_BANDS: usize = 32;

/// One peaking band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqBand {
    /// Centre frequency in Hz.
    pub freq: f64,
    /// Boost (positive) or cut (negative) in dB.
    pub gain_db: f64,
    /// Quality factor (bandwidth); higher is narrower.
    pub q: f64,
}

/// N-band parametric equalizer built from cascaded peaking biquads, with a
/// leading preamp. Coefficients are recomputed on a sample-rate change.
pub struct Equalizer {
    bands: Vec<EqBand>,
    preamp: f64,
    sample_rate: u32,
    channels: usize,
    coeffs: Vec<BiquadCoeffs>,
    /// `coeffs.len() * channels` states, band-major.
    state: Vec<BiquadState>,
}

impl Equalizer {
    #[must_use]
    pub fn new(bands: Vec<EqBand>, preamp_db: f64, sample_rate: u32, channels: u8) -> Self {
        let mut eq = Self {
            bands,
            preamp: db_to_lin(preamp_db),
            sample_rate,
            channels: usize::from(channels),
            coeffs: Vec::new(),
            state: Vec::new(),
        };
        eq.recompute();
        eq
    }

    fn recompute(&mut self) {
        let sr = f64::from(self.sample_rate);
        self.coeffs = self
            .bands
            .iter()
            .map(|b| BiquadCoeffs::peaking(sr, b.freq, b.gain_db, b.q))
            .filter(|c| !c.is_identity())
            .collect();
        self.state = vec![BiquadState::default(); self.coeffs.len() * self.channels];
    }

    /// Number of active (non-identity) biquad sections.
    #[must_use]
    pub fn active_bands(&self) -> usize {
        self.coeffs.len()
    }
}

impl AudioFilter for Equalizer {
    fn name(&self) -> &str {
        "equalizer"
    }

    fn set_format(&mut self, sample_rate: u32, channels: u8) {
        let channels = usize::from(channels);
        if sample_rate != self.sample_rate || channels != self.channels {
            self.sample_rate = sample_rate;
            self.channels = channels;
            self.recompute();
        }
    }

    fn apply(&mut self, buf: &mut [f32]) {
        if self.channels == 0 {
            return;
        }
        if (self.preamp - 1.0).abs() > 1e-12 {
            let g = self.preamp as f32;
            for s in buf.iter_mut() {
                *s *= g;
            }
        }
        for (b, c) in self.coeffs.iter().enumerate() {
            let base = b * self.channels;
            for frame in buf.chunks_mut(self.channels) {
                for (ch, s) in frame.iter_mut().enumerate() {
                    *s = self.state[base + ch].process(c, f64::from(*s)) as f32;
                }
            }
        }
    }
}

/// Setting keys accepted by the `equalizer` filter.
pub const EQUALIZER_SETTINGS: &[&str] = &["bands", "preamp_db"];

fn parse_bands(v: Option<&toml::Value>) -> Result<Vec<EqBand>, FilterError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let toml::Value::Array(items) = v else {
        return Err(cfg_err(
            "`bands` must be an array of { freq, gain_db, q } tables",
        ));
    };
    if items.len() > MAX_EQ_BANDS {
        return Err(cfg_err(format!(
            "at most {MAX_EQ_BANDS} bands are supported"
        )));
    }
    let mut bands = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let toml::Value::Table(t) = item else {
            return Err(cfg_err(format!("bands[{i}] must be a table")));
        };
        if let Some(key) = t
            .keys()
            .find(|k| !matches!(k.as_str(), "freq" | "gain_db" | "q"))
        {
            return Err(cfg_err(format!(
                "bands[{i}]: unknown key `{key}` (expected freq, gain_db, q)"
            )));
        }
        let req = |key: &str| -> Result<f64, FilterError> {
            t.get(key)
                .and_then(num)
                .ok_or_else(|| cfg_err(format!("bands[{i}]: `{key}` is required and numeric")))
        };
        let freq = req("freq")?;
        let gain_db = req("gain_db")?;
        let q = match t.get("q") {
            None => 1.0,
            Some(v) => {
                num(v).ok_or_else(|| cfg_err(format!("bands[{i}]: `q` must be a number")))?
            }
        };
        if !(10.0..=96_000.0).contains(&freq) {
            return Err(cfg_err(format!(
                "bands[{i}]: `freq` must be between 10 and 96000 Hz"
            )));
        }
        if !(-48.0..=48.0).contains(&gain_db) {
            return Err(cfg_err(format!(
                "bands[{i}]: `gain_db` must be between -48 and 48"
            )));
        }
        if !(0.05..=100.0).contains(&q) {
            return Err(cfg_err(format!(
                "bands[{i}]: `q` must be between 0.05 and 100"
            )));
        }
        bands.push(EqBand { freq, gain_db, q });
    }
    Ok(bands)
}

/// Factory of the `equalizer` registry entry.
pub fn equalizer_factory(p: &FilterParams<'_>) -> Result<Box<dyn AudioFilter>, FilterError> {
    let preamp_db = opt_num(p.settings, "preamp_db", 0.0, -60.0..=24.0)?;
    let bands = parse_bands(p.settings.get("bands"))?;
    Ok(Box::new(Equalizer::new(
        bands,
        preamp_db,
        p.sample_rate,
        p.channels,
    )))
}

// ── Normalize (AGC + limiter) ────────────────────────────────────────────────

/// Below this envelope (≈ -60 dBFS) the gain is held instead of chasing the
/// signal upwards, so silence / noise floors are not pumped up.
const SILENCE_ENVELOPE: f32 = 1e-3;
/// Floor of the AGC gain (≈ -40 dB).
const MIN_GAIN: f32 = 0.01;
/// Release time of the safety limiter.
const LIMITER_RELEASE_MS: f64 = 50.0;

/// Tunables of [`Normalize`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizeParams {
    /// Peak level the AGC steers towards, in dBFS.
    pub target_db: f64,
    /// Maximum amplification applied to quiet material, in dB.
    pub max_gain_db: f64,
    /// Time constant when the gain must fall (loud passage), in ms.
    pub attack_ms: f64,
    /// Time constant when the gain may rise (quiet passage), in ms.
    pub release_ms: f64,
    /// Time constant of the peak-hold window the level is measured over, in ms.
    pub window_ms: f64,
    /// Hard output ceiling enforced by the limiter, in dBFS.
    pub ceiling_db: f64,
}

impl Default for NormalizeParams {
    fn default() -> Self {
        Self {
            target_db: -3.0,
            max_gain_db: 30.0,
            attack_ms: 10.0,
            release_ms: 1500.0,
            window_ms: 400.0,
            ceiling_db: -0.3,
        }
    }
}

/// MPD-style automatic gain control (`normalize` filter): a smoothed gain
/// steers the recent peak level towards `target_db`, capped by `max_gain_db`;
/// an instantaneous-attack limiter guarantees the output never exceeds
/// `ceiling_db`.
pub struct Normalize {
    params: NormalizeParams,
    sample_rate: u32,
    channels: usize,
    target: f32,
    max_gain: f32,
    ceiling: f32,
    attack_coef: f32,
    release_coef: f32,
    env_decay: f32,
    limiter_release: f32,
    /// Decaying peak envelope of the input.
    env: f32,
    /// Smoothed AGC gain.
    gain: f32,
    /// Limiter gain (1.0 = not limiting).
    limiter: f32,
}

/// One-pole smoothing coefficient for time constant `ms` at `sample_rate`.
fn smoothing_coef(ms: f64, sample_rate: u32) -> f32 {
    let samples = (ms * 0.001 * f64::from(sample_rate)).max(1.0);
    (1.0 - (-1.0 / samples).exp()) as f32
}

impl Normalize {
    #[must_use]
    pub fn new(params: NormalizeParams, sample_rate: u32, channels: u8) -> Self {
        let mut n = Self {
            params,
            sample_rate,
            channels: usize::from(channels),
            target: 0.0,
            max_gain: 1.0,
            ceiling: 1.0,
            attack_coef: 0.0,
            release_coef: 0.0,
            env_decay: 0.0,
            limiter_release: 0.0,
            env: 0.0,
            gain: 1.0,
            limiter: 1.0,
        };
        n.update_coefs();
        n
    }

    fn update_coefs(&mut self) {
        let p = &self.params;
        self.target = db_to_lin(p.target_db) as f32;
        self.max_gain = db_to_lin(p.max_gain_db) as f32;
        self.ceiling = db_to_lin(p.ceiling_db) as f32;
        self.attack_coef = smoothing_coef(p.attack_ms, self.sample_rate);
        self.release_coef = smoothing_coef(p.release_ms, self.sample_rate);
        // The envelope decays by exp(-1/tau) per frame.
        self.env_decay = 1.0 - smoothing_coef(p.window_ms, self.sample_rate);
        self.limiter_release = smoothing_coef(LIMITER_RELEASE_MS, self.sample_rate);
    }

    /// Current AGC gain (linear), for diagnostics and tests.
    #[must_use]
    pub fn current_gain(&self) -> f32 {
        self.gain
    }
}

impl AudioFilter for Normalize {
    fn name(&self) -> &str {
        "normalize"
    }

    fn set_format(&mut self, sample_rate: u32, channels: u8) {
        self.channels = usize::from(channels);
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.update_coefs();
        }
    }

    fn apply(&mut self, buf: &mut [f32]) {
        if self.channels == 0 {
            return;
        }
        for frame in buf.chunks_mut(self.channels) {
            let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            self.env = peak.max(self.env * self.env_decay);

            let desired = if self.env < SILENCE_ENVELOPE {
                self.gain
            } else {
                (self.target / self.env).clamp(MIN_GAIN, self.max_gain)
            };
            let coef = if desired < self.gain {
                self.attack_coef
            } else {
                self.release_coef
            };
            self.gain += (desired - self.gain) * coef;

            // Limiter: instant attack, smooth release; never exceeds ceiling.
            let out_peak = peak * self.gain;
            let need = if out_peak > self.ceiling {
                self.ceiling / out_peak
            } else {
                1.0
            };
            self.limiter = (self.limiter + (1.0 - self.limiter) * self.limiter_release).min(need);

            let g = self.gain * self.limiter;
            for s in frame.iter_mut() {
                *s *= g;
            }
        }
    }
}

/// Setting keys accepted by the `normalize` filter.
pub const NORMALIZE_SETTINGS: &[&str] = &[
    "target_db",
    "max_gain_db",
    "attack_ms",
    "release_ms",
    "window_ms",
    "ceiling_db",
];

/// Factory of the `normalize` registry entry.
pub fn normalize_factory(p: &FilterParams<'_>) -> Result<Box<dyn AudioFilter>, FilterError> {
    let d = NormalizeParams::default();
    let s = p.settings;
    let params = NormalizeParams {
        target_db: opt_num(s, "target_db", d.target_db, -60.0..=0.0)?,
        max_gain_db: opt_num(s, "max_gain_db", d.max_gain_db, 0.0..=60.0)?,
        attack_ms: opt_num(s, "attack_ms", d.attack_ms, 0.1..=10_000.0)?,
        release_ms: opt_num(s, "release_ms", d.release_ms, 1.0..=60_000.0)?,
        window_ms: opt_num(s, "window_ms", d.window_ms, 1.0..=60_000.0)?,
        ceiling_db: opt_num(s, "ceiling_db", d.ceiling_db, -60.0..=0.0)?,
    };
    Ok(Box::new(Normalize::new(params, p.sample_rate, p.channels)))
}

// ── Route / channels ─────────────────────────────────────────────────────────

/// What a [`Route`] filter does to each frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteMode {
    /// Average all channels into every channel (stereo→mono downmix that
    /// keeps the channel count).
    Mono,
    /// Swap the two channels of a stereo stream.
    Swap,
    /// Copy the left (first) channel to every channel.
    Left,
    /// Copy the right (second) channel to every channel.
    Right,
    /// Output channel `i` takes input channel `map[i]`.
    Map(Vec<usize>),
}

/// Channel router. Stream layouts a mode does not apply to (e.g. `swap` on
/// a 6-channel stream, or a `map` of the wrong length) pass through unchanged.
pub struct Route {
    mode: RouteMode,
    channels: usize,
    scratch: Vec<f32>,
}

impl Route {
    #[must_use]
    pub fn new(mode: RouteMode, channels: u8) -> Self {
        let channels = usize::from(channels);
        Self {
            mode,
            channels,
            scratch: vec![0.0; channels],
        }
    }

    /// Whether the mode can act on the current channel count.
    #[must_use]
    pub fn is_effective(&self) -> bool {
        match &self.mode {
            RouteMode::Mono | RouteMode::Left | RouteMode::Right => self.channels >= 2,
            RouteMode::Swap => self.channels == 2,
            RouteMode::Map(m) => {
                m.len() == self.channels
                    && self.channels > 0
                    && m.iter().all(|&i| i < self.channels)
            }
        }
    }
}

impl AudioFilter for Route {
    fn name(&self) -> &str {
        "route"
    }

    fn set_format(&mut self, _sample_rate: u32, channels: u8) {
        self.channels = usize::from(channels);
        self.scratch = vec![0.0; self.channels];
    }

    fn apply(&mut self, buf: &mut [f32]) {
        if !self.is_effective() {
            return;
        }
        let ch = self.channels;
        for frame in buf.chunks_mut(ch) {
            if frame.len() != ch {
                continue; // trailing partial frame
            }
            match &self.mode {
                RouteMode::Mono => {
                    let avg = frame.iter().sum::<f32>() / ch as f32;
                    frame.fill(avg);
                }
                RouteMode::Swap => frame.swap(0, 1),
                RouteMode::Left => {
                    let l = frame[0];
                    frame.fill(l);
                }
                RouteMode::Right => {
                    let r = frame[1];
                    frame.fill(r);
                }
                RouteMode::Map(map) => {
                    self.scratch.copy_from_slice(frame);
                    for (dst, &src) in frame.iter_mut().zip(map) {
                        *dst = self.scratch[src];
                    }
                }
            }
        }
    }
}

/// Setting keys accepted by the `route` / `channels` filters.
pub const ROUTE_SETTINGS: &[&str] = &["mode", "map"];

/// Factory of the `route` / `channels` registry entries.
pub fn route_factory(p: &FilterParams<'_>) -> Result<Box<dyn AudioFilter>, FilterError> {
    let mode = match p.settings.get("mode") {
        None => "mono",
        Some(toml::Value::String(s)) => s.trim(),
        Some(_) => return Err(cfg_err("`mode` must be a string")),
    };
    let mode = match mode.to_ascii_lowercase().as_str() {
        "mono" | "downmix" => RouteMode::Mono,
        "swap" => RouteMode::Swap,
        "left" => RouteMode::Left,
        "right" => RouteMode::Right,
        "map" => {
            let Some(toml::Value::Array(items)) = p.settings.get("map") else {
                return Err(cfg_err(
                    "mode `map` needs `map = [source channel per output channel, ...]`",
                ));
            };
            let mut map = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    toml::Value::Integer(i) if *i >= 0 && *i < 32 => map.push(*i as usize),
                    _ => {
                        return Err(cfg_err(
                            "`map` entries must be channel indices between 0 and 31",
                        ));
                    }
                }
            }
            if map.is_empty() {
                return Err(cfg_err("`map` must not be empty"));
            }
            RouteMode::Map(map)
        }
        other => {
            return Err(cfg_err(format!(
                "unknown route mode `{other}` (expected mono, swap, left, right or map)"
            )));
        }
    };
    Ok(Box::new(Route::new(mode, p.channels)))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, freq: f64, sr: f64, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f64 / sr).sin() as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn table(src: &str) -> toml::Table {
        src.parse::<toml::Table>().unwrap()
    }

    fn params(settings: &toml::Table, sr: u32, ch: u8) -> FilterParams<'_> {
        FilterParams {
            sample_rate: sr,
            channels: ch,
            settings,
        }
    }

    #[test]
    fn biquad_zero_db_is_identity() {
        let input = sine(2048, 440.0, 44_100.0, 0.8);
        // Shortcut path.
        let c = BiquadCoeffs::peaking(44_100.0, 1000.0, 0.0, 1.0);
        assert!(c.is_identity());
        let mut st = BiquadState::default();
        for &x in &input {
            assert_eq!(st.process(&c, f64::from(x)) as f32, x);
        }
        // Full cookbook formula at 0 dB is also a pass-through.
        let c = BiquadCoeffs::peaking_unchecked(44_100.0, 1000.0, 0.0, 1.0);
        let mut st = BiquadState::default();
        for &x in &input {
            let y = st.process(&c, f64::from(x)) as f32;
            assert!((y - x).abs() < 1e-6, "{y} vs {x}");
        }
    }

    #[test]
    fn peaking_gain_at_centre_matches_setting() {
        let sr = 48_000.0;
        for gain in [-12.0, -3.0, 6.0, 12.0] {
            let c = BiquadCoeffs::peaking(sr, 1000.0, gain, 1.4);
            let at_centre = 20.0 * c.magnitude_at(sr, 1000.0).log10();
            assert!((at_centre - gain).abs() < 1e-6, "{at_centre} vs {gain}");
            // Far from the centre the band is (nearly) transparent.
            let far = 20.0 * c.magnitude_at(sr, 30.0).log10();
            assert!(far.abs() < 0.5, "{far}");
        }
    }

    #[test]
    fn equalizer_boosts_band_and_recomputes_on_rate_change() {
        let bands = vec![EqBand {
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }];
        let mut eq = Equalizer::new(bands, 0.0, 48_000, 1);
        let mut buf = sine(48_000, 1000.0, 48_000.0, 0.25);
        let before = rms(&buf[24_000..]);
        eq.apply(&mut buf);
        let ratio_db = 20.0 * (rms(&buf[24_000..]) / before).log10();
        assert!((ratio_db - 6.0).abs() < 0.2, "{ratio_db}");

        // Different sample rate: coefficients are recomputed and still
        // deliver +6 dB at 1 kHz.
        eq.set_format(96_000, 1);
        let mut buf = sine(96_000, 1000.0, 96_000.0, 0.25);
        let before = rms(&buf[48_000..]);
        eq.apply(&mut buf);
        let ratio_db = 20.0 * (rms(&buf[48_000..]) / before).log10();
        assert!((ratio_db - 6.0).abs() < 0.2, "{ratio_db}");
    }

    #[test]
    fn equalizer_flat_and_preamp() {
        let input = sine(512, 300.0, 44_100.0, 0.5);
        let mut flat = Equalizer::new(
            vec![EqBand {
                freq: 100.0,
                gain_db: 0.0,
                q: 1.0,
            }],
            0.0,
            44_100,
            2,
        );
        assert_eq!(flat.active_bands(), 0);
        let mut buf = input.clone();
        flat.apply(&mut buf);
        assert_eq!(buf, input);

        let mut pre = Equalizer::new(Vec::new(), -6.0, 44_100, 2);
        let mut buf = input.clone();
        pre.apply(&mut buf);
        let want = db_to_lin(-6.0) as f32;
        for (o, i) in buf.iter().zip(&input) {
            assert!((o - i * want).abs() < 1e-6);
        }
    }

    #[test]
    fn equalizer_channels_are_independent() {
        let bands = vec![EqBand {
            freq: 500.0,
            gain_db: 9.0,
            q: 0.7,
        }];
        let mut eq = Equalizer::new(bands, 0.0, 44_100, 2);
        let mono = sine(4096, 500.0, 44_100.0, 0.3);
        // Left carries the tone, right is silent.
        let mut buf: Vec<f32> = mono.iter().flat_map(|&s| [s, 0.0]).collect();
        eq.apply(&mut buf);
        assert!(buf.iter().skip(1).step_by(2).all(|&s| s == 0.0));
        assert!(buf.iter().step_by(2).any(|&s| s.abs() > 0.3));
    }

    #[test]
    fn equalizer_factory_parses_bands() {
        let t = table(
            "preamp_db = -3\nbands = [{ freq = 60, gain_db = 4.5, q = 0.7 }, { freq = 1000, gain_db = -2 }]",
        );
        let f = equalizer_factory(&params(&t, 44_100, 2)).unwrap();
        assert_eq!(f.name(), "equalizer");

        for bad in [
            "bands = 3",
            "bands = [3]",
            "bands = [{ freq = 100 }]",
            "bands = [{ freq = 100, gain_db = 1, q = 0 }]",
            "bands = [{ freq = 100, gain_db = 99 }]",
            "bands = [{ freq = 100, gain_db = 1, bogus = 1 }]",
            "preamp_db = 100",
        ] {
            assert!(
                equalizer_factory(&params(&table(bad), 44_100, 2)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn normalize_converges_to_target() {
        let sr = 8000u32;
        let params = NormalizeParams {
            target_db: -6.0,
            release_ms: 200.0,
            ..NormalizeParams::default()
        };
        let mut n = Normalize::new(params, sr, 1);
        let tone = sine(sr as usize / 2, 200.0, f64::from(sr), 0.1);
        let mut last = Vec::new();
        for _ in 0..10 {
            let mut chunk = tone.clone();
            n.apply(&mut chunk);
            last = chunk;
        }
        let peak = last.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let target = db_to_lin(-6.0) as f32;
        assert!(
            (peak - target).abs() / target < 0.05,
            "peak {peak} target {target}"
        );
        assert!(n.current_gain() > 4.0);
    }

    #[test]
    fn normalize_attenuates_loud_input() {
        let sr = 8000u32;
        let params = NormalizeParams {
            target_db: -6.0,
            ..NormalizeParams::default()
        };
        let mut n = Normalize::new(params, sr, 1);
        let tone = sine(sr as usize, 200.0, f64::from(sr), 1.0);
        let mut last = Vec::new();
        for _ in 0..4 {
            let mut chunk = tone.clone();
            n.apply(&mut chunk);
            last = chunk;
        }
        let peak = last.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak < 0.55 && peak > 0.45, "{peak}");
    }

    #[test]
    fn normalize_limiter_never_exceeds_ceiling() {
        let sr = 8000u32;
        let params = NormalizeParams {
            target_db: 0.0,
            ceiling_db: -6.0,
            attack_ms: 500.0, // sluggish AGC: the limiter must do the work
            ..NormalizeParams::default()
        };
        let ceiling = db_to_lin(-6.0) as f32;
        let mut n = Normalize::new(params, sr, 2);
        let mut buf: Vec<f32> = sine(sr as usize, 150.0, f64::from(sr), 0.95)
            .iter()
            .flat_map(|&s| [s, -s])
            .collect();
        n.apply(&mut buf);
        let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= ceiling * 1.0001, "{peak} > {ceiling}");
    }

    #[test]
    fn normalize_holds_gain_on_silence() {
        let mut n = Normalize::new(NormalizeParams::default(), 44_100, 2);
        let mut buf = vec![0.0f32; 4096];
        n.apply(&mut buf);
        assert!(buf.iter().all(|&s| s == 0.0));
        assert_eq!(n.current_gain(), 1.0);
    }

    #[test]
    fn route_mono_downmix_math() {
        let mut r = Route::new(RouteMode::Mono, 2);
        let mut buf = [1.0, 0.0, 0.5, -0.5, -1.0, -1.0];
        r.apply(&mut buf);
        assert_eq!(buf, [0.5, 0.5, 0.0, 0.0, -1.0, -1.0]);
    }

    #[test]
    fn route_swap_left_right_and_map() {
        let mut buf = [1.0, 2.0, 3.0, 4.0];
        Route::new(RouteMode::Swap, 2).apply(&mut buf);
        assert_eq!(buf, [2.0, 1.0, 4.0, 3.0]);

        let mut buf = [1.0, 2.0, 3.0, 4.0];
        Route::new(RouteMode::Left, 2).apply(&mut buf);
        assert_eq!(buf, [1.0, 1.0, 3.0, 3.0]);

        let mut buf = [1.0, 2.0, 3.0, 4.0];
        Route::new(RouteMode::Right, 2).apply(&mut buf);
        assert_eq!(buf, [2.0, 2.0, 4.0, 4.0]);

        // 3 channels: rotate (out0 <- in2, out1 <- in0, out2 <- in1).
        let mut buf = [10.0, 20.0, 30.0];
        Route::new(RouteMode::Map(vec![2, 0, 1]), 3).apply(&mut buf);
        assert_eq!(buf, [30.0, 10.0, 20.0]);
    }

    #[test]
    fn route_ineffective_layouts_pass_through() {
        let src = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut buf = src;
        Route::new(RouteMode::Swap, 6).apply(&mut buf);
        assert_eq!(buf, src);
        let mut buf = src;
        Route::new(RouteMode::Mono, 1).apply(&mut buf);
        assert_eq!(buf, src);
        let mut buf = src;
        Route::new(RouteMode::Map(vec![0, 1]), 6).apply(&mut buf);
        assert_eq!(buf, src);
        let mut buf = src;
        Route::new(RouteMode::Map(vec![0, 9]), 2).apply(&mut buf);
        assert_eq!(buf, src);
    }

    #[test]
    fn route_set_format_adapts_channel_count() {
        let mut r = Route::new(RouteMode::Swap, 6);
        r.set_format(44_100, 2);
        let mut buf = [1.0, 2.0];
        r.apply(&mut buf);
        assert_eq!(buf, [2.0, 1.0]);
    }

    #[test]
    fn route_factory_modes() {
        let ok = |src: &str| route_factory(&params(&table(src), 44_100, 2));
        assert!(ok("").is_ok());
        assert!(ok("mode = \"swap\"").is_ok());
        assert!(ok("mode = \"map\"\nmap = [1, 0]").is_ok());
        assert!(ok("mode = \"map\"").is_err());
        assert!(ok("mode = \"map\"\nmap = [-1]").is_err());
        assert!(ok("mode = \"bogus\"").is_err());
        assert!(ok("mode = 3").is_err());
    }

    #[test]
    fn normalize_factory_validates_ranges() {
        let ok = |src: &str| normalize_factory(&params(&table(src), 44_100, 2));
        assert!(ok("").is_ok());
        assert!(ok("target_db = -9\nmax_gain_db = 12").is_ok());
        assert!(ok("target_db = 3").is_err());
        assert!(ok("attack_ms = \"fast\"").is_err());
    }
}
