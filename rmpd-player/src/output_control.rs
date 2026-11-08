// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-output software gain slots of one engine (the control block itself, `OutputControl`, lives in `playcore::control`).

use parking_lot::Mutex;
use playcore::control::OutputControl;
use std::sync::Arc;

/// Per-output software gain slots of one engine.
///
/// Every output gets a control block (see [`OutputControl::with_own_gain`])
/// sharing pause/flush/played-frames with the engine but with its own gain:
/// outputs on the software mixer follow the requested volume, hardware /
/// `none`-mixer outputs stay at unity so the device mixer is the only
/// attenuation.
#[derive(Debug, Default)]
pub struct OutputGains {
    slots: Mutex<Vec<GainSlot>>,
}

#[derive(Debug)]
struct GainSlot {
    software: bool,
    control: Arc<OutputControl>,
}

impl OutputGains {
    /// Forget all registered outputs (call before (re)building the outputs).
    pub fn clear(&self) {
        self.slots.lock().clear();
    }

    /// Create and register the control block for one output. A software
    /// output is seeded with the master's current gain, read while holding
    /// the slots lock so a concurrent [`Self::set_software_gain`] (which
    /// sets the master first, then takes the lock) can never be missed.
    pub fn register(&self, master: &OutputControl, software: bool) -> Arc<OutputControl> {
        let mut slots = self.slots.lock();
        let control = Arc::new(master.with_own_gain(if software { master.gain() } else { 1.0 }));
        slots.push(GainSlot {
            software,
            control: control.clone(),
        });
        control
    }

    /// Set the software gain of every software-mixer output.
    pub fn set_software_gain(&self, gain: f32) {
        for slot in self.slots.lock().iter().filter(|s| s.software) {
            slot.control.set_gain(gain);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_gains_only_move_software_outputs() {
        let master = OutputControl::new();
        master.set_gain(0.4);
        let gains = OutputGains::default();
        let soft = gains.register(&master, true);
        let hard = gains.register(&master, false);
        assert!((soft.gain() - 0.4).abs() < f32::EPSILON);
        assert!((hard.gain() - 1.0).abs() < f32::EPSILON);
        gains.set_software_gain(0.8);
        assert!((soft.gain() - 0.8).abs() < f32::EPSILON);
        assert!((hard.gain() - 1.0).abs() < f32::EPSILON);
    }
}
