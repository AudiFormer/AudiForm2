// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Flush-to-zero for the threads that run effects.
//!
//! Envelopes, filters and reverb tails decaying into silence end in
//! subnormal numbers, which the processor handles many times slower than
//! normal ones: an effect left running on silence can take two or three
//! times the CPU it takes on sound. Setting the processor to treat
//! subnormals as zero (FTZ and DAZ on x86, FZ on ARM) removes that cost
//! for every effect at once. The difference is below -700 dB.

/// Sets the calling thread to flush subnormal floats to zero. Cheap enough
/// to call at the top of every audio callback (a thread's mode can be
/// changed under it by a host or plug-in).
#[inline]
pub fn flush_to_zero() {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    // SAFETY: reads and writes this thread's SSE control register only;
    // FTZ (bit 15) and DAZ (bit 6) change how subnormals are treated and
    // nothing else. Every x86-64 processor (and x86 with SSE2, which Rust
    // targets by default) has them.
    unsafe {
        let mut csr: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags));
        if csr & 0x8040 != 0x8040 {
            csr |= 0x8040;
            std::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack, preserves_flags));
        }
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: sets the FZ bit (24) of this thread's floating-point control
    // register, which only flushes subnormals to zero.
    unsafe {
        let mut fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack, preserves_flags));
        if fpcr & (1 << 24) == 0 {
            fpcr |= 1 << 24;
            std::arch::asm!("msr fpcr, {}", in(reg) fpcr, options(nomem, nostack, preserves_flags));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn subnormals_become_zero() {
        std::thread::spawn(|| {
            super::flush_to_zero();
            let tiny = std::hint::black_box(f32::MIN_POSITIVE);
            let half = std::hint::black_box(tiny * 0.5);
            assert_eq!(half, 0.0);
        })
        .join()
        .unwrap();
    }
}
