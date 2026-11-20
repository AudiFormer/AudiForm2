// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Core types shared by every part of Audiform27: planar audio buffers,
//! the multitrack session model, and a generic undo history.

pub mod buffer;
pub mod history;
pub mod midi;
pub mod noteops;
pub mod quantize;
pub mod session;
pub mod space;
pub mod time;

pub use buffer::{AudioBuffer, Range};
pub use history::History;
pub use session::{
    Clip, ClipId, ClipProps, ClipStretch, EffectSlot, Marker, MediaId, MediaItem, Rack, Send,
    Session, StretchKind, StretchMode, Track, TrackId, TrackKind,
};
