//! List cpal output devices. The `id` column is what you pass to
//! `RMPD_AUDIO_DEVICE` (e.g. a raw ALSA `hw:CARD=...,DEV=0` for bit-perfect
//! DoP/DSD, bypassing PipeWire/PulseAudio).
//!
//! Run: `cargo run -p rmpd-player --example list_devices`
use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(devices) => {
            println!("{:<28} description", "RMPD_AUDIO_DEVICE id");
            for d in devices {
                let id = d
                    .id()
                    .map(|i| i.id().to_owned())
                    .unwrap_or_else(|_| "<unknown>".to_owned());
                println!("{id:<28} {d}");
            }
        }
        Err(e) => eprintln!("error listing devices: {e}"),
    }
}
