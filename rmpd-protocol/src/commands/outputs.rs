//! Audio output control command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

pub async fn handle_outputs_command(state: &AppState) -> String {
    let outputs = state.outputs.read().await;
    let mut resp = ResponseBuilder::new();

    for (i, output) in outputs.iter().enumerate() {
        resp.field("outputid", output.id);
        resp.field("outputname", &output.name);
        resp.field("plugin", &output.plugin);
        resp.field("outputenabled", if output.enabled { "1" } else { "0" });
        if let Some(partition) = &output.partition {
            resp.field("partition", partition);
        }
        // Add blank line between outputs, but not after the last one
        if i < outputs.len() - 1 {
            resp.blank_line();
        }
    }

    resp.ok()
}

pub async fn handle_enableoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = true;
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "enableoutput", "No such output")
    }
}

pub async fn handle_disableoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = false;
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "disableoutput", "No such output")
    }
}

pub async fn handle_toggleoutput_command(state: &AppState, id: u32) -> String {
    let mut outputs = state.outputs.write().await;

    if let Some(output) = outputs.iter_mut().find(|o| o.id == id) {
        output.enabled = !output.enabled;
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "toggleoutput", "No such output")
    }
}

pub async fn handle_outputset_command(
    state: &AppState,
    id: u32,
    _name: &str,
    _value: &str,
) -> String {
    // Verify output exists
    let outputs = state.outputs.read().await;
    if outputs.iter().any(|o| o.id == id) {
        // For now, just acknowledge - actual attribute setting would be implemented
        // when we have configurable output properties
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "outputset", "No such output")
    }
}
