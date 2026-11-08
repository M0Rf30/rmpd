// SPDX-License-Identifier: MIT OR Apache-2.0

//! Config layer (TOML settings → filter) of the DSP filters in [`playcore::dsp`], registered in
//! the [`crate::filter`] registry.
//!
//! | Registry name         | Type                        | Purpose                                    |
//! |-----------------------|-----------------------------|--------------------------------------------|
//! | `normalize`           | [`playcore::dsp::Normalize`] | AGC / volume normalization with limiter   |
//! | `equalizer`           | [`playcore::dsp::Equalizer`] | N-band peaking EQ (RBJ biquads) + preamp  |
//! | `route`, `channels`   | [`playcore::dsp::Route`]     | stereo→mono downmix, swap, channel remap  |
//!
//! Each factory validates its `[[filter]]` settings and builds the playcore filter for the
//! stream format it is handed.

use crate::filter::{FilterError, FilterParams};
use playcore::dsp::{
    EqBand, Equalizer, MAX_EQ_BANDS, Normalize, NormalizeParams, Route, RouteMode,
};
use playcore::filter::AudioFilter;

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

#[cfg(test)]
mod tests {
    use super::*;

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
