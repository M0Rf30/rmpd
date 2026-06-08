//! List cpal output devices with their capabilities. The `id` column is what
//! you pass to `RMPD_AUDIO_DEVICE` / `audio.device` (e.g. a raw ALSA
//! `hw:CARD=...,DEV=0` for bit-perfect DoP/DSD, bypassing PipeWire/PulseAudio).
//!
//! Run: `cargo run -p rmpd-player --example list_devices`
use std::collections::BTreeSet;

use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        eprintln!("no output devices");
        return;
    };
    println!("{:<26} capabilities  |  description", "id");
    for device in devices {
        let id = device
            .id()
            .map(|i| i.id().to_owned())
            .unwrap_or_else(|_| "<unknown>".to_owned());

        let caps = match device.supported_output_configs() {
            Ok(configs) => {
                let (mut min, mut max, mut ch) = (u32::MAX, 0u32, 0u16);
                let mut formats = BTreeSet::new();
                for c in configs {
                    min = min.min(c.min_sample_rate());
                    max = max.max(c.max_sample_rate());
                    ch = ch.max(c.channels());
                    formats.insert(format!("{:?}", c.sample_format()));
                }
                if max == 0 {
                    "(no output configs)".to_owned()
                } else {
                    let fmts: Vec<_> = formats.into_iter().collect();
                    format!("{min}-{max} Hz, {ch}ch, {}", fmts.join("/"))
                }
            }
            Err(e) => format!("(unavailable: {e})"),
        };

        println!("{id:<26} {caps}  |  {device}");
    }
}
