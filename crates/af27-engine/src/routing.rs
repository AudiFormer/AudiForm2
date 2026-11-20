// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Which device channels sound goes to, and which input channels a track
//! records.
//!
//! Output: the stereo mix plays on a chosen pair of the device's outputs;
//! a 3D Space session can instead send each speaker of its bed to its own
//! output. A speaker without a choice plays on the output at its place in
//! the layout (L on 1, R on 2, C on 3 and so on). Outputs nothing is sent
//! to stay silent.
//!
//! Input: a track records one channel ("Mono 3") or a pair ("Stereo 1-2")
//! of a device. Every track on one device shares one input stream opened
//! with all of its channels; each take picks its channels out of the
//! interleaved frames afterwards ([`pick_channels`]).
//!
//! Everything here is pure, so it is tested without hardware.

use std::collections::BTreeMap;

use af27_core::AudioBuffer;
use serde::{Deserialize, Serialize};

use crate::MAX_SPEAKERS;

/// Settings > Channel Routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputMap {
    /// 0-based device outputs for left and right.
    pub stereo: [u16; 2],
    /// Speaker name (as in the layout, "L", "C", "Ltf") to a 0-based
    /// device output.
    pub speakers: BTreeMap<String, u16>,
    /// 3D Space sessions: each speaker to its own output, instead of the
    /// bed heard binaurally or folded on the stereo pair.
    pub speakers_out: bool,
}

impl Default for OutputMap {
    fn default() -> Self {
        OutputMap {
            stereo: [0, 1],
            speakers: BTreeMap::new(),
            speakers_out: false,
        }
    }
}

/// Where each source channel goes, resolved for one program and kept in
/// a fixed array so the audio thread can copy it without allocating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Routes {
    pub stereo: [usize; 2],
    /// Output per bed speaker (the first `bed_len`).
    pub bed: [usize; MAX_SPEAKERS],
    /// 0: the stereo mix plays; otherwise the speaker bed this wide.
    pub bed_len: usize,
}

impl Default for Routes {
    fn default() -> Self {
        Routes {
            stereo: [0, 1],
            bed: [0; MAX_SPEAKERS],
            bed_len: 0,
        }
    }
}

impl OutputMap {
    /// The output for speaker `name`, the `index`th of its layout.
    pub fn speaker(&self, name: &str, index: usize) -> usize {
        self.speakers.get(name).map_or(index, |&o| usize::from(o))
    }

    /// The routes for a program: `speakers` is the 3D layout's speaker
    /// names when the session has one.
    pub fn routes(&self, speakers: Option<&[&str]>) -> Routes {
        let mut r = Routes {
            stereo: [usize::from(self.stereo[0]), usize::from(self.stereo[1])],
            ..Routes::default()
        };
        if let (true, Some(names)) = (self.speakers_out, speakers) {
            let n = names.len().min(MAX_SPEAKERS);
            for (k, name) in names.iter().take(n).enumerate() {
                r.bed[k] = self.speaker(name, k);
            }
            r.bed_len = n;
        }
        r
    }

    /// Outputs the device needs so nothing is dropped.
    pub fn width_needed(&self, speakers: Option<&[&str]>) -> usize {
        let r = self.routes(speakers);
        let stereo = r.stereo[0].max(r.stereo[1]) + 1;
        let bed = r.bed[..r.bed_len].iter().map(|o| o + 1).max().unwrap_or(0);
        stereo.max(bed)
    }

    /// What will not be heard on a device with `width` outputs: speaker
    /// names, or "Left"/"Right" for the stereo pair. Empty when all fits.
    pub fn unplayable(&self, speakers: Option<&[&str]>, width: usize) -> Vec<String> {
        let r = self.routes(speakers);
        let mut out = Vec::new();
        if r.bed_len > 0 {
            if let Some(names) = speakers {
                for (k, name) in names.iter().take(r.bed_len).enumerate() {
                    if r.bed[k] >= width {
                        out.push((*name).to_string());
                    }
                }
            }
        } else if width >= 2 {
            for (side, o) in ["Left", "Right"].iter().zip(r.stereo) {
                if o >= width {
                    out.push((*side).to_string());
                }
            }
        }
        out
    }
}

