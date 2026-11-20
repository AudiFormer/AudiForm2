// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Edits on a clip's notes, for the piano roll. Notes are picked by index;
//! nothing here reorders the list, so picked indexes stay valid (only
//! `remove` and `paste` change it, and say how).
//!
//! Gesture edits (`move_from`, `resize_*_from`) work from the notes as
//! they were when the gesture began, so limits never pile up while
//! dragging.

use crate::midi::MidiNote;

/// The shortest a note can be, in quarter notes.
pub const MIN_NOTE: f64 = 1.0 / 64.0;

/// The notes an edit acts on: the picked ones, or all when none are.
pub fn targets(len: usize, picked: &[usize]) -> Vec<usize> {
    if picked.is_empty() {
        (0..len).collect()
    } else {
        picked.iter().copied().filter(|&i| i < len).collect()
    }
}

/// Moves the picked notes by `beats` and `semitones` from `orig`, keeping
/// them all at or after 0 and within 0..127 together.
pub fn move_from(
    notes: &mut [MidiNote],
    orig: &[MidiNote],
    idx: &[usize],
    beats: f64,
    semitones: i32,
) {
    let picked = || idx.iter().filter_map(|&i| orig.get(i));
    let Some(earliest) = picked().map(|n| n.start).min_by(f64::total_cmp) else {
        return;
    };
    let low = picked().map(|n| n.pitch as i32).min().unwrap_or(0);
    let high = picked().map(|n| n.pitch as i32).max().unwrap_or(127);
    let db = beats.max(-earliest);
    let ds = semitones.clamp(-low, 127 - high);
    for &i in idx {
        if let (Some(n), Some(o)) = (notes.get_mut(i), orig.get(i)) {
            n.start = o.start + db;
            n.pitch = (o.pitch as i32 + ds) as u8;
        }
    }
}

/// Stretches the picked notes' ends by `beats` from `orig`.
pub fn resize_end_from(notes: &mut [MidiNote], orig: &[MidiNote], idx: &[usize], beats: f64) {
    for &i in idx {
        if let (Some(n), Some(o)) = (notes.get_mut(i), orig.get(i)) {
            n.length = (o.length + beats).max(MIN_NOTE);
        }
    }
}

/// Moves the picked notes' starts by `beats` from `orig`, ends held.
pub fn resize_start_from(notes: &mut [MidiNote], orig: &[MidiNote], idx: &[usize], beats: f64) {
    for &i in idx {
        if let (Some(n), Some(o)) = (notes.get_mut(i), orig.get(i)) {
            let end = o.start + o.length;
            let start = (o.start + beats).clamp(0.0, end - MIN_NOTE);
            n.start = start;
            n.length = end - start;
        }
    }
}

/// Removes the picked notes.
pub fn remove(notes: &mut Vec<MidiNote>, idx: &[usize]) {
    let mut i = 0;
    notes.retain(|_| {
        let keep = !idx.contains(&i);
        i += 1;
        keep
    });
}

/// The picked notes, moved so the earliest starts at 0 (what Copy keeps).
pub fn copy(notes: &[MidiNote], idx: &[usize]) -> Vec<MidiNote> {
    let picked: Vec<MidiNote> = idx.iter().filter_map(|&i| notes.get(i).copied()).collect();
    let first = picked.iter().map(|n| n.start).fold(f64::MAX, f64::min);
    picked
        .into_iter()
        .map(|n| MidiNote {
            start: n.start - first,
            ..n
        })
        .collect()
}

/// Adds copied notes at `at`; returns the indexes of the new notes.
pub fn paste(notes: &mut Vec<MidiNote>, copied: &[MidiNote], at: f64) -> Vec<usize> {
    let from = notes.len();
    notes.extend(copied.iter().map(|n| MidiNote {
        start: n.start + at.max(0.0),
        ..*n
    }));
    (from..notes.len()).collect()
}

fn clamp_vel(v: f64) -> u8 {
    v.round().clamp(1.0, 127.0) as u8
}

pub fn set_velocity(notes: &mut [MidiNote], idx: &[usize], v: u8) {
    for &i in idx {
        if let Some(n) = notes.get_mut(i) {
            n.velocity = v.clamp(1, 127);
        }
    }
}

/// Scales velocity by `percent` (100 leaves it).
pub fn scale_velocity(notes: &mut [MidiNote], idx: &[usize], percent: f64) {
    for &i in idx {
        if let Some(n) = notes.get_mut(i) {
            n.velocity = clamp_vel(n.velocity as f64 * percent / 100.0);
        }
    }
}

/// Moves each velocity by a random amount up to `range` either way.
pub fn randomize_velocity(notes: &mut [MidiNote], idx: &[usize], range: f64, rng: &mut Rng) {
    for &i in idx {
        if let Some(n) = notes.get_mut(i) {
            n.velocity = clamp_vel(n.velocity as f64 + rng.signed() * range);
        }
    }
}

