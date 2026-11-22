// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Signal processing for Audiform2. Everything here is written from
//! published, textbook descriptions (see docs/clean-room.md):
//! RBJ biquads, ITU-R BS.1770 loudness, a Schroeder-style reverb with
//! Freeverb's public-domain tunings, windowed-sinc resampling and STFT
//! spectral noise reduction, and a phase-locked phase vocoder for time
//! stretch and pitch shift (Laroche & Dolson).

pub mod biquad;
pub mod effects;
pub mod fft;
pub mod generate;
pub mod instrument;
pub mod live;
pub mod loudness;
pub mod noise;
pub mod peaks;
pub mod phase;
pub mod repair;
pub mod resample;
pub mod reverb;
pub mod spectral_edit;
pub mod stretch;
pub mod wide;

pub use effects::{Effect, ParamSpec};
pub use peaks::Peaks;