/// Writes `src` (interleaved, `src_ch` wide: the stereo mix, or the
/// speaker bed when `routes.bed_len` matches) into `out` (`out_ch` wide,
/// the device's channels). Outputs nothing goes to are silent; a mono
/// device gets the stereo mix summed. Never allocates.
pub fn map_frames(src: &[f32], src_ch: usize, out: &mut [f32], out_ch: usize, routes: &Routes) {
    out.fill(0.0);
    let (src_ch, out_ch) = (src_ch.max(1), out_ch.max(1));
    let frames = (src.len() / src_ch).min(out.len() / out_ch);
    let bed = routes.bed_len > 0 && src_ch == routes.bed_len;
    for f in 0..frames {
        let s = &src[f * src_ch..(f + 1) * src_ch];
        let o = &mut out[f * out_ch..(f + 1) * out_ch];
        if bed {
            for (k, x) in s.iter().enumerate() {
                if let Some(d) = o.get_mut(routes.bed[k]) {
                    *d += x;
                }
            }
        } else if out_ch == 1 {
            o[0] = if src_ch >= 2 {
                (s[0] + s[1]) * 0.5
            } else {
                s[0]
            };
        } else {
            let (l, r) = (s[0], if src_ch >= 2 { s[1] } else { s[0] });
            if let Some(d) = o.get_mut(routes.stereo[0]) {
                *d += l;
            }
            if let Some(d) = o.get_mut(routes.stereo[1]) {
                *d += r;
            }
        }
    }
}

/// Adds the click (one value per frame) to the stereo pair's outputs of
/// interleaved `out`, or to the only channel of a mono device.
pub fn add_clicks_at(out: &mut [f32], out_ch: usize, clicks: &[f32], stereo: [usize; 2]) {
    let ch = out_ch.max(1);
    for (f, c) in out.chunks_exact_mut(ch).zip(clicks) {
        if *c == 0.0 {
            continue;
        }
        if ch == 1 {
            f[0] += c;
            continue;
        }
        for o in stereo {
            if let Some(d) = f.get_mut(o) {
                *d += c;
            }
        }
        if stereo[0] == stereo[1] {
            // One output for both sides: once, not twice.
            if let Some(d) = f.get_mut(stereo[0]) {
                *d -= c;
            }
        }
    }
}

// ------------------------------------------------------------ inputs

/// One or two channels of an input device, for a track.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputPick {
    /// The device by name; None is the recording device in Settings (or
    /// the system default).
    pub device: Option<String>,
    /// 0-based channels: one (mono) or two (stereo).
    pub channels: Vec<u16>,
}

impl InputPick {
    /// "Mono 3" or "Stereo 1-2" (1-based, as on the hardware).
    pub fn channels_label(&self) -> String {
        match self.channels.as_slice() {
            [a] => format!("Mono {}", a + 1),
            [a, b] => format!("Stereo {}-{}", a + 1, b + 1),
            other => {
                let list: Vec<String> = other.iter().map(|c| (c + 1).to_string()).collect();
                format!("Channels {}", list.join(", "))
            }
        }
    }

    /// The device and its channels, for menus and take names.
    pub fn label(&self) -> String {
        let dev = self.device.as_deref().unwrap_or("Default input");
        format!("{dev}: {}", self.channels_label())
    }
}

/// The picks a device with `channels` inputs offers, in menu order: one
/// mono entry per channel, then the stereo pairs 1-2, 3-4 and so on.
pub fn input_choices(channels: u16) -> Vec<Vec<u16>> {
    let mut out: Vec<Vec<u16>> = (0..channels).map(|c| vec![c]).collect();
    let mut a = 0;
    while a + 1 < channels {
        out.push(vec![a, a + 1]);
        a += 2;
    }
    out
}

/// The channels `pick` (0-based) out of `interleaved` frames from a device
/// with `dev_ch` channels. A channel the device lacks records silence.
pub fn pick_channels(interleaved: &[f32], dev_ch: usize, rate: u32, pick: &[u16]) -> AudioBuffer {
    let dev_ch = dev_ch.max(1);
    let frames = interleaved.len() / dev_ch;
    let channels = pick
        .iter()
        .map(|&c| {
            let c = usize::from(c);
            if c < dev_ch {
                (0..frames).map(|f| interleaved[f * dev_ch + c]).collect()
            } else {
                vec![0.0; frames]
            }
        })
        .collect();
    AudioBuffer { rate, channels }
}

