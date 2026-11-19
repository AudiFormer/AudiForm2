// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Spectral editing: change the level of an area of the time x frequency
//! plane, or fill it in from its surroundings.
//!
//! The method is the textbook short-time Fourier transform: Hann windows
//! at 75 % overlap, the same window again for synthesis, and overlap-add
//! divided by the summed squared window. An edit is applied as a
//! *difference*: only the frames the area touches are transformed, the
//! change of each frame's spectrum is turned back into sound, windowed and
//! added to the original. Samples no changed frame reaches are therefore
//! the original samples, bit for bit, and the synthesis window fades the
//! change in and out at the edges of the changed span.

use crate::fft::{fft, hann};

/// Which frames and bins an edit reaches, and how much (0..1).
///
/// Frame `k` (an absolute index on the lattice) is centred on sample
/// `k * hop` and starts at `k * hop - fft / 2`. The grid holds frames
/// `k0 .. k0 + frames`; every other frame has weight 0. Each frame has
/// `fft / 2 + 1` bins, lowest first; bin `b` is `b * rate / fft` Hz.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub fft: usize,
    pub hop: usize,
    pub k0: i64,
    pub frames: usize,
    /// `frames * bins()` weights, frame by frame.
    pub w: Vec<f32>,
}

impl Grid {
    /// An all-zero grid.
    pub fn new(fft: usize, k0: i64, frames: usize) -> Grid {
        let fft = fft.next_power_of_two().max(16);
        Grid {
            fft,
            hop: fft / 4,
            k0,
            frames,
            w: vec![0.0; frames * (fft / 2 + 1)],
        }
    }

    pub fn bins(&self) -> usize {
        self.fft / 2 + 1
    }

    /// The weight of absolute frame `k`, bin `b` (0 outside the grid).
    pub fn at(&self, k: i64, b: usize) -> f32 {
        let i = k - self.k0;
        if i < 0 || i as usize >= self.frames || b >= self.bins() {
            0.0
        } else {
            self.w[i as usize * self.bins() + b]
        }
    }

    /// The first and last frames with any weight.
    pub fn used_frames(&self) -> Option<(i64, i64)> {
        let bins = self.bins();
        let mut lo = None;
        let mut hi = None;
        for f in 0..self.frames {
            if self.w[f * bins..(f + 1) * bins].iter().any(|&x| x > 1e-6) {
                lo.get_or_insert(f);
                hi = Some(f);
            }
        }
        Some((self.k0 + lo? as i64, self.k0 + hi? as i64))
    }
}

/// What an edit does to the area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Multiply the area's magnitudes by this linear gain (0 removes it).
    Gain(f64),
    /// Fill each frequency band of the area from the level just before
    /// and just after it, keeping the original phase.
    Repair,
}

impl Action {
    /// A cut of `db` decibels (positive `db` lowers the level).
    pub fn reduce_db(db: f64) -> Action {
        Action::Gain(10f64.powf(-db / 20.0))
    }
}

/// The squared-window sum the overlap-add divides by, for each sample
/// position within one hop (the lattice is regular, so it repeats).
fn ola_norm(win: &[f64], hop: usize) -> Vec<f64> {
    let n = win.len();
    (0..hop)
        .map(|p| {
            let mut s = 0.0;
            let mut i = p;
            while i < n {
                s += win[i] * win[i];
                i += hop;
            }
            s.max(1e-12)
        })
        .collect()
}

/// Copies frame `k` of `data` (zeros past either end) times the window.
fn load_frame(data: &[f32], k: i64, hop: usize, win: &[f64], re: &mut [f64], im: &mut [f64]) {
    let n = win.len();
    let start = k * hop as i64 - (n / 2) as i64;
    for i in 0..n {
        let idx = start + i as i64;
        let s = if idx >= 0 && (idx as usize) < data.len() {
            data[idx as usize] as f64
        } else {
            0.0
        };
        re[i] = s * win[i];
        im[i] = 0.0;
    }
}

/// The frames whose centre lies inside the signal: `0 ..= last`.
fn last_frame(len: usize, hop: usize) -> i64 {
    (len.saturating_sub(1) / hop) as i64
}

