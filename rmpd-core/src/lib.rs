// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::cargo_common_metadata)]

pub mod config;
pub mod discovery;
pub mod error;
pub mod event;
pub mod filter;
pub mod history;
pub mod messaging;
pub mod partition;
pub mod path;
pub mod playback;
pub mod queue;
pub mod song;
pub mod state;
pub mod storage;
pub mod tag;
pub mod time;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