pub fn set_pan(notes: &mut [MidiNote], idx: &[usize], pan: f32) {
    let p = pan.clamp(-1.0, 1.0);
    for &i in idx {
        if let Some(n) = notes.get_mut(i) {
            n.pan = if p.abs() < 0.005 { 0.0 } else { p };
        }
    }
}

/// Moves each pan by a random amount up to `range` (0..1) either way.
pub fn randomize_pan(notes: &mut [MidiNote], idx: &[usize], range: f32, rng: &mut Rng) {
    for &i in idx {
        if let Some(n) = notes.get_mut(i) {
            n.pan = (n.pan + rng.signed() as f32 * range).clamp(-1.0, 1.0);
        }
    }
}

pub fn set_channel(notes: &mut [MidiNote], idx: &[usize], ch: u8) {
    for &i in idx {
        if let Some(n) = notes.get_mut(i) {
            n.channel = ch.min(15);
        }
    }
}

/// Moves each start up to `beats` earlier or later at random (never
/// before 0); lengths stay.
pub fn humanize(notes: &mut [MidiNote], idx: &[usize], beats: f64, rng: &mut Rng) {
    for &i in idx {
        if let Some(n) = notes.get_mut(i) {
            n.start = (n.start + rng.signed() * beats).max(0.0);
        }
    }
}

/// The notes whose rectangles meet a box in (beats, pitch) space.
pub fn in_box(notes: &[MidiNote], b0: f64, b1: f64, p_lo: i32, p_hi: i32) -> Vec<usize> {
    let (b0, b1) = (b0.min(b1), b0.max(b1));
    notes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.start < b1
                && n.start + n.length > b0
                && (n.pitch as i32) >= p_lo.min(p_hi)
                && (n.pitch as i32) <= p_lo.max(p_hi)
        })
        .map(|(i, _)| i)
        .collect()
}

/// A small, seedable random source (xorshift64*).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    /// Seeded from the clock.
    pub fn from_time() -> Rng {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9);
        Rng::new(t ^ 0x2545_F491_4F6C_DD1D)
    }
    /// 0..1.
    pub fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
    /// -1..1.
    pub fn signed(&mut self) -> f64 {
        self.unit() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(pitch: u8, start: f64, length: f64) -> MidiNote {
        MidiNote {
            pitch,
            start,
            length,
            velocity: 100,
            channel: 0,
            pan: 0.0,
        }
    }

    #[test]
    fn moves_keep_the_group_together_and_in_range() {
        let orig = vec![n(60, 1.0, 1.0), n(120, 2.0, 1.0), n(40, 0.5, 1.0)];
        let mut notes = orig.clone();
        move_from(&mut notes, &orig, &[0, 1], -3.0, 12);
        // Held at 0 by the earliest picked note, and below 127 by the highest.
        assert_eq!(notes[0].start, 0.0);
        assert_eq!(notes[1].start, 1.0);
        assert_eq!((notes[0].pitch, notes[1].pitch), (67, 127));
        assert_eq!(notes[2], orig[2]);
        resize_end_from(&mut notes, &orig, &[0, 2], -5.0);
        assert_eq!(notes[0].length, MIN_NOTE);
        resize_start_from(&mut notes, &orig, &[2], -1.0);
        assert_eq!((notes[2].start, notes[2].length), (0.0, 1.5));
    }

    #[test]
    fn copy_paste_remove_and_tools() {
        let mut notes = vec![n(60, 1.0, 1.0), n(62, 1.5, 1.0), n(64, 4.0, 1.0)];
        let c = copy(&notes, &[1, 0]);
        assert_eq!((c[0].start, c[1].start), (0.5, 0.0));
        let new = paste(&mut notes, &c, 8.0);
        assert_eq!(new, vec![3, 4]);
        assert_eq!(notes[3].start, 8.5);
        remove(&mut notes, &[0, 3]);
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].pitch, 62);
        let all = targets(notes.len(), &[]);
        scale_velocity(&mut notes, &all, 150.0);
        assert!(notes.iter().all(|n| n.velocity == 127));
        set_velocity(&mut notes, &[1], 0);
        assert_eq!(notes[1].velocity, 1);
        let mut rng = Rng::new(7);
        set_velocity(&mut notes, &all, 64);
        randomize_velocity(&mut notes, &all, 10.0, &mut rng);
        assert!(notes.iter().all(|n| (54..=74).contains(&n.velocity)));
        randomize_pan(&mut notes, &all, 0.5, &mut rng);
        assert!(notes.iter().all(|n| n.pan.abs() <= 0.5));
        humanize(&mut notes, &[0], 0.1, &mut rng);
        assert!((notes[0].start - 1.5).abs() <= 0.1);
        assert_eq!(in_box(&notes, 0.0, 2.0, 60, 63), vec![0]);
    }
}
