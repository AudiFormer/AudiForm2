// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! Core types shared by every part of Audiform27: planar audio buffers,
//! the multitrack session model, and a generic undo history.

pub mod buffer;
pub mod history;
pub mod session;
pub mod time;

pub use buffer::{AudioBuffer, Range};
pub use history::History;
pub use session::{Clip, ClipId, Marker, MediaId, MediaItem, Session, Track, TrackId};
