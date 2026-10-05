// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! MPD Protocol Conformance Test Suite
//!
//! TCP-level tests that start a real rmpd server on a random port, connect
//! via TCP, send MPD protocol commands, and validate responses match the spec.
//!
//! Run with: cargo test --test conformance_suite -- --test-threads=1

mod common;
mod conformance;
#[path = "common/tcp_harness.rs"]
mod tcp_harness;
