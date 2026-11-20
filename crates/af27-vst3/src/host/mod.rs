// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The plug-in side: everything that runs inside a host or scan process
//! and calls into plug-ins. The only unsafe code in AudiForm 2's plug-in
//! hosting lives here.

// The SDK's enum types differ between systems, so some casts are
// needed on one and not another.
#![allow(clippy::missing_safety_doc, clippy::unnecessary_cast)]

pub mod com;
pub mod editor;
pub mod module;
pub mod plugin;
pub mod scan;
pub mod serve;

/// Sets up COM (and OLE, for drag and drop in plug-in windows) on the
/// calling thread.
pub fn init_com() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Ole::OleInitialize(std::ptr::null_mut());
    }
}