/// Applies `action` to one channel. Returns the first sample of the span
/// that changed and the new samples for it, or None when the grid is
/// empty. Everything outside the returned span is unchanged, and inside
/// it, samples no changed frame reaches are the original samples exactly.
/// `progress` is told the share done, 0..1.
pub fn edit_channel(
    data: &[f32],
    grid: &Grid,
    action: Action,
    progress: &mut dyn FnMut(f32),
) -> Option<(usize, Vec<f32>)> {
    if data.is_empty() {
        return None;
    }
    let n = grid.fft;
    let hop = grid.hop.clamp(1, n);
    let bins = grid.bins();
    let (kmin, kmax) = grid.used_frames()?;
    let win = hann(n);
    let norm = ola_norm(&win, hop);
    let len = data.len();

    // The span the changed frames reach.
    let first = (kmin * hop as i64 - (n / 2) as i64).max(0) as usize;
    let end = ((kmax * hop as i64 + (n / 2) as i64).max(0) as usize).min(len);
    if first >= end {
        return None;
    }
    let mut delta = vec![0.0f64; end - first];

    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];

    // Repair: the level every changed cell should have.
    let targets = match action {
        Action::Repair => Some(repair_targets(data, grid, kmin, kmax, &win, hop)),
        Action::Gain(_) => None,
    };
    let total = (kmax - kmin + 1) as f32;
    let mut dr = vec![0.0; n];
    let mut di = vec![0.0; n];
    for k in kmin..=kmax {
        let row = (k - grid.k0) as usize;
        let ws = &grid.w[row * bins..(row + 1) * bins];
        if ws.iter().all(|&x| x <= 1e-6) {
            continue;
        }
        load_frame(data, k, hop, &win, &mut re, &mut im);
        fft(&mut re, &mut im, false);
        // D = Y - X on the positive bins, mirrored so the result is real.
        dr.iter_mut().for_each(|x| *x = 0.0);
        di.iter_mut().for_each(|x| *x = 0.0);
        let mut any = false;
        for b in 0..bins {
            let w = ws[b].clamp(0.0, 1.0) as f64;
            if w <= 1e-6 {
                continue;
            }
            let (xr, xi) = (re[b], im[b]);
            let (yr, yi) = match action {
                Action::Gain(g) => {
                    let s = 1.0 - w * (1.0 - g);
                    (xr * s, xi * s)
                }
                Action::Repair => {
                    let t = targets.as_ref().unwrap()[(k - kmin) as usize * bins + b] as f64;
                    let mag = xr.hypot(xi);
                    let new = (1.0 - w) * mag + w * t;
                    if mag > 1e-12 {
                        (xr * new / mag, xi * new / mag)
                    } else {
                        (new, 0.0)
                    }
                }
            };
            let (er, ei) = (yr - xr, yi - xi);
            if er == 0.0 && ei == 0.0 {
                continue;
            }
            any = true;
            dr[b] = er;
            di[b] = ei;
            if b != 0 && b != n / 2 {
                dr[n - b] = er;
                di[n - b] = -ei;
            }
        }
        if !any {
            continue;
        }
        fft(&mut dr, &mut di, true);
        let start = k * hop as i64 - (n / 2) as i64;
        for i in 0..n {
            let idx = start + i as i64;
            if idx < first as i64 || idx >= end as i64 {
                continue;
            }
            delta[(idx as usize) - first] += dr[i] * win[i];
        }
        if (k - kmin) % 64 == 0 {
            progress((k - kmin + 1) as f32 / total);
        }
    }
    progress(1.0);
    let out: Vec<f32> = (first..end)
        .map(|i| {
            let d = delta[i - first];
            if d == 0.0 {
                data[i]
            } else {
                (data[i] as f64 + d / norm[i % hop]) as f32
            }
        })
        .collect();
    Some((first, out))
}

/// For Repair: the magnitude each cell of frames `kmin..=kmax` should
/// take, interpolated per bin between the level just before and just
/// after each run of selected frames. Reference frames are far enough
/// out that their windows do not reach the area.
fn repair_targets(
    data: &[f32],
    grid: &Grid,
    kmin: i64,
    kmax: i64,
    win: &[f64],
    hop: usize,
) -> Vec<f32> {
    let n = win.len();
    let bins = grid.bins();
    // Frames between the run and its reference whose windows would
    // still overlap the run's centres.
    let off = (n / (2 * hop)) as i64 + 1;
    let m = off + 2;
    let (lo, hi) = (kmin - m, kmax + m);
    let last = last_frame(data.len(), hop);
    let mut mags = vec![0.0f32; (hi - lo + 1) as usize * bins];
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for k in lo..=hi {
        if k < 0 || k > last {
            continue;
        }
        load_frame(data, k, hop, win, &mut re, &mut im);
        fft(&mut re, &mut im, false);
        let row = (k - lo) as usize * bins;
        for b in 0..bins {
            mags[row + b] = re[b].hypot(im[b]) as f32;
        }
    }
    let mag = |k: i64, b: usize| mags[(k - lo) as usize * bins + b];
    // The mean level of reference frames `ks` that exist.
    let level = |ks: [i64; 2], b: usize| -> Option<f32> {
        let got: Vec<f32> = ks
            .iter()
            .filter(|&&k| k >= lo && k <= hi && k >= 0 && k <= last)
            .map(|&k| mag(k, b))
            .collect();
        (!got.is_empty()).then(|| got.iter().sum::<f32>() / got.len() as f32)
    };
    let mut out = vec![0.0f32; (kmax - kmin + 1) as usize * bins];
    for b in 0..bins {
        let mut k = kmin;
        while k <= kmax {
            if grid.at(k, b) <= 1e-6 {
                k += 1;
                continue;
            }
            let r0 = k;
            while k <= kmax && grid.at(k, b) > 1e-6 {
                k += 1;
            }
            let r1 = k - 1;
            let before = level([r0 - off - 1, r0 - off], b);
            let after = level([r1 + off, r1 + off + 1], b);
            let (a, z) = match (before, after) {
                (Some(a), Some(z)) => (a, z),
                (Some(a), None) => (a, a),
                (None, Some(z)) => (z, z),
                (None, None) => (0.0, 0.0),
            };
            let span = (r1 - r0 + 2) as f32;
            for j in r0..=r1 {
                let f = (j - r0 + 1) as f32 / span;
                out[(j - kmin) as usize * bins + b] = a + (z - a) * f;
            }
        }
    }
    out
}

