//! Partition commands - Multi-queue support
//!
//! MPD supports multiple queues called "partitions" to allow independent playback
//! contexts. This module handles partition management commands.
//!
//! IMPLEMENTATION STATUS: Stubs only
//! Full partition support requires major architectural changes:
//! - Multiple AppState instances (one per partition)
//! - Per-partition queues, players, and outputs
//! - Client session partition tracking
//! - Output migration between partitions

use super::{AppState, ResponseBuilder};

/// Switch to a specific partition
///
/// Stub: Only "default" partition exists. Returns error for non-existent partitions.
pub async fn handle_partition_command(_state: &AppState, name: &str) -> String {
    if name == "default" {
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "partition", "No such partition")
    }
}

/// List all available partitions
///
/// Currently only the default partition exists
pub async fn handle_listpartitions_command() -> String {
    // List partitions - only default for now
    let mut resp = ResponseBuilder::new();
    resp.field("partition", "default");
    resp.ok()
}

/// Create a new partition
///
/// Stub: Partition creation requires major architectural changes. Returns error.
pub async fn handle_newpartition_command(_name: &str) -> String {
    ResponseBuilder::error(50, 0, "newpartition", "Partition creation not implemented")
}

/// Delete an existing partition
///
/// Stub: Returns error. Cannot delete default partition.
pub async fn handle_delpartition_command(name: &str) -> String {
    if name == "default" {
        ResponseBuilder::error(50, 0, "delpartition", "Cannot delete default partition")
    } else {
        ResponseBuilder::error(50, 0, "delpartition", "No such partition")
    }
}

/// Move an output to the current partition
///
/// Stub: Output migration not implemented. Returns error.
pub async fn handle_moveoutput_command(_name: &str) -> String {
    ResponseBuilder::error(50, 0, "moveoutput", "Output migration not implemented")
}
