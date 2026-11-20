// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Which channels of the open file are switched on in the Waveform
//! editor. Click a channel's name at the right of its lane to switch it
//! off or on; Ctrl+click to hear and edit that channel alone. A channel
//! that is off is silent in playback, and edits, effects, copy and paste
//! pass it by. It is a view setting of the open file, never saved into
//! the audio.

use af27_core::{AudioBuffer, Range};

/// Channels past this many are always on (no layout comes near it).
pub const MAX_SWITCHED: usize = 64;

/// One on/off switch per channel of a file. At least one is always on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelSet {
    n: usize,
    /// Bit `k` set: channel `k` is off.
    off: u64,
}

impl ChannelSet {
    /// Every channel of an `n`-channel file on.
    pub fn all(n: usize) -> ChannelSet {
        ChannelSet {
            n: n.max(1),
            off: 0,
        }
    }

    pub fn is_on(&self, k: usize) -> bool {
        k >= MAX_SWITCHED || self.off & (1u64 << k) == 0
    }

    /// Every channel on: edits reach the whole file.
    pub fn all_on(&self) -> bool {
        self.off == 0
    }

    /// Some channels are off, so edits reach only part of the file.
    pub fn some_off(&self) -> bool {
        !self.all_on()
    }

    /// The channels that are on, in order.
    pub fn on(&self) -> Vec<usize> {
        (0..self.n).filter(|&k| self.is_on(k)).collect()
    }

    /// Bits of the channels that are on (bit 0 the first channel).
    pub fn on_mask(&self) -> u64 {
        let all = if self.n >= 64 {
            u64::MAX
        } else {
            (1u64 << self.n) - 1
        };
        all & !self.off
    }

    /// The same switches for a file with `n` channels: all on again when
    /// the channel count changed (an undo past a channel conversion).
    pub fn fit(self, n: usize) -> ChannelSet {
        if self.n == n.max(1) {
            self
        } else {
            ChannelSet::all(n)
        }
    }

    /// Click: channel `k` off, or on again. Switching off the last
    /// channel still on turns every other channel on instead, so the
    /// click still changes what is heard (in stereo it swaps sides).
    pub fn toggle(self, k: usize) -> ChannelSet {
        if k >= self.n.min(MAX_SWITCHED) || self.n == 1 {
            return self;
        }
        let next = ChannelSet {
            off: self.off ^ (1u64 << k),
            ..self
        };
        if next.on_mask() == 0 {
            // Everything but `k`.
            ChannelSet {
                off: 1u64 << k,
                ..self
            }
        } else {
            next
        }
    }

    /// Ctrl+click: channel `k` alone, or every channel again when it is
    /// already alone.
    pub fn solo(self, k: usize) -> ChannelSet {
        if k >= self.n.min(MAX_SWITCHED) || self.n == 1 {
            return self;
        }
        let alone = ChannelSet {
            off: !(1u64 << k) & ChannelSet::all(self.n).on_mask(),
            ..self
        };
        if self == alone {
            ChannelSet::all(self.n)
        } else {
            alone
        }
    }

    /// " (L)", " (C, LFE)" for an edit's name in the history; "" with
    /// every channel on.
    pub fn edit_suffix(&self) -> String {
        if self.all_on() {
            return String::new();
        }
        let names: Vec<&str> = self
            .on()
            .into_iter()
            .map(|k| crate::waveform::channel_name(self.n, k))
            .collect();
        format!(" ({})", names.join(", "))
    }

    /// `edited` with the channels that are off put back as they were in
    /// `old`. Buffers of another shape (an edit that changed the length
    /// or the channel count) come back as they are.
    pub fn merge(&self, old: &AudioBuffer, mut edited: AudioBuffer) -> AudioBuffer {
        if self.all_on()
            || old.num_channels() != edited.num_channels()
            || old.frames() != edited.frames()
        {
            return edited;
        }
        for (k, ch) in edited.channels.iter_mut().enumerate() {
            if !self.is_on(k) {
                ch.copy_from_slice(&old.channels[k]);
            }
        }
        edited
    }

