// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The effects, each in its own module, written against
//! [`crate::framework`] (re-exported here so they can say `super::num`).

pub use crate::framework::*;
pub use crate::info;

pub mod amp;
pub mod center;
pub mod mixer;
pub mod modulation;
pub mod phase_curve;
pub mod reverb;
pub mod tube;
pub mod verb;
pub mod vocal;
pub mod width;
