//! Audio output control command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{ACK_ERROR_ARG, ACK_ERROR_NO_EXIST};

/// Reconcile the engine's active output set after an enabled-flag change.
/// Feeds the engine ALL enabled outputs; if none are enabled, stops playback.
async fn reconcile_active_output(state: &AppState) {
    let enabled: Vec<rmpd_core::config::OutputConfig> = {
        let outputs = state.outputs.read().await;
        outputs
            .iter()
            .filter(|o| o.enabled)
            .filter_map(|o| o.config.clone())
            .collect()
    };
    if enabled.is_empty() {
        let _ = state.engine.write().await.stop().await;
    } else {
        state.engine.write().await.set_outputs(enabled);
    }
}

pub async fn handle_outputs_command(state: &AppState, current_partition: &str) -> String {
    let outputs = state.outputs.read().await;
    let mut resp = ResponseBuilder::new();

    for output in outputs
        .iter()
        .filter(|o| o.partition.as_deref() == Some(current_partition))
    {
        resp.field("outputid", output.id);
        resp.field("outputname", &output.name);
        resp.field("plugin", &output.plugin);
        resp.field("outputenabled", if output.enabled { "1" } else { "0" });
        for (key, value) in &output.attributes {
            resp.field("attribute", format!("{key}={value}"));
        }
    }

    resp.ok()
}

pub async fn handle_enableoutput_command(
    state: &AppState,
    current_partition: &str,
    id: u32,
) -> String {
    let found = {
        let mut outputs = state.outputs.write().await;
        if let Some(output) = outputs
            .iter_mut()
            .find(|o| o.id == id && o.partition.as_deref() == Some(current_partition))
        {
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
        ResponseBuilder::error(
            ACK_ERROR_NO_EXIST,
            0,
            "enableoutput",
            "No such audio output",
        )
    }
}

pub async fn handle_disableoutput_command(
    state: &AppState,
    current_partition: &str,
    id: u32,
) -> String {
    let found = {
        let mut outputs = state.outputs.write().await;
        if let Some(output) = outputs
            .iter_mut()
            .find(|o| o.id == id && o.partition.as_deref() == Some(current_partition))
        {
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
        ResponseBuilder::error(
            ACK_ERROR_NO_EXIST,
            0,
            "disableoutput",
            "No such audio output",
        )
    }
}

pub async fn handle_toggleoutput_command(
    state: &AppState,
    current_partition: &str,
    id: u32,
) -> String {
    let found = {
        let mut outputs = state.outputs.write().await;
        if let Some(output) = outputs
            .iter_mut()
            .find(|o| o.id == id && o.partition.as_deref() == Some(current_partition))
        {
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
        ResponseBuilder::error(
            ACK_ERROR_NO_EXIST,
            0,
            "toggleoutput",
            "No such audio output",
        )
    }
}

/// MPD's IsValidAttributeName: non-empty, alphanumeric plus '_'.
fn is_valid_attribute_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub async fn handle_outputset_command(
    state: &AppState,
    current_partition: &str,
    id: u32,
    name: &str,
    value: &str,
) -> String {
    if !is_valid_attribute_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "outputset", "Illegal attribute name");
    }

    let mut outputs = state.outputs.write().await;
    if let Some(output) = outputs
        .iter_mut()
        .find(|o| o.id == id && o.partition.as_deref() == Some(current_partition))
    {
        output
            .attributes
            .insert(name.to_string(), value.to_string());
        drop(outputs);
        state
            .event_bus
            .emit(rmpd_core::event::Event::OutputsChanged);
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "outputset", "No such audio output")
    }
}