    /// An effect's output for a `len`-frame selection: with channels off
    /// it keeps the selection's length (cut short or padded with silence),
    /// since the channels left alone cannot change length.
    pub fn fit_len(&self, mut out: AudioBuffer, len: usize) -> AudioBuffer {
        if self.some_off() {
            for ch in &mut out.channels {
                ch.resize(len, 0.0);
            }
        }
        out
    }

    /// Silence in range `r` of the channels that are on (Delete and Cut
    /// with channels off: the length stays).
    pub fn silence(&self, buf: &mut AudioBuffer, r: Range) {
        let r = r.clamp(buf.frames());
        for (k, ch) in buf.channels.iter_mut().enumerate() {
            if self.is_on(k) {
                ch[r.start..r.end].fill(0.0);
            }
        }
    }

    /// Silence outside range `r` of the channels that are on (Crop with
    /// channels off).
    pub fn silence_outside(&self, buf: &mut AudioBuffer, r: Range) {
        let r = r.clamp(buf.frames());
        for (k, ch) in buf.channels.iter_mut().enumerate() {
            if self.is_on(k) {
                ch[..r.start].fill(0.0);
                ch[r.end..].fill(0.0);
            }
        }
    }

    /// The channels that are on, alone (what Copy takes).
    pub fn pick(&self, buf: &AudioBuffer) -> AudioBuffer {
        if self.all_on() {
            return buf.clone();
        }
        AudioBuffer {
            rate: buf.rate,
            channels: self
                .on()
                .into_iter()
                .filter_map(|k| buf.channels.get(k).cloned())
                .collect(),
        }
    }

    /// Paste with channels off: `clip` (one channel per channel that is
    /// on, or one channel for them all) written over the channels that
    /// are on from frame `at`. The others keep their audio; the file
    /// grows with silence if the clip runs past its end.
    pub fn overwrite(&self, buf: &mut AudioBuffer, at: usize, clip: &AudioBuffer) {
        let len = clip.frames();
        if at + len > buf.frames() {
            let n = at + len;
            for ch in &mut buf.channels {
                ch.resize(n, 0.0);
            }
        }
        if clip.num_channels() == 0 {
            return;
        }
        for (i, k) in self.on().into_iter().enumerate() {
            let src = &clip.channels[i.min(clip.num_channels() - 1)];
            if let Some(ch) = buf.channels.get_mut(k) {
                ch[at..at + len].copy_from_slice(src);
            }
        }
    }

