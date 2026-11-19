// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! An iterative radix-2 complex FFT and a spectrogram helper.

use std::f64::consts::PI;

/// In-place FFT of `re` + i`im` (length a power of two).
/// `inverse` runs the inverse transform, scaled by 1/n.
pub fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * 2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
    if inverse {
        let s = 1.0 / n as f64;
        re.iter_mut().for_each(|x| *x *= s);
        im.iter_mut().for_each(|x| *x *= s);
    }
}

pub fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos())
        .collect()
}

/// Magnitudes in dBFS for `columns` evenly spaced frames between
/// `start` and `end` of `data`. Each column has `size / 2` bins, low first.
pub fn spectrogram(
    data: &[f32],
    start: usize,
    end: usize,
    columns: usize,
    size: usize,
) -> Vec<Vec<f32>> {
    let size = size.next_power_of_two().max(64);
    let win = hann(size);
    let norm = 2.0 / win.iter().sum::<f64>();
    let span = end.saturating_sub(start).max(1) as f64;
    let mut re = vec![0.0; size];
    let mut im = vec![0.0; size];
    (0..columns.max(1))
        .map(|c| {
            let centre = start as f64 + span * (c as f64 + 0.5) / columns.max(1) as f64;
            let first = centre as i64 - size as i64 / 2;
            for k in 0..size {
                let idx = first + k as i64;
                let s = if idx >= 0 && (idx as usize) < data.len() {
                    data[idx as usize] as f64
                } else {
                    0.0
                };
                re[k] = s * win[k];
                im[k] = 0.0;
            }
            fft(&mut re, &mut im, false);
            (0..size / 2)
                .map(|b| {
                    let m = (re[b] * re[b] + im[b] * im[b]).sqrt() * norm;
                    (20.0 * m.max(1e-10).log10()) as f32
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_peak_bin() {
        let n = 256;
        let mut re: Vec<f64> = (0..n)
            .map(|i| (2.0 * PI * 8.0 * i as f64 / n as f64).sin())
            .collect();
        let orig = re.clone();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im, false);
        let mags: Vec<f64> = (0..n / 2).map(|b| re[b].hypot(im[b])).collect();
        let peak = mags
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 8);
        fft(&mut re, &mut im, true);
        for (a, b) in re.iter().zip(&orig) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn spectrogram_finds_a_tone() {
        let rate = 48_000.0;
        let data: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 3000.0 * i as f32 / rate).sin())
            .collect();
        let cols = spectrogram(&data, 0, data.len(), 4, 1024);
        let col = &cols[2];
        let peak = col
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        let freq = peak.0 as f64 * rate as f64 / 1024.0;
        assert!((freq - 3000.0).abs() < 50.0);
        assert!(
            peak.1.abs() < 1.5,
            "full-scale sine reads about 0 dBFS, got {}",
            peak.1
        );
    }
}
