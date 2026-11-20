// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! A planar, in-memory audio buffer: one `Vec<f32>` per channel.

/// A half-open frame range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl Range {
    pub fn new(a: usize, b: usize) -> Self {
        Range {
            start: a.min(b),
            end: a.max(b),
        }
    }
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
    pub fn clamp(self, frames: usize) -> Self {
        Range::new(self.start.min(frames), self.end.min(frames))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    pub rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    pub fn new(rate: u32, channels: usize, frames: usize) -> Self {
        AudioBuffer {
            rate,
            channels: vec![vec![0.0; frames]; channels.max(1)],
        }
    }

    /// Builds a buffer from interleaved samples.
    pub fn from_interleaved(rate: u32, channels: usize, data: &[f32]) -> Self {
        let ch = channels.max(1);
        let frames = data.len() / ch;
        let mut out = vec![Vec::with_capacity(frames); ch];
        for frame in data.chunks_exact(ch) {
            for (c, s) in frame.iter().enumerate() {
                out[c].push(*s);
            }
        }
        AudioBuffer {
            rate,
            channels: out,
        }
    }

    pub fn to_interleaved(&self) -> Vec<f32> {
        let ch = self.num_channels();
        let mut out = Vec::with_capacity(self.frames() * ch);
        for i in 0..self.frames() {
            for c in &self.channels {
                out.push(c[i]);
            }
        }
        out
    }

    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }

    pub fn is_empty(&self) -> bool {
        self.frames() == 0
    }

    /// A copy of the frames in `r`.
    pub fn slice(&self, r: Range) -> AudioBuffer {
        let r = r.clamp(self.frames());
        AudioBuffer {
            rate: self.rate,
            channels: self
                .channels
                .iter()
                .map(|c| c[r.start..r.end].to_vec())
                .collect(),
        }
    }

    /// Removes the frames in `r` and returns them.
    pub fn cut(&mut self, r: Range) -> AudioBuffer {
        let r = r.clamp(self.frames());
        let rate = self.rate;
        let removed = self
            .channels
            .iter_mut()
            .map(|c| c.drain(r.start..r.end).collect())
            .collect();
        AudioBuffer {
            rate,
            channels: removed,
        }
    }

    /// Inserts `other` at frame `at`. Channel counts are matched by
    /// repeating the last channel of `other` or dropping extra ones.
    pub fn insert(&mut self, at: usize, other: &AudioBuffer) {
        let at = at.min(self.frames());
        if other.channels.is_empty() {
            return;
        }
        for (c, dst) in self.channels.iter_mut().enumerate() {
            let src = &other.channels[c.min(other.channels.len() - 1)];
            dst.splice(at..at, src.iter().copied());
        }
    }

    /// Replaces the frames in `r` with `other` (lengths may differ).
    pub fn replace(&mut self, r: Range, other: &AudioBuffer) {
        let r = r.clamp(self.frames());
        self.cut(r);
        self.insert(r.start, other);
    }

    /// Mono or stereo view of one frame, for metering and playback.
    #[inline]
    pub fn stereo_at(&self, i: usize) -> (f32, f32) {
        match self.channels.len() {
            0 => (0.0, 0.0),
            1 => {
                let s = self.channels[0].get(i).copied().unwrap_or(0.0);
                (s, s)
            }
            _ => (
                self.channels[0].get(i).copied().unwrap_or(0.0),
                self.channels[1].get(i).copied().unwrap_or(0.0),
            ),
        }
    }

    /// Peak absolute sample value over all channels.
    pub fn peak(&self) -> f32 {
        self.channels
            .iter()
            .flat_map(|c| c.iter())
            .fold(0.0f32, |m, s| m.max(s.abs()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_round_trip() {
        let data = [0.1, -0.1, 0.2, -0.2, 0.3, -0.3];
        let b = AudioBuffer::from_interleaved(48000, 2, &data);
        assert_eq!(b.frames(), 3);
        assert_eq!(b.channels[1], vec![-0.1, -0.2, -0.3]);
        assert_eq!(b.to_interleaved(), data.to_vec());
    }

    #[test]
    fn cut_insert_replace() {
        let mut b = AudioBuffer::from_interleaved(8, 1, &[0., 1., 2., 3., 4., 5.]);
        let removed = b.cut(Range::new(1, 3));
        assert_eq!(removed.channels[0], vec![1., 2.]);
        assert_eq!(b.channels[0], vec![0., 3., 4., 5.]);
        b.insert(1, &removed);
        assert_eq!(b.channels[0], vec![0., 1., 2., 3., 4., 5.]);
        let silence = AudioBuffer::new(8, 1, 1);
        b.replace(Range::new(2, 5), &silence);
        assert_eq!(b.channels[0], vec![0., 1., 0., 5.]);
    }

    #[test]
    fn range_orders_and_clamps() {
        let r = Range::new(10, 2).clamp(5);
        assert_eq!((r.start, r.end), (2, 5));
    }
}
