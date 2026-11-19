// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Signal processing for Audiform27. Everything here is written from
//! published, textbook descriptions (see docs/clean-room.md):
//! RBJ biquads, ITU-R BS.1770 loudness, a Schroeder-style reverb with
//! Freeverb's public-domain tunings, windowed-sinc resampling and STFT
//! spectral noise reduction.

pub mod biquad;
pub mod effects;
pub mod fft;
pub mod generate;
pub mod live;
pub mod loudness;
pub mod noise;
pub mod peaks;
pub mod phase;
pub mod resample;
pub mod reverb;
pub mod spectral_edit;
pub mod wide;

pub use effects::{Effect, ParamSpec};
pub use peaks::Peaks;
