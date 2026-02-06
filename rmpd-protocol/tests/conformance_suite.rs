//! MPD Protocol Conformance Test Suite
//!
//! TCP-level tests that start a real rmpd server on a random port, connect
//! via TCP, send MPD protocol commands, and validate responses match the spec.
//!
//! Run with: cargo test --test conformance_suite -- --test-threads=1

mod common;
mod conformance;