/// Reads Audiform 1's track input strings ("Mono: <device> <n>",
/// "Stereo: <device> <a>-<b>", 1-based), for sessions made there. The
/// device name may contain spaces; the channels follow the last one.
pub fn parse_af1_input(s: &str) -> Option<InputPick> {
    let (kind, rest) = s.split_once(": ")?;
    let (device, chans) = rest.rsplit_once(' ')?;
    let num = |t: &str| {
        t.trim()
            .parse::<u16>()
            .ok()
            .filter(|&n| n >= 1)
            .map(|n| n - 1)
    };
    let channels = match kind {
        "Mono" => vec![num(chans)?],
        "Stereo" => {
            let (a, b) = chans.split_once('-')?;
            vec![num(a)?, num(b)?]
        }
        _ => return None,
    };
    Some(InputPick {
        device: Some(device.to_string()).filter(|d| !d.is_empty()),
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const L51: [&str; 6] = ["L", "R", "C", "LFE", "Ls", "Rs"];

    fn frames(out: &[f32], ch: usize) -> Vec<Vec<f32>> {
        out.chunks(ch).map(|f| f.to_vec()).collect()
    }

    #[test]
    fn stereo_plays_on_the_picked_pair() {
        let map = OutputMap {
            stereo: [2, 3],
            ..Default::default()
        };
        let r = map.routes(None);
        let src = [0.5, -0.25, 0.1, 0.2];
        let mut out = [9.0; 8];
        map_frames(&src, 2, &mut out, 4, &r);
        assert_eq!(
            frames(&out, 4),
            vec![vec![0.0, 0.0, 0.5, -0.25], vec![0.0, 0.0, 0.1, 0.2]]
        );
    }

    #[test]
    fn extra_device_channels_stay_silent() {
        let r = OutputMap::default().routes(None);
        let src = [1.0, 2.0];
        let mut out = [7.0; 6];
        map_frames(&src, 2, &mut out, 6, &r);
        assert_eq!(out, [1.0, 2.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_mono_device_gets_the_sum() {
        let r = OutputMap::default().routes(None);
        let mut out = [0.0; 2];
        map_frames(&[1.0, 0.0, 0.5, 0.5], 2, &mut out, 1, &r);
        assert_eq!(out, [0.5, 0.5]);
    }

    #[test]
    fn speakers_play_on_their_mapped_outputs() {
        let mut map = OutputMap {
            speakers_out: true,
            ..Default::default()
        };
        // L on 3, R on 1, C on 2 (0-based 2, 0, 1); the rest by place.
        map.speakers.insert("L".into(), 2);
        map.speakers.insert("R".into(), 0);
        map.speakers.insert("C".into(), 1);
        let r = map.routes(Some(&L51[..3]));
        assert_eq!(r.bed_len, 3);
        let src = [0.1, 0.2, 0.3];
        let mut out = [5.0; 4];
        map_frames(&src, 3, &mut out, 4, &r);
        // Output 4 silent; the stereo pair is not used.
        assert_eq!(out, [0.2, 0.3, 0.1, 0.0]);
    }

    #[test]
    fn a_7_1_4_bed_maps_one_to_one_by_default() {
        let names = [
            "L", "R", "C", "LFE", "Lrs", "Rrs", "Lss", "Rss", "Ltf", "Rtf", "Ltr", "Rtr",
        ];
        let map = OutputMap {
            speakers_out: true,
            ..Default::default()
        };
        let r = map.routes(Some(&names));
        assert_eq!(r.bed_len, 12);
        assert_eq!(&r.bed[..12], &(0..12).collect::<Vec<_>>()[..]);
        assert_eq!(map.width_needed(Some(&names)), 12);
        let src: Vec<f32> = (0..12).map(|k| k as f32).collect();
        let mut out = vec![0.0; 12];
        map_frames(&src, 12, &mut out, 12, &r);
        assert_eq!(out, src);
    }

    #[test]
    fn speakers_beyond_the_device_are_reported_not_silently_dropped() {
        let map = OutputMap {
            speakers_out: true,
            ..Default::default()
        };
        // A 5.1 bed on a 4-output interface: Ls and Rs have nowhere to go.
        assert_eq!(
            map.unplayable(Some(&L51), 4),
            vec!["Ls".to_string(), "Rs".to_string()]
        );
        assert!(map.unplayable(Some(&L51), 6).is_empty());
        let wide = OutputMap {
            stereo: [4, 5],
            ..Default::default()
        };
        assert_eq!(wide.unplayable(None, 4), vec!["Left", "Right"]);
        assert_eq!(wide.width_needed(None), 6);
        // Speakers off: the bed is heard on the stereo pair.
        assert!(OutputMap::default().unplayable(Some(&L51), 2).is_empty());
    }

    #[test]
    fn the_stereo_mix_plays_when_speakers_are_off_or_the_bed_differs() {
        let map = OutputMap::default();
        let r = map.routes(Some(&L51));
        assert_eq!(r.bed_len, 0);
        // A two-wide source with bed routes for six: still stereo.
        let on = OutputMap {
            speakers_out: true,
            ..Default::default()
        }
        .routes(Some(&L51));
        let mut out = [0.0; 6];
        map_frames(&[1.0, 2.0], 2, &mut out, 6, &on);
        assert_eq!(out, [1.0, 2.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_click_goes_to_the_stereo_pair() {
        let mut out = [0.0; 8];
        add_clicks_at(&mut out, 4, &[1.0, 0.0], [2, 3]);
        assert_eq!(out, [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        let mut one = [0.0; 4];
        add_clicks_at(&mut one, 2, &[0.5, 0.5], [1, 1]);
        assert_eq!(one, [0.0, 0.5, 0.0, 0.5]);
        let mut mono = [0.0; 2];
        add_clicks_at(&mut mono, 1, &[0.25, 0.0], [0, 1]);
        assert_eq!(mono, [0.25, 0.0]);
    }

    #[test]
    fn inputs_offer_each_channel_then_pairs() {
        assert_eq!(
            input_choices(4),
            vec![vec![0], vec![1], vec![2], vec![3], vec![0, 1], vec![2, 3]]
        );
        assert_eq!(input_choices(1), vec![vec![0]]);
        assert_eq!(input_choices(3).len(), 4);
        let p = InputPick {
            device: Some("SSL 2+ MKII".into()),
            channels: vec![0, 1],
        };
        assert_eq!(p.label(), "SSL 2+ MKII: Stereo 1-2");
        assert_eq!(
            InputPick {
                device: None,
                channels: vec![2]
            }
            .label(),
            "Default input: Mono 3"
        );
    }

    #[test]
    fn takes_pick_their_channels_from_the_device_frames() {
        // Three device channels, two frames.
        let inter = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mono = pick_channels(&inter, 3, 48_000, &[1]);
        assert_eq!(mono.channels, vec![vec![2.0, 5.0]]);
        let pair = pick_channels(&inter, 3, 48_000, &[2, 0]);
        assert_eq!(pair.channels, vec![vec![3.0, 6.0], vec![1.0, 4.0]]);
        // A channel the device lacks records silence, at the right length.
        let past = pick_channels(&inter, 3, 48_000, &[7]);
        assert_eq!(past.channels, vec![vec![0.0, 0.0]]);
        assert_eq!(past.rate, 48_000);
    }

    #[test]
    fn audiform_1_input_strings_are_read() {
        assert_eq!(
            parse_af1_input("Mono: SSL 2+ MKII 2"),
            Some(InputPick {
                device: Some("SSL 2+ MKII".into()),
                channels: vec![1]
            })
        );
        assert_eq!(
            parse_af1_input("Stereo: Focusrite USB ASIO 3-4"),
            Some(InputPick {
                device: Some("Focusrite USB ASIO".into()),
                channels: vec![2, 3]
            })
        );
        assert_eq!(parse_af1_input("none"), None);
        assert_eq!(parse_af1_input("Mono: Mic 0"), None);
        assert_eq!(parse_af1_input("Stereo: Mic 1"), None);
    }
}
