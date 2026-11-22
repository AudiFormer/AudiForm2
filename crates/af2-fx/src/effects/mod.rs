// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The effects, each in its own module, written against
//! [`crate::framework`] (re-exported here so they can say `super::num`).

pub use crate::framework::*;
pub use crate::info;

pub mod amp;
pub mod amp_rig;
pub mod band_eq;
pub mod center;
pub mod center_remover;
pub mod curve_eq;
pub mod dynamics;
pub mod echo;
pub mod filters;
pub mod mixer;
pub mod modulation;
pub mod parametric_eq;
pub mod phase_curve;
pub mod phase_rotate;
pub mod pitch;
pub mod restore;
pub mod reverb;
pub mod speech;
pub mod studio;
pub mod tube;
pub mod tube_master;
pub mod verb;
pub mod vocal;
pub mod width;
