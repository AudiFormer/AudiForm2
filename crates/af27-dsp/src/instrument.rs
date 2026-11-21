// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Instruments that run elsewhere (VST3 plug-ins): they take note and
//! controller events placed within a block and give back stereo sound.
//! Another crate builds them (registered with
//! [`register_instrument_factory`]), as for plug-in effects.

/// One event for an instrument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteEvent {
    /// Velocity 0..1; `tuning` in cents off the key.
    On {
        pitch: u8,
        velocity: f32,
        channel: u8,
        tuning: f32,
    },
    Off {
        pitch: u8,
        channel: u8,
    },
    /// 0..127 as in MIDI, 128 channel aftertouch, 129 pitch bend (as VST3
    /// numbers them); value 0..1 (pitch bend: 0.5 is the middle).
    Control {
        controller: u8,
        value: f32,
        channel: u8,
    },
}

/// An event at a frame of the block it is handed with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timed {
    pub offset: u32,
    pub event: NoteEvent,
}

/// What an instrument copy is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// The session's playback (and, offline, exports).
    Playback,
    /// Played from a MIDI keyboard: a copy of its own, kept apart from
    /// playback's, with as little buffering as it can.
    Keys,
}

pub trait Instrument: Send {
    /// Renders one block: `events` (sorted by offset, each inside the
    /// block) go in, its sound is written over `l` and `r` (the same
    /// length). Must not allocate or wait (live), except offline.
    fn render(&mut self, events: &[Timed], l: &mut [f32], r: &mut [f32]);
    /// How many frames late its sound is after the events that make it.
    fn latency(&self) -> usize {
        0
    }
    /// Stops every note and clears tails (a jump, a start).
    fn reset(&mut self) {}
    /// This copy renders offline (an export): it may take its time on
    /// each block but must not leave any out. Called once, off the audio
    /// thread.
    fn set_offline(&mut self, _offline: bool) {}
    /// Offline, before rendering: lets an instrument that loads its
    /// sounds in the background (a sampler) get them in. Blocks.
    fn warm_up(&mut self) {}
    /// Whether it takes controller `c` (a sustain pedal it does not take
    /// is held by the player instead, by holding notes).
    fn takes(&self, _controller: u8) -> bool {
        true
    }
}

/// Builds an instrument for a plug-in slot (None when it is not there).
pub type InstrumentFactory =
    fn(&af27_core::EffectSlot, u32, Use) -> Option<Box<dyn Instrument>>;

static FACTORY: std::sync::OnceLock<InstrumentFactory> = std::sync::OnceLock::new();

pub fn register_instrument_factory(f: InstrumentFactory) {
    let _ = FACTORY.set(f);
}

/// The instrument for a slot, from the registered factory.
pub fn build_instrument(
    slot: &af27_core::EffectSlot,
    rate: u32,
    use_: Use,
) -> Option<Box<dyn Instrument>> {
    FACTORY.get().and_then(|f| f(slot, rate, use_))
}
