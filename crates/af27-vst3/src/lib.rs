// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! VST3 plug-in hosting for AudiForm 2.
//!
//! Plug-ins never run inside the app. Each runs in a host process (the
//! app's own executable started again with `--plugin-host`), by default
//! one process per maker, so a plug-in that crashes or hangs takes down
//! only its process: the app hears the pipe close and plays on without
//! it. Bundles are read by scan processes (`--plugin-scan <bundle>`).
//!
//! - [`protocol`]: frames and messages between the app and a host.
//! - [`host`]: the code that runs inside host and scan processes.
//! - [`client`]: the app's side of a host process.
//! - [`registry`]: the scanned plug-ins (`plugins.json`) and the scanner.
//! - [`live`]: plug-ins as effects in a chain, without ever blocking the
//!   audio thread, and the pool of running plug-ins.

pub mod client;
pub mod host;
pub mod live;
pub mod protocol;
pub mod registry;

pub use live::{build_live, manager, Manager, PIPELINE_DEFAULT};
pub use registry::{Found, Registry};

/// The effect id prefix of a plug-in slot: `vst3:<32 hex class id>`.
pub const PREFIX: &str = "vst3:";

pub fn is_plugin(effect: &str) -> bool {
    effect.starts_with(PREFIX)
}

/// The class id of a plug-in slot's effect id.
pub fn cid_of(effect: &str) -> Option<&str> {
    effect.strip_prefix(PREFIX)
}

/// Runs a host or scan process when the command line asks for one, and
/// returns its exit code; None for a normal start of the app.
pub fn main_with_args(args: &[String]) -> Option<i32> {
    match args.get(1).map(String::as_str) {
        Some("--plugin-host") => {
            host::init_com();
            Some(host::serve::serve())
        }
        Some("--plugin-scan") => {
            host::init_com();
            let path = args.get(2).map_or("", String::as_str);
            Some(host::scan::scan(std::path::Path::new(path)))
        }
        _ => None,
    }
}
