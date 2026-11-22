// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Quantize: pulls MIDI notes toward a grid.
//!
//! The grid counts from the session start in quarter notes, so notes in
//! different clips line up the same way. Strength says how far a note
//! moves toward its grid line (100% lands on it, 50% halfway). Swing
//! delays every second grid line: 0% is straight, 100% puts the off-beat
//! a third of a step late, the triplet (2:1) feel.

use crate::midi::MidiNote;

/// What happens to a note's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ends {
    /// The length stays; the end moves with the start.
    #[default]
    Keep,
    /// The end is pulled to its own grid line.
    End,
    /// The length is pulled to a whole number of grid steps.
    Length,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantize {
    /// Grid step in quarter notes (0.25 = sixteenths).
    pub grid: f64,
    /// 0..1.
    pub strength: f64,
    /// 0..1.
    pub swing: f64,
    /// Move starts (off: only ends or lengths change).
    pub starts: bool,
    pub ends: Ends,
}

impl Default for Quantize {
    fn default() -> Self {
        Quantize {
            grid: 0.25,
            strength: 1.0,
            swing: 0.0,
            starts: true,
            ends: Ends::Keep,
        }
    }
}

/// Grid line `k` (from the session start) with swing.
pub fn grid_line(k: i64, grid: f64, swing: f64) -> f64 {
    let base = k as f64 * grid;
    if k.rem_euclid(2) == 1 {
        base + swing.clamp(0.0, 1.0) * grid / 3.0
    } else {
        base
    }
}

/// The grid line index nearest `x`.
pub fn nearest_line(x: f64, grid: f64, swing: f64) -> i64 {
    let k0 = (x / grid).floor() as i64;
    (k0 - 1..=k0 + 2)
        .min_by(|&a, &b| {
            (grid_line(a, grid, swing) - x)
                .abs()
                .total_cmp(&(grid_line(b, grid, swing) - x).abs())
        })
        .unwrap_or(k0)
}

/// The grid line nearest `x`.
pub fn snap(x: f64, grid: f64, swing: f64) -> f64 {
    grid_line(nearest_line(x, grid, swing), grid, swing)
}

impl Quantize {
    /// Quantizes one note of a clip starting at `clip_start` (quarter
    /// notes from the session start).
    pub fn note(&self, n: &mut MidiNote, clip_start: f64) {
        let g = self.grid;
        if g <= 0.0 {
            return;
        }
        let s = self.strength.clamp(0.0, 1.0);
        let start = clip_start + n.start;
        let end = start + n.length;
        let new_start = if self.starts {
            start + (snap(start, g, self.swing) - start) * s
        } else {
            start
        };
        let length = match self.ends {
            Ends::Keep => n.length,
            Ends::End => {
                let ks = nearest_line(new_start, g, self.swing);
                let mut ke = nearest_line(end, g, self.swing);
                if ke <= ks {
                    ke = ks + 1;
                }
                let target = grid_line(ke, g, self.swing);
                let new_end = end + (target - end) * s;
                new_end - new_start
            }
            Ends::Length => {
                let steps = (n.length / g).round().max(1.0);
                n.length + (steps * g - n.length) * s
            }
        };
        n.start = (new_start - clip_start).max(0.0);
        n.length = length.max(1.0 / 960.0);
    }

    /// Quantizes every note (or those `pick` says yes to).
    pub fn notes(&self, notes: &mut [MidiNote], clip_start: f64, pick: impl Fn(usize) -> bool) {
        for (i, n) in notes.iter_mut().enumerate() {
            if pick(i) {
                self.note(n, clip_start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: f64, length: f64) -> MidiNote {
        MidiNote {
            pitch: 60,
            start,
            length,
            velocity: 100,
            channel: 0,
            pan: 0.0,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn starts_land_on_the_grid() {
        let q = Quantize::default();
        let mut a = n(0.30, 0.5);
        q.note(&mut a, 0.0);
        assert!(close(a.start, 0.25) && close(a.length, 0.5));
        let mut b = n(0.40, 0.5);
        q.note(&mut b, 0.0);
        assert!(close(b.start, 0.5));
        // Grid counts from the session start, not the clip.
        let mut c = n(0.3, 0.5);
        q.note(&mut c, 1.1);
        assert!(close(c.start + 1.1, 1.5));
        // Never before the clip's start.
        let mut d = n(0.0, 0.5);
        q.note(&mut d, 1.1);
        assert!(close(d.start, 0.0));
    }

    #[test]
    fn strength_moves_part_way() {
        let q = Quantize {
            strength: 0.5,
            ..Quantize::default()
        };
        let mut a = n(0.35, 0.5);
        q.note(&mut a, 0.0);
        assert!(close(a.start, 0.30));
        let off = Quantize {
            strength: 0.0,
            ..Quantize::default()
        };
        let mut b = n(0.35, 0.5);
        off.note(&mut b, 0.0);
        assert!(close(b.start, 0.35));
    }

    #[test]
    fn ends_and_lengths() {
        let q = Quantize {
            ends: Ends::End,
            ..Quantize::default()
        };
        let mut a = n(0.30, 0.40);
        q.note(&mut a, 0.0);
        // Start 0.25, end 0.70 -> 0.75.
        assert!(close(a.start, 0.25) && close(a.length, 0.5));
        // An end that would land on the start gets one step.
        let mut b = n(0.0, 0.05);
        q.note(&mut b, 0.0);
        assert!(close(b.length, 0.25));
        let q = Quantize {
            ends: Ends::Length,
            starts: false,
            ..Quantize::default()
        };
        let mut c = n(0.33, 0.6);
        q.note(&mut c, 0.0);
        assert!(close(c.start, 0.33) && close(c.length, 0.5));
    }

    #[test]
    fn swing_delays_off_beats() {
        // Eighths at full swing: the off-beat sits at 2/3 of the beat.
        let g = 0.5;
        assert!(close(grid_line(0, g, 1.0), 0.0));
        assert!(close(grid_line(1, g, 1.0), 0.5 + 0.5 / 3.0));
        assert!(close(grid_line(2, g, 1.0), 1.0));
        assert!(close(grid_line(3, g, 0.5), 1.5 + 0.25 / 3.0));
        let q = Quantize {
            grid: g,
            swing: 1.0,
            ..Quantize::default()
        };
        let mut a = n(0.52, 0.25);
        q.note(&mut a, 0.0);
        assert!(close(a.start, 2.0 / 3.0));
        let mut b = n(0.95, 0.25);
        q.note(&mut b, 0.0);
        assert!(close(b.start, 1.0));
        // Swing parity counts from the session start.
        let mut c = n(0.0, 0.25);
        q.note(&mut c, 0.5);
        assert!(close(c.start + 0.5, 2.0 / 3.0));
    }
}
