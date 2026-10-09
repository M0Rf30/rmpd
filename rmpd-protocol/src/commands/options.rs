// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Playback option command handlers (volume, repeat, random, etc.)

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{ACK_ERROR_ARG, ACK_ERROR_SYS};

/// Notify idle clients (subsystem `options`) and MPRIS that a playback option
/// changed (repeat/random/single/consume/crossfade/mixramp/replaygain).
fn notify_options(state: &AppState) {
    state
        .event_bus
        .emit(rmpd_core::event::Event::QueueOptionsChanged);
}

/// Apply `volume` through the engine's active mixers (software gain or the
/// outputs' hardware mixers) and return the volume the mixer reports back
/// (hardware may round to its own step size).
async fn apply_volume(state: &AppState, volume: u8) -> rmpd_core::error::Result<u8> {
    let mut engine = state.engine.write().await;
    engine.set_volume(volume).await?;
    Ok(engine.hardware_volume().unwrap_or(volume))
}

/// MPD's reply when no mixer can change the volume (`mixer_type = none`, or a
/// failing hardware mixer).
fn volume_error(command: &str, e: &rmpd_core::error::RmpdError) -> String {
    tracing::warn!("{command}: {e}");
    ResponseBuilder::error(ACK_ERROR_SYS, 0, command, "problems setting volume")
}

/// The volume `status`/`getvol` report: `None` when no enabled output has a
/// mixer (MPD omits the field then), the hardware mixer's level when one is
/// active (it can be moved by other applications), else the tracked software
/// volume.
pub async fn current_volume(state: &AppState) -> Option<u8> {
    {
        let engine = state.engine.read().await;
        if !engine.volume_available() {
            return None;
        }
        if let Some(v) = engine.hardware_volume() {
            return Some(v);
        }
    }
    Some(state.status.read().await.volume)
}

pub async fn handle_setvol_command(state: &AppState, volume: u8) -> String {
    match apply_volume(state, volume).await {
        Ok(actual) => {
            state.status.write().await.volume = actual;
            ResponseBuilder::new().ok()
        }
        Err(e) => volume_error("setvol", &e),
    }
}

pub async fn handle_volume_command(state: &AppState, change: i32) -> String {
    // The active mixer is the source of truth (a hardware mixer can be moved
    // by other applications); software volume lives in the status.
    let hardware = state.engine.read().await.hardware_volume();
    let current_vol = match hardware {
        Some(v) => v,
        None => state.status.read().await.volume,
    };
    let new_vol = (current_vol as i32 + change).clamp(0, 100) as u8;

    match apply_volume(state, new_vol).await {
        Ok(actual) => {
            state.status.write().await.volume = actual;
            ResponseBuilder::new().ok()
        }
        Err(e) => volume_error("volume", &e),
    }
}

pub async fn handle_repeat_command(state: &AppState, enabled: bool) -> String {
    state.status.write().await.repeat = enabled;
    notify_options(state);
    ResponseBuilder::new().ok()
}

pub async fn handle_random_command(state: &AppState, enabled: bool) -> String {
    state.status.write().await.random = enabled;
    // The decode thread reads this to pick track vs. album gain for
    // ReplayGainMode::Auto (mpd `ReplayGainMode::AUTO`): shuffled playback
    // breaks album context, so `auto` uses track gain while random is on.
    state.engine.write().await.set_random(enabled);
    notify_options(state);
    ResponseBuilder::new().ok()
}

pub async fn handle_single_command(state: &AppState, mode: &str) -> String {
    let single_mode = match mode {
        "0" => rmpd_core::state::SingleMode::Off,
        "1" => rmpd_core::state::SingleMode::On,
        "oneshot" => rmpd_core::state::SingleMode::Oneshot,
        _ => {
            return ResponseBuilder::error(
                ACK_ERROR_ARG,
                0,
                "single",
                "Unrecognized single mode, expected 0, 1, or oneshot",
            );
        }
    };
    state.status.write().await.single = single_mode;
    notify_options(state);
    ResponseBuilder::new().ok()
}

pub async fn handle_consume_command(state: &AppState, mode: &str) -> String {
    let consume_mode = match mode {
        "0" => rmpd_core::state::ConsumeMode::Off,
        "1" => rmpd_core::state::ConsumeMode::On,
        "oneshot" => rmpd_core::state::ConsumeMode::Oneshot,
        _ => {
            return ResponseBuilder::error(
                ACK_ERROR_ARG,
                0,
                "consume",
                "Unrecognized consume mode, expected 0, 1, or oneshot",
            );
        }
    };
    state.status.write().await.consume = consume_mode;
    notify_options(state);
    ResponseBuilder::new().ok()
}

pub async fn handle_crossfade_command(state: &AppState, seconds: u32) -> String {
    state.status.write().await.crossfade = seconds;
    state.engine.write().await.set_crossfade(seconds);
    notify_options(state);
    ResponseBuilder::new().ok()
}

pub async fn handle_mixrampdb_command(state: &AppState, decibels: f32) -> String {
    let delay = {
        let mut status = state.status.write().await;
        status.mixramp_db = decibels;
        status.mixramp_delay
    };
    state.engine.write().await.set_mixramp(decibels, delay);
    notify_options(state);
    ResponseBuilder::new().ok()
}

pub async fn handle_mixrampdelay_command(state: &AppState, seconds: f32) -> String {
    let db = {
        let mut status = state.status.write().await;
        status.mixramp_delay = seconds;
        status.mixramp_db
    };
    state.engine.write().await.set_mixramp(db, seconds);
    notify_options(state);
    ResponseBuilder::new().ok()
}

pub async fn handle_replaygain_mode_command(state: &AppState, mode: &str) -> String {
    match mode {
        "off" | "track" | "album" | "auto" => {
            state.status.write().await.replay_gain_mode =
                rmpd_core::state::ReplayGainMode::parse_mode(mode);
            notify_options(state);
            ResponseBuilder::new().ok()
        }
        _ => ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            "replay_gain_mode",
            "Unrecognized replay gain mode",
        ),
    }
}

pub async fn handle_replaygain_status_command(state: &AppState) -> String {
    let mode = state.status.read().await.replay_gain_mode.to_string();
    let mut resp = ResponseBuilder::new();
    resp.field("replay_gain_mode", &mode);
    resp.ok()
}
