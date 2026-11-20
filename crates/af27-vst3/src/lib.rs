// SPDX-License-Identifier: GPL-3.0-or-later
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
pub mod presets;
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

/// Ties a host or scan process to the app: they share one Windows job set
/// to kill everything in it when its last handle closes, which is when the
/// app exits, however it exits. Copy protection in some plug-ins can keep a
/// host from ever ending on its own; this ends it anyway.
pub fn bind_to_app(child: &std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::sync::OnceLock;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        static JOB: OnceLock<usize> = OnceLock::new();
        let job = *JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            // The handle stays open for the app's life on purpose.
            h as usize
        });
        if job != 0 {
            unsafe {
                AssignProcessToJobObject(
                    job as windows_sys::Win32::Foundation::HANDLE,
                    child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
                );
            }
        }
    }
    #[cfg(not(windows))]
    let _ = child;
}

/// Puts the calling thread at audio priority (the Windows multimedia
/// scheduler's "Pro Audio" class, and the highest thread priority), so a
/// busy desktop does not make a plug-in late: a late plug-in plays dry for
/// that moment, which is heard as a stutter.
pub fn audio_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority,
            THREAD_PRIORITY_HIGHEST,
        };
        let name: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
        let mut task = 0u32;
        unsafe {
            AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut task);
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
    }
}

/// Raises the whole plug-in host process above normal, so its message
/// loop and plug-in threads keep up while the app draws.
pub fn host_process_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS,
        };
        unsafe {
            SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);
        }
    }
}

/// Ends a host or scan process now with `code`, without running loaded
/// plug-in DLLs' shutdown code (a stuck plug-in can hang a normal exit
/// forever, leaving the process behind after the app has gone).
pub fn exit_now(code: i32) -> ! {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    std::process::exit(code)
}

/// Exits this process as soon as process `pid` is gone.
fn watch_parent(pid: u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, OpenProcess, TerminateProcess, WaitForSingleObject, INFINITE,
            PROCESS_SYNCHRONIZE,
        };
        // SAFETY: plain Win32 calls on a handle this thread owns.
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if h.is_null() {
            // Already gone.
            unsafe { TerminateProcess(GetCurrentProcess(), 0) };
        }
        let h = h as usize;
        let _ = std::thread::Builder::new()
            .name("parent-watch".into())
            .spawn(move || {
                let h = h as windows_sys::Win32::Foundation::HANDLE;
                unsafe {
                    WaitForSingleObject(h, INFINITE);
                    CloseHandle(h);
                    // Ends at once: a normal exit runs every plug-in DLL's
                    // shutdown code, which a stuck plug-in can hang.
                    TerminateProcess(GetCurrentProcess(), 0);
                }
            });
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// Runs a host or scan process when the command line asks for one, and
/// returns its exit code; None for a normal start of the app.
pub fn main_with_args(args: &[String]) -> Option<i32> {
    // A host or scan process never outlives the app that started it, even
    // when a plug-in has it stuck and it never reads its pipe again.
    if let Some(pid) = args
        .iter()
        .position(|a| a == "--parent")
        .and_then(|k| args.get(k + 1))
        .and_then(|p| p.parse::<u32>().ok())
    {
        watch_parent(pid);
    }
    if matches!(
        args.get(1).map(String::as_str),
        Some("--plugin-host") | Some("--plugin-scan")
    ) {
        // Any panic ends a host at once too: unwinding out of main runs a
        // normal exit, and copy-protection code in some plug-ins (seen with
        // CodeMeter) sleeps forever in its shutdown, leaving the process
        // behind.
        std::panic::set_hook(Box::new(|_| exit_now(101)));
    }
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
