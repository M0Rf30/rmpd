//! Partition commands - Multi-queue support
//!
//! MPD supports multiple queues called "partitions" to allow independent playback
//! contexts. This module handles partition management commands.
//!
//! IMPLEMENTATION STATUS: Core commands implemented
//! - newpartition: Create new partitions ✅
//! - delpartition: Delete partitions (except default) ✅
//! - partition: Switch to a partition ✅
//! - listpartitions: List all partitions ✅
//! - moveoutput: Move output to partition ✅
//!
//! Note: Command handlers still need updating to use partition context

use super::utils::ACK_ERROR_SYSTEM;
use super::{AppState, ResponseBuilder};
use crate::connection::ConnectionState;
use tracing::info;

/// Switch to a specific partition
///
/// Changes the client's current partition. All subsequent commands will
/// operate within this partition context.
///
/// Returns:
/// - OK if partition exists
/// - ACK `[50@0]` {partition} No such partition
pub async fn handle_partition_command(
    state: &AppState,
    conn_state: &mut ConnectionState,
    name: &str,
) -> String {
    // Check if partition manager is available
    let manager = match &state.partition_manager {
        Some(m) => m,
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "partition",
                "Partition support not initialized",
            );
        }
    };

    // Check if partition exists
    if manager.get_partition(name).await.is_some() {
        info!("client switching to partition: {}", name);
        conn_state.current_partition = name.to_string();
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "partition", "No such partition")
    }
}

/// List all available partitions
///
/// Returns a list of all partition names.
///
/// Response format:
/// ```text
/// partition: default
/// partition: bedroom
/// partition: kitchen
/// OK
/// ```
pub async fn handle_listpartitions_command(state: &AppState) -> String {
    let manager = match &state.partition_manager {
        Some(m) => m,
        None => {
            // If no partition manager, return just default
            let mut resp = ResponseBuilder::new();
            resp.field("partition", "default");
            return resp.ok();
        }
    };

    let partitions = manager.list_partitions().await;

    let mut resp = ResponseBuilder::new();
    for name in partitions {
        resp.field("partition", &name);
    }

    resp.ok()
}

/// Create a new partition
///
/// Creates a new independent playback context with its own queue,
/// player status, and output assignments.
///
/// Returns:
/// - OK if partition created successfully
/// - ACK `[50@0]` {newpartition} Partition already exists
/// - ACK `[50@0]` {newpartition} Invalid partition name
pub async fn handle_newpartition_command(state: &AppState, name: &str) -> String {
    // Validate partition name
    if name.is_empty() {
        return ResponseBuilder::error(
            ACK_ERROR_SYSTEM,
            0,
            "newpartition",
            "Invalid partition name",
        );
    }

    // Check for invalid characters
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return ResponseBuilder::error(
            ACK_ERROR_SYSTEM,
            0,
            "newpartition",
            "Invalid partition name",
        );
    }

    let manager = match &state.partition_manager {
        Some(m) => m,
        None => {
            return ResponseBuilder::error(
                50,
                0,
                "newpartition",
                "Partition support not initialized",
            );
        }
    };

    match manager.create_partition(name.to_string()).await {
        Ok(_) => {
            info!("created new partition: {}", name);
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "newpartition", &e),
    }
}

/// Delete an existing partition
///
/// Deletes a partition and all its associated state. Cannot delete the
/// default partition. Clients in the deleted partition are moved to default.
///
/// Returns:
/// - OK if partition deleted successfully
/// - ACK `[50@0]` {delpartition} Cannot delete default partition
/// - ACK `[50@0]` {delpartition} No such partition
pub async fn handle_delpartition_command(state: &AppState, name: &str) -> String {
    let manager = match &state.partition_manager {
        Some(m) => m,
        None => {
            return ResponseBuilder::error(
                50,
                0,
                "delpartition",
                "Partition support not initialized",
            );
        }
    };

    match manager.delete_partition(name).await {
        Ok(_) => {
            info!("deleted partition: {}", name);
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "delpartition", &e),
    }
}

/// Move an output to the current partition
///
/// Transfers ownership of an output from its current partition to the
/// client's current partition. The output will play audio from the new
/// partition's queue.
///
/// Returns:
/// - OK if output moved successfully
/// - ACK `[50@0]` {moveoutput} No such output
/// - ACK `[50@0]` {moveoutput} Output move failed
pub async fn handle_moveoutput_command(
    state: &AppState,
    conn_state: &ConnectionState,
    output_name: &str,
) -> String {
    let manager = match &state.partition_manager {
        Some(m) => m,
        None => {
            return ResponseBuilder::error(
                50,
                0,
                "moveoutput",
                "Partition support not initialized",
            );
        }
    };

    // Find output by name
    let outputs = state.outputs.read().await;
    let output = outputs.iter().find(|o| o.name == output_name);

    let (output_id, current_partition) = match output {
        Some(o) => (o.id, o.partition.clone()),
        None => {
            return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "moveoutput", "No such output");
        }
    };

    let to = &conn_state.current_partition;

    // Check if output is already assigned to target partition (via OutputInfo)
    if current_partition.as_deref() == Some(to.as_str()) {
        return ResponseBuilder::new().ok();
    }

    // Determine source partition (currently we assume it's in some partition)
    // For now, we'll try to find which partition has this output
    let partitions = manager.list_partitions().await;
    let mut source_partition = None;

    for part_name in &partitions {
        if let Some(partition) = manager.get_partition(part_name).await {
            let assigned_outputs = partition.get_outputs().await;
            if assigned_outputs.contains(&output_id) {
                source_partition = Some(part_name.clone());
                break;
            }
        }
    }

    // If source partition is found, perform the move
    let move_result = if let Some(found_partition) = source_partition {
        // Output is assigned to a known partition, do full move
        let from = found_partition;
        info!("moving output from '{}' to '{}'", from, to);
        manager.move_output(output_id, &from, to).await
    } else {
        // Output is not assigned to any partition yet, just assign to target
        info!("assigning unassigned output to '{}'", to);
        let target = manager.get_partition(to).await;
        if let Some(target_partition) = target {
            target_partition.assign_output(output_id).await;
            Ok(())
        } else {
            Err(format!("Target partition not found: {}", to))
        }
    };

    match move_result {
        Ok(_) => {
            // Update OutputInfo to reflect new partition ownership
            drop(outputs); // Release read lock before acquiring write lock
            {
                let mut outputs_mut = state.outputs.write().await;
                if let Some(output) = outputs_mut.iter_mut().find(|o| o.id == output_id) {
                    output.partition = Some(to.clone());
                }
            }

            info!("moved output '{}' to partition '{}'", output_name, to);
            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(
            ACK_ERROR_SYSTEM,
            0,
            "moveoutput",
            &format!("Output move failed: {}", e),
        ),
    }
}
