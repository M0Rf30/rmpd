//! Playback option command handlers (volume, repeat, random, etc.)

use crate::response::ResponseBuilder;
use crate::state::AppState;

pub async fn handle_setvol_command(state: &AppState, volume: u8) -> String {
    match state.engine.write().await.set_volume(volume).await {
        Ok(_) => {
            let mut status = state.status.write().await;
            status.volume = volume;
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "setvol", &format!("Volume error: {}", e)),
    }
}

pub async fn handle_volume_command(state: &AppState, change: i8) -> String {
    let current_vol = state.status.read().await.volume;
    let new_vol = (current_vol as i16 + change as i16).clamp(0, 100) as u8;

    match state.engine.write().await.set_volume(new_vol).await {
        Ok(_) => {
            state.status.write().await.volume = new_vol;
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "volume", &format!("Volume error: {}", e)),
    }
}

pub async fn handle_repeat_command(state: &AppState, enabled: bool) -> String {
    state.status.write().await.repeat = enabled;
    ResponseBuilder::new().ok()
}

pub async fn handle_random_command(state: &AppState, enabled: bool) -> String {
    state.status.write().await.random = enabled;
    ResponseBuilder::new().ok()
}

pub async fn handle_single_command(state: &AppState, mode: &str) -> String {
    let single_mode = match mode {
        "0" => rmpd_core::state::SingleMode::Off,
        "1" => rmpd_core::state::SingleMode::On,
        "oneshot" => rmpd_core::state::SingleMode::Oneshot,
        _ => return ResponseBuilder::error(2, 0, "single", "Invalid mode"),
    };
    state.status.write().await.single = single_mode;
    ResponseBuilder::new().ok()
}

pub async fn handle_consume_command(state: &AppState, mode: &str) -> String {
    let consume_mode = match mode {
        "0" => rmpd_core::state::ConsumeMode::Off,
        "1" => rmpd_core::state::ConsumeMode::On,
        "oneshot" => rmpd_core::state::ConsumeMode::Oneshot,
        _ => return ResponseBuilder::error(2, 0, "consume", "Invalid mode"),
    };
    state.status.write().await.consume = consume_mode;
    ResponseBuilder::new().ok()
}

pub async fn handle_crossfade_command(state: &AppState, seconds: u32) -> String {
    state.status.write().await.crossfade = seconds;
    ResponseBuilder::new().ok()
}

pub async fn handle_mixrampdb_command(state: &AppState, decibels: f32) -> String {
    let mut status = state.status.write().await;
    status.mixramp_db = decibels;
    ResponseBuilder::new().ok()
}

pub async fn handle_mixrampdelay_command(state: &AppState, seconds: f32) -> String {
    let mut status = state.status.write().await;
    status.mixramp_delay = seconds;
    ResponseBuilder::new().ok()
}

pub async fn handle_replaygain_mode_command(state: &AppState, mode: &str) -> String {
    // Set ReplayGain mode (off, track, album, auto)
    // Store in player status or engine config
    let _ = (state, mode);
    ResponseBuilder::new().ok()
}

pub async fn handle_replaygain_status_command(state: &AppState) -> String {
    // Return current ReplayGain status
    let _ = state;
    let mut resp = ResponseBuilder::new();
    resp.field("replay_gain_mode", "off");
    resp.ok()
}
