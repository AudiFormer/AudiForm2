// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! A stand-alone plug-in host, for this crate's tests. The app hosts
//! plug-ins by starting itself again with `--plugin-host`.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match af27_vst3::main_with_args(&args) {
        Some(code) => std::process::exit(code),
        None => {
            eprintln!("usage: af27-plughost --plugin-host | --plugin-scan <bundle>");
            std::process::exit(2);
        }
    }
}
