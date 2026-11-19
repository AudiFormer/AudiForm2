// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! A min/max pyramid so waveforms of long files draw quickly at any zoom.

use af27_core::AudioBuffer;

const BLOCK: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct Peaks {
    /// Per channel, per level: (min, max) per block. Level 0 covers
    /// `BLOCK` samples per entry; each next level covers twice as many.
    levels: Vec<Vec<Vec<(f32, f32)>>>,
}

impl Peaks {
    pub fn build(buf: &AudioBuffer) -> Self {
        let levels = buf
            .channels
            .iter()
            .map(|ch| {
                let mut lv: Vec<Vec<(f32, f32)>> = vec![ch
                    .chunks(BLOCK)
                    .map(|c| {
                        c.iter()
                            .fold((f32::MAX, f32::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)))
                    })
                    .collect()];
                while lv.last().unwrap().len() > 1 {
                    let prev = lv.last().unwrap();
                    let next = prev
                        .chunks(2)
                        .map(|p| {
                            p.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(a, b)| {
                                (lo.min(a), hi.max(b))
                            })
                        })
                        .collect();
                    lv.push(next);
                }
                lv
            })
            .collect();
        Peaks { levels }
    }

    /// (min, max) of `data[start..end]` for channel `ch`, using the raw
    /// samples for short spans and the pyramid for long ones.
    pub fn range(&self, data: &[f32], ch: usize, start: usize, end: usize) -> (f32, f32) {
        let end = end.min(data.len());
        if start >= end {
            return (0.0, 0.0);
        }
        let span = end - start;
        if span <= BLOCK * 2 || ch >= self.levels.len() {
            return data[start..end]
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        }
        let mut level = 0;
        let mut size = BLOCK;
        while size * 4 <= span && level + 1 < self.levels[ch].len() {
            level += 1;
            size *= 2;
        }
        let lv = &self.levels[ch][level];
        let a = start / size;
        let b = end.div_ceil(size).min(lv.len());
        lv[a..b.max(a + 1).min(lv.len())]
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &(x, y)| {
                (lo.min(x), hi.max(y))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyramid_matches_raw_scan() {
        let mut b = AudioBuffer::new(48_000, 1, 100_000);
        for (i, s) in b.channels[0].iter_mut().enumerate() {
            *s = ((i * 7919) % 1000) as f32 / 1000.0 - 0.5;
        }
        b.channels[0][54_321] = 0.99;
        let p = Peaks::build(&b);
        let (_, hi) = p.range(&b.channels[0], 0, 0, 100_000);
        assert_eq!(hi, 0.99);
        let (lo, hi) = p.range(&b.channels[0], 0, 10, 50);
        let raw = b.channels[0][10..50]
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), &s| (l.min(s), h.max(s)));
        assert_eq!((lo, hi), raw);
    }
}