/// A plain STFT of `data` on the same lattice (frames `0..=last`), each
/// frame's `fft / 2 + 1` complex bins. For analysis and tests.
pub fn stft(data: &[f32], n: usize, hop: usize) -> Vec<Vec<(f64, f64)>> {
    let win = hann(n);
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    // Enough frames that every sample is covered by a full set.
    let last = last_frame(data.len(), hop) + (n / hop) as i64;
    (-((n / hop) as i64)..=last)
        .map(|k| {
            load_frame(data, k, hop, &win, &mut re, &mut im);
            fft(&mut re, &mut im, false);
            (0..=n / 2).map(|b| (re[b], im[b])).collect()
        })
        .collect()
}

/// The inverse of [`stft`]: windowed overlap-add, divided by the summed
/// squared window, `len` samples long.
pub fn istft(frames: &[Vec<(f64, f64)>], n: usize, hop: usize, len: usize) -> Vec<f32> {
    let win = hann(n);
    let norm = ola_norm(&win, hop);
    let mut out = vec![0.0f64; len];
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    let k_first = -((n / hop) as i64);
    for (i, fr) in frames.iter().enumerate() {
        for b in 0..=n / 2 {
            re[b] = fr[b].0;
            im[b] = fr[b].1;
            if b != 0 && b != n / 2 {
                re[n - b] = fr[b].0;
                im[n - b] = -fr[b].1;
            }
        }
        fft(&mut re, &mut im, true);
        let k = k_first + i as i64;
        let start = k * hop as i64 - (n / 2) as i64;
        for j in 0..n {
            let idx = start + j as i64;
            if idx >= 0 && (idx as usize) < len {
                out[idx as usize] += re[j] * win[j];
            }
        }
    }
    out.iter()
        .enumerate()
        .map(|(i, &s)| (s / norm[i % hop]) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const RATE: f64 = 48_000.0;

    fn tone(len: usize, hz: f64, amp: f64) -> Vec<f32> {
        (0..len)
            .map(|i| (amp * (2.0 * PI * hz * i as f64 / RATE).sin()) as f32)
            .collect()
    }

    /// The level of `hz` in `x[a..b]`, as an amplitude.
    fn level(x: &[f32], a: usize, b: usize, hz: f64) -> f64 {
        let (mut s, mut c) = (0.0, 0.0);
        for (i, &v) in x[a..b].iter().enumerate() {
            let ph = 2.0 * PI * hz * (a + i) as f64 / RATE;
            s += v as f64 * ph.sin();
            c += v as f64 * ph.cos();
        }
        2.0 * s.hypot(c) / (b - a) as f64
    }

    fn db(x: f64) -> f64 {
        20.0 * x.max(1e-12).log10()
    }

    /// A grid of ones over frames whose centres fall in `t0..t1` seconds
    /// and bins in `f0..f1` Hz.
    fn box_grid(n: usize, t0: f64, t1: f64, f0: f64, f1: f64) -> Grid {
        let hop = n / 4;
        let k0 = (t0 * RATE / hop as f64).ceil() as i64;
        let k1 = (t1 * RATE / hop as f64).floor() as i64;
        let mut g = Grid::new(n, k0, (k1 - k0 + 1) as usize);
        let bins = g.bins();
        for f in 0..g.frames {
            for b in 0..bins {
                let hz = b as f64 * RATE / n as f64;
                if hz >= f0 && hz <= f1 {
                    g.w[f * bins + b] = 1.0;
                }
            }
        }
        g
    }

    #[test]
    fn stft_round_trip_is_identity() {
        let x: Vec<f32> = (0..20_000)
            .map(|i| ((i as f64 * 0.37).sin() * 0.5 + (i as f64 * 0.011).cos() * 0.3) as f32)
            .collect();
        for n in [512, 2048] {
            let fr = stft(&x, n, n / 4);
            let y = istft(&fr, n, n / 4, x.len());
            let err = x
                .iter()
                .zip(&y)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(err < 1e-5, "n {n}: {err}");
        }
    }

    #[test]
    fn all_ones_mask_at_unity_gain_changes_nothing() {
        let x = tone(30_000, 997.0, 0.5);
        let mut g = Grid::new(2048, 0, 30_000 / 512 + 1);
        g.w.iter_mut().for_each(|w| *w = 1.0);
        let (first, out) = edit_channel(&x, &g, Action::Gain(1.0), &mut |_| {}).unwrap();
        for (i, &v) in out.iter().enumerate() {
            assert!((v - x[first + i]).abs() < 1e-5);
        }
    }

    #[test]
    fn reduce_12_db_inside_the_box_only() {
        let len = 96_000;
        let inside = tone(len, 1000.0, 0.4);
        let outside = tone(len, 6000.0, 0.4);
        let x: Vec<f32> = inside.iter().zip(&outside).map(|(a, b)| a + b).collect();
        let g = box_grid(2048, 0.5, 1.5, 600.0, 1600.0);
        let (first, seg) = edit_channel(&x, &g, Action::reduce_db(12.0), &mut |_| {}).unwrap();
        let mut y = x.clone();
        y[first..first + seg.len()].copy_from_slice(&seg);
        // Middle of the box.
        let (a, b) = (60_000, 66_000);
        let drop_in = db(level(&y, a, b, 1000.0)) - db(level(&x, a, b, 1000.0));
        assert!(
            (drop_in + 12.0).abs() < 0.5,
            "in-box tone fell {drop_in} dB"
        );
        let drop_out = db(level(&y, a, b, 6000.0)) - db(level(&x, a, b, 6000.0));
        assert!(drop_out.abs() < 0.05, "out-of-box tone moved {drop_out} dB");
        // Before and after the changed span: exactly the original.
        assert!(first > 0 && first + seg.len() < len);
        assert_eq!(&y[..first], &x[..first]);
        assert_eq!(&y[first + seg.len()..], &x[first + seg.len()..]);
        // Well before the box in time, also the in-box tone is untouched.
        let pre = db(level(&y, 2000, 8000, 1000.0)) - db(level(&x, 2000, 8000, 1000.0));
        assert!(pre.abs() < 1e-3);
    }

    #[test]
    fn remove_kills_the_tone_in_the_box() {
        let x = tone(96_000, 1500.0, 0.5);
        let g = box_grid(2048, 0.4, 1.6, 1000.0, 2000.0);
        let (first, seg) = edit_channel(&x, &g, Action::Gain(0.0), &mut |_| {}).unwrap();
        let mut y = x.clone();
        y[first..first + seg.len()].copy_from_slice(&seg);
        let (a, b) = (40_000, 56_000);
        let after = db(level(&y, a, b, 1500.0));
        assert!(after < -60.0, "tone left at {after} dB");
        let rms =
            (y[a..b].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / (b - a) as f64).sqrt();
        assert!(db(rms) < -60.0, "residue {} dB", db(rms));
    }

    #[test]
    fn repair_fills_a_click_from_the_tone_around_it() {
        let len = 96_000;
        let mut x = tone(len, 1000.0, 0.3);
        let click = 48_000;
        for i in 0..8 {
            x[click + i] += if i % 2 == 0 { 0.9 } else { -0.9 };
        }
        // The click's time and a frame either side, all frequencies.
        let n = 2048;
        let hop = n / 4;
        let k = (click / hop) as i64;
        let mut g = Grid::new(n, k - 2, 5);
        g.w.iter_mut().for_each(|w| *w = 1.0);
        let (first, seg) = edit_channel(&x, &g, Action::Repair, &mut |_| {}).unwrap();
        let mut y = x.clone();
        y[first..first + seg.len()].copy_from_slice(&seg);
        let clean = tone(len, 1000.0, 0.3);
        // Around the click the signal is near the clean tone again.
        let (a, b) = (click - 1000, click + 1000);
        let err = y[a..b]
            .iter()
            .zip(&clean[a..b])
            .map(|(p, q)| ((p - q) as f64).abs())
            .fold(0.0, f64::max);
        assert!(err < 0.08, "biggest error near the click {err}");
        let lvl = db(level(&y, a, b, 1000.0)) - db(0.3);
        assert!(lvl.abs() < 1.0, "tone level after repair {lvl} dB");
        let peak = y[a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.36, "click still peaks at {peak}");
    }

    #[test]
    fn empty_grid_changes_nothing() {
        let x = tone(10_000, 440.0, 0.5);
        let g = Grid::new(1024, 0, 10);
        assert!(edit_channel(&x, &g, Action::Gain(0.0), &mut |_| {}).is_none());
    }
}
