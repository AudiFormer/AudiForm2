// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The process's memory, for benchmarks and `AF2_MEMLOG=1` (prints it
//! every two seconds with what is loaded).

/// This process's private memory in MB (Windows; 0 elsewhere).
pub fn private_mb() -> f64 {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            cb: u32,
            faults: u32,
            peak_ws: usize,
            ws: usize,
            a: usize,
            b: usize,
            c: usize,
            d: usize,
            pagefile: usize,
            peak_pagefile: usize,
            private: usize,
        }
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(p: isize, c: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters {
            cb: std::mem::size_of::<Counters>() as u32,
            ..Default::default()
        };
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        return c.private as f64 / 1048576.0;
    }
    #[allow(unreachable_code)]
    0.0
}