    /// What playback hears: `buf` with the channels that are off silent,
    /// or None with every channel on (play `buf` itself).
    pub fn audible(&self, buf: &AudioBuffer) -> Option<AudioBuffer> {
        if self.all_on() {
            return None;
        }
        let mut out = buf.clone();
        for (k, ch) in out.channels.iter_mut().enumerate() {
            if !self.is_on(k) {
                ch.fill(0.0);
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(n: usize, frames: usize) -> AudioBuffer {
        AudioBuffer {
            rate: 48_000,
            channels: (0..n)
                .map(|k| (0..frames).map(|i| (k * 100 + i) as f32).collect())
                .collect(),
        }
    }

    #[test]
    fn toggle_and_never_all_off() {
        let s = ChannelSet::all(2);
        let l_off = s.toggle(0);
        assert!(!l_off.is_on(0) && l_off.is_on(1));
        // Switching off the last one on swaps to the others.
        let swapped = l_off.toggle(1);
        assert!(swapped.is_on(0) && !swapped.is_on(1));
        assert_eq!(l_off.toggle(0), s);
        // Mono stays on.
        assert_eq!(ChannelSet::all(1).toggle(0), ChannelSet::all(1));
        assert_eq!(ChannelSet::all(1).solo(0), ChannelSet::all(1));
    }

    #[test]
    fn last_one_on_in_surround_turns_the_rest_on() {
        let s = ChannelSet::all(8).solo(2);
        assert_eq!(s.on(), vec![2]);
        let t = s.toggle(2);
        assert_eq!(t.on(), vec![0, 1, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn solo_and_back() {
        let s = ChannelSet::all(12);
        let c = s.solo(2);
        assert_eq!(c.on(), vec![2]);
        assert_eq!(c.solo(2), s);
        // Solo another from a solo moves it.
        assert_eq!(c.solo(11).on(), vec![11]);
        // Solo from a partial set.
        assert_eq!(s.toggle(0).toggle(1).solo(5).on(), vec![5]);
        assert_eq!(c.on_mask(), 0b100);
        assert_eq!(s.on_mask(), 0xFFF);
    }

    #[test]
    fn out_of_range_and_fit() {
        let s = ChannelSet::all(2);
        assert_eq!(s.toggle(5), s);
        let off = s.toggle(1);
        assert_eq!(off.fit(2), off);
        assert_eq!(off.fit(6), ChannelSet::all(6));
        assert!(ChannelSet::all(80).is_on(70));
        assert_eq!(ChannelSet::all(64).on_mask(), u64::MAX);
    }

    #[test]
    fn suffix_names_the_channels_on() {
        assert_eq!(ChannelSet::all(2).edit_suffix(), "");
        assert_eq!(ChannelSet::all(2).toggle(1).edit_suffix(), " (L)");
        let s = ChannelSet::all(8).solo(2).toggle(3);
        assert_eq!(s.edit_suffix(), " (C, LFE)");
    }

    #[test]
    fn merge_keeps_off_channels() {
        let old = buf(3, 4);
        let mut edited = old.clone();
        for ch in &mut edited.channels {
            ch.fill(9.0);
        }
        let s = ChannelSet::all(3).toggle(1);
        let m = s.merge(&old, edited.clone());
        assert_eq!(m.channels[0], vec![9.0; 4]);
        assert_eq!(m.channels[1], old.channels[1]);
        assert_eq!(m.channels[2], vec![9.0; 4]);
        // All on, or another shape: as edited.
        assert_eq!(ChannelSet::all(3).merge(&old, edited.clone()), edited);
        let short = buf(3, 2);
        assert_eq!(s.merge(&old, short.clone()), short);
    }

    #[test]
    fn silence_crop_and_fit_len() {
        let s = ChannelSet::all(2).toggle(0);
        let mut b = buf(2, 6);
        s.silence(&mut b, Range::new(2, 4));
        assert_eq!(b.channels[0], buf(2, 6).channels[0]);
        assert_eq!(b.channels[1], vec![100.0, 101.0, 0.0, 0.0, 104.0, 105.0]);
        let mut b = buf(2, 6);
        s.silence_outside(&mut b, Range::new(2, 4));
        assert_eq!(b.frames(), 6);
        assert_eq!(b.channels[1], vec![0.0, 0.0, 102.0, 103.0, 0.0, 0.0]);
        assert_eq!(s.fit_len(buf(2, 9), 6).frames(), 6);
        assert_eq!(s.fit_len(buf(2, 3), 6).channels[1][5], 0.0);
        assert_eq!(ChannelSet::all(2).fit_len(buf(2, 9), 6).frames(), 9);
    }

    #[test]
    fn copy_then_paste_into_another_channel() {
        let src = buf(2, 4);
        let left = ChannelSet::all(2).toggle(1);
        let clip = left.pick(&src.slice(Range::new(1, 3)));
        assert_eq!(clip.num_channels(), 1);
        assert_eq!(clip.channels[0], vec![1.0, 2.0]);
        let right = ChannelSet::all(2).toggle(0);
        let mut b = src.clone();
        right.overwrite(&mut b, 3, &clip);
        // Grows by one frame; left untouched (silence where it grew).
        assert_eq!(b.frames(), 5);
        assert_eq!(b.channels[0], vec![0.0, 1.0, 2.0, 3.0, 0.0]);
        assert_eq!(b.channels[1], vec![100.0, 101.0, 102.0, 1.0, 2.0]);
    }

    #[test]
    fn playback_silences_off_channels() {
        let b = buf(12, 3);
        assert!(ChannelSet::all(12).audible(&b).is_none());
        let s = ChannelSet::all(12).toggle(3).toggle(11);
        let a = s.audible(&b).unwrap();
        assert_eq!(a.channels[3], vec![0.0; 3]);
        assert_eq!(a.channels[11], vec![0.0; 3]);
        assert_eq!(a.channels[0], b.channels[0]);
    }
}
