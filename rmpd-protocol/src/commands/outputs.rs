//! Audio output control command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::ACK_ERROR_SYSTEM;

/// Reconcile the engine's active output after an enabled-flag change.
/// Picks the first enabled output and calls set_active_output; if none are
/// enabled, stops playback.
async fn reconcile_active_output(state: &AppState) {
    let first = {
        let outputs = state.outputs.read().await;
        outputs
            .iter()
            .find(|o| o.enabled)
            .and_then(|o| o.config.clone())
    };
    match first {
        Some(cfg) => {
            state.engine.write().await.set_active_output(cfg);
        }
        None => {
            let _ = state.engine.write().await.stop().await;
        }
    }
}

pub async fn handle_outputs_command(state: &AppState) -> String {
    let outputs = state.outputs.read().await;
    let mut resp = ResponseBuilder::new();

    for (i, output) in outputs.iter().enumerate() {
        resp.field("outputid", output.id);
        resp.field("outputname", &output.name);
        resp.field("plugin", &output.plugin);
        resp.field("outputenabled", if output.enabled { "1" } else { "0" });
        for (key, value) in &output.attributes {
            resp.field("attribute", format!("{key}={value}"));
        }
        // Add blank line between outputs, but not after the last one
        if i < outputs.len() - 1 {
            resp.blank_line();
        }
    }

    resp.ok()
}

pub async fn handle_enableoutput_command(state: &AppState, id: u32) -> String {
    let found = {
        let mut outputs = state.outputs.write().await;
        if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
            output.enabled = true;
            true
        } else {
            false
        }
    };

    if found {
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        reconcile_active_output(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "enableoutput", "No such audio output")
    }
}

pub async fn handle_disableoutput_command(state: &AppState, id: u32) -> String {
    let found = {
        let mut outputs = state.outputs.write().await;
        if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
            output.enabled = false;
            true
        } else {
            false
        }
    };

    if found {
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        reconcile_active_output(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "disableoutput", "No such audio output")
    }
}

pub async fn handle_toggleoutput_command(state: &AppState, id: u32) -> String {
    let found = {
        let mut outputs = state.outputs.write().await;
        if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
            output.enabled = !output.enabled;
            true
        } else {
            false
        }
    };

    if found {
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        reconcile_active_output(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "toggleoutput", "No such audio output")
    }
}

pub async fn handle_outputset_command(
    state: &AppState,
    id: u32,
    name: &str,
    value: &str,
) -> String {
    let mut outputs = state.outputs.write().await;
    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output
            .attributes
            .insert(name.to_string(), value.to_string());
        drop(outputs);
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "outputset", "No such audio output")
    }
}
