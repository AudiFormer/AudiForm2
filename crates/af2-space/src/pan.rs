// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// Ported from Audiform's 3D Space code, written by its author (AudiFormer)
// for Audiform and relicensed here by them. See docs/clean-room.md.

//! 3D Space sessions: speaker layouts, and placing a sound at a spot in
//! the room onto them.
//!
//! The room is a box: x runs left (-1) to right (1), y back (-1) to
//! front (1), z floor (0) to ceiling (1). Each speaker sits on that box.
//! A sound is placed by working one axis at a time: between the floor
//! and ceiling speakers by its height, then within each layer between
//! the two rows of speakers either side of it (front, side, back), then
//! within each row between the two speakers either side of it. Every
//! step shares power (cos and sin), so a sound keeps its loudness as it
//! moves. Size then blends toward all the speakers of its layers, for a
//! sound that fills the room. Our own method.
//!
//! Everything is placed on the speaker bed; listening (folded to stereo,
//! binaural, or each speaker on its own output) and exports all start
//! from that bed, so what is heard is what exports.

pub use af2_core::space::{SpaceLayout, SpacePos};

/// One speaker: its WAV channel bit and its place in the room (None for
/// the low-frequency channel, which sounds are not placed in).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Speaker {
    pub name: &'static str,
    pub mask: u32,
    pub at: Option<[f32; 3]>,
}

const FL: Speaker = Speaker {
    name: "L",
    mask: 0x1,
    at: Some([-1.0, 1.0, 0.0]),
};
const FR: Speaker = Speaker {
    name: "R",
    mask: 0x2,
    at: Some([1.0, 1.0, 0.0]),
};
const FC: Speaker = Speaker {
    name: "C",
    mask: 0x4,
    at: Some([0.0, 1.0, 0.0]),
};
const LFE: Speaker = Speaker {
    name: "LFE",
    mask: 0x8,
    at: None,
};
const BL: Speaker = Speaker {
    name: "Ls",
    mask: 0x10,
    at: Some([-1.0, -1.0, 0.0]),
};
const BR: Speaker = Speaker {
    name: "Rs",
    mask: 0x20,
    at: Some([1.0, -1.0, 0.0]),
};
const BL7: Speaker = Speaker {
    name: "Lrs",
    mask: 0x10,
    at: Some([-1.0, -1.0, 0.0]),
};
const BR7: Speaker = Speaker {
    name: "Rrs",
    mask: 0x20,
    at: Some([1.0, -1.0, 0.0]),
};
const SL: Speaker = Speaker {
    name: "Lss",
    mask: 0x200,
    at: Some([-1.0, 0.0, 0.0]),
};
const SR: Speaker = Speaker {
    name: "Rss",
    mask: 0x400,
    at: Some([1.0, 0.0, 0.0]),
};
const TML: Speaker = Speaker {
    name: "Ltm",
    mask: 0x1000,
    at: Some([-1.0, 0.0, 1.0]),
};
const TMR: Speaker = Speaker {
    name: "Rtm",
    mask: 0x4000,
    at: Some([1.0, 0.0, 1.0]),
};
const TFL: Speaker = Speaker {
    name: "Ltf",
    mask: 0x1000,
    at: Some([-1.0, 1.0, 1.0]),
};
const TFR: Speaker = Speaker {
    name: "Rtf",
    mask: 0x4000,
    at: Some([1.0, 1.0, 1.0]),
};
const TBL: Speaker = Speaker {
    name: "Ltr",
    mask: 0x8000,
    at: Some([-1.0, -1.0, 1.0]),
};
const TBR: Speaker = Speaker {
    name: "Rtr",
    mask: 0x20000,
    at: Some([1.0, -1.0, 1.0]),
};

/// Where the low-frequency channel is in a layout's channels, if it has one.
pub fn lfe_channel(layout: SpaceLayout) -> Option<usize> {
    speakers(layout).iter().position(|s| s.at.is_none())
}

/// A layout's speakers, in WAV channel order (by channel bit).
pub fn speakers(layout: SpaceLayout) -> &'static [Speaker] {
    match layout {
        SpaceLayout::Stereo20 => &[FL, FR],
        SpaceLayout::Surround51 => &[FL, FR, FC, LFE, BL, BR],
        SpaceLayout::Surround512 => &[FL, FR, FC, LFE, BL, BR, TML, TMR],
        SpaceLayout::Surround712 => &[FL, FR, FC, LFE, BL7, BR7, SL, SR, TML, TMR],
        SpaceLayout::Surround71 => &[FL, FR, FC, LFE, BL7, BR7, SL, SR],
        SpaceLayout::Surround514 => &[FL, FR, FC, LFE, BL, BR, TFL, TFR, TBL, TBR],
        SpaceLayout::Surround714 => &[FL, FR, FC, LFE, BL7, BR7, SL, SR, TFL, TFR, TBL, TBR],
    }
}

/// The WAV channel mask for a layout.
pub fn channel_mask(layout: SpaceLayout) -> u32 {
    speakers(layout).iter().map(|s| s.mask).sum()
}

/// The film (cinema) channel order for a layout: sides before rears in a
/// 7.x bed (L R C LFE Lss Rss Lrs Rrs, then heights). Output channel `k`
/// takes WAV-order channel `order[k]`. None when it is the WAV order.
pub fn film_order(layout: SpaceLayout) -> Option<Vec<usize>> {
    let sp = speakers(layout);
    let rear = sp.iter().position(|s| s.name == "Lrs")?;
    let side = sp.iter().position(|s| s.name == "Lss")?;
    let mut order: Vec<usize> = (0..sp.len()).collect();
    order[rear] = side;
    order[rear + 1] = side + 1;
    order[side] = rear;
    order[side + 1] = rear + 1;
    Some(order)
}

/// Whether FLAC can hold a layout's bed in its own channel order (FLAC
/// knows the orders of 2.0, 5.1 and 7.1 only).
pub fn flac_ok(layout: SpaceLayout) -> bool {
    matches!(
        layout,
        SpaceLayout::Stereo20 | SpaceLayout::Surround51 | SpaceLayout::Surround71
    )
}

/// What the interface shows about a layout.
#[derive(Debug, Clone)]
pub struct LayoutInfo {
    pub channels: usize,
    pub names: Vec<&'static str>,
    /// Each placed speaker: name and where it is.
    pub spots: Vec<(&'static str, [f32; 3])>,
}

pub fn layout_info(layout: SpaceLayout) -> LayoutInfo {
    let sp = speakers(layout);
    LayoutInfo {
        channels: sp.len(),
        names: sp.iter().map(|s| s.name).collect(),
        spots: sp
            .iter()
            .filter_map(|s| s.at.map(|a| (s.name, a)))
            .collect(),
    }
}

/// Shares a unit of power between two places, `t` of the way from the first to the second.
pub(crate) fn share(t: f32) -> (f32, f32) {
    let a = t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (a.cos(), a.sin())
}

/// Spreads a unit of power between the two entries of `keys` (sorted low
/// to high) either side of `v`. Returns (index, gain) pairs.
pub(crate) fn between(keys: &[f32], v: f32) -> [(usize, f32); 2] {
    if v <= keys[0] {
        return [(0, 1.0), (0, 0.0)];
    }
    let last = keys.len() - 1;
    if v >= keys[last] {
        return [(last, 1.0), (last, 0.0)];
    }
    let i = keys
        .windows(2)
        .position(|w| v >= w[0] && v <= w[1])
        .unwrap_or(0);
    let (a, b) = share((v - keys[i]) / (keys[i + 1] - keys[i]).max(1e-6));
    [(i, a), (i + 1, b)]
}

/// Speaker channels at most (7.1.4 is 12).
pub const MAX_SPEAKERS: usize = 16;

/// One gain per speaker, in channel order (unused entries 0).
pub type Gains = [f32; MAX_SPEAKERS];

/// Gains for one point, one per speaker of `layout` (in channel order).
pub fn point_gains(layout: SpaceLayout, x: f32, y: f32, z: f32, size: f32) -> Vec<f32> {
    let mut g = [0.0; MAX_SPEAKERS];
    point_gains_into(layout, x, y, z, size, &mut g);
    g[..speakers(layout).len()].to_vec()
}

/// Sorts a short list in place (no allocation).
fn sort_small<T: Copy>(v: &mut [T], key: impl Fn(&T) -> f32) {
    for i in 1..v.len() {
        let mut j = i;
        while j > 0 && key(&v[j - 1]) > key(&v[j]) {
            v.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// [`point_gains`] into a fixed array, without allocating (for the audio
/// thread).
pub fn point_gains_into(layout: SpaceLayout, x: f32, y: f32, z: f32, size: f32, g: &mut Gains) {
    g.fill(0.0);
    let sp = speakers(layout);
    let has_top = sp.iter().any(|s| s.at.is_some_and(|a| a[2] > 0.5));
    // Below ear level plays from the ear-level speakers: there are none lower.
    let (floor, top) = if has_top {
        share(z.max(0.0))
    } else {
        (1.0, 0.0)
    };
    for (layer, lg) in [(0.0f32, floor), (1.0f32, top)] {
        if lg <= 0.0 {
            continue;
        }
        // This layer's rows (distinct y), low to high.
        let mut rows = [0f32; MAX_SPEAKERS];
        let mut nrows = 0;
        for a in sp.iter().filter_map(|s| s.at) {
            if (a[2] - layer).abs() < 0.5 && !rows[..nrows].iter().any(|r| (r - a[1]).abs() < 1e-6)
            {
                rows[nrows] = a[1];
                nrows += 1;
            }
        }
        if nrows == 0 {
            continue;
        }
        sort_small(&mut rows[..nrows], |v| *v);
        for (ri, rg) in between(&rows[..nrows], y) {
            if rg <= 0.0 {
                continue;
            }
            let mut row = [(0usize, 0f32); MAX_SPEAKERS];
            let mut nrow = 0;
            for (i, s) in sp.iter().enumerate() {
                if let Some(a) = s.at {
                    if (a[2] - layer).abs() < 0.5 && (a[1] - rows[ri]).abs() < 1e-6 {
                        row[nrow] = (i, a[0]);
                        nrow += 1;
                    }
                }
            }
            sort_small(&mut row[..nrow], |r| r.1);
            let mut xs = [0f32; MAX_SPEAKERS];
            for (k, r) in row[..nrow].iter().enumerate() {
                xs[k] = r.1;
            }
            for (k, kg) in between(&xs[..nrow], x) {
                g[row[k].0] += lg * rg * kg;
            }
        }
    }
    let size = size.clamp(0.0, 1.0);
    if size > 0.0 {
        // Blend toward every placed speaker of the layers in use.
        let used = |s: &Speaker| {
            s.at.is_some_and(|a| if a[2] > 0.5 { top > 0.0 } else { floor > 0.0 })
        };
        let count = sp.iter().filter(|s| used(s)).count();
        let u = 1.0 / (count.max(1) as f32).sqrt();
        for (i, s) in sp.iter().enumerate() {
            g[i] *= 1.0 - size;
            if used(s) {
                g[i] += size * u;
            }
        }
        let p: f32 = g.iter().map(|v| v * v).sum::<f32>().sqrt();
        if p > 0.0 {
            for v in g.iter_mut() {
                *v /= p;
            }
        }
    }
}

/// The two points a source plays from: one for mono, a pair for stereo
/// (left and right spread by `width` about its spot).
pub fn source_gains(layout: SpaceLayout, pos: &SpacePos, stereo: bool) -> [Vec<f32>; 2] {
    let [l, r] = source_gains_into(layout, pos, stereo);
    let n = speakers(layout).len();
    [l[..n].to_vec(), r[..n].to_vec()]
}

/// [`source_gains`] into fixed arrays, without allocating. The spot is
/// tidied first.
pub fn source_gains_into(layout: SpaceLayout, pos: &SpacePos, stereo: bool) -> [Gains; 2] {
    let pos = pos.tidy();
    let (x, y, z, size) = (pos.x as f32, pos.y as f32, pos.z as f32, pos.size as f32);
    let mut l = [0.0; MAX_SPEAKERS];
    let mut r = [0.0; MAX_SPEAKERS];
    if !stereo {
        // A mono source has the same sound on both sides: half from each.
        point_gains_into(layout, x, y, z, size, &mut l);
        for v in l.iter_mut() {
            *v *= 0.5;
        }
        return [l, l];
    }
    let [a, b] = sides(&pos);
    point_gains_into(layout, a[0], a[1], z, size, &mut l);
    point_gains_into(layout, b[0], b[1], z, size, &mut r);
    [l, r]
}

/// Where a stereo source's left and right sides sit (x, y): `width` either
/// side of its spot, the pair turned by its rotation, kept in the room.
pub fn sides(pos: &SpacePos) -> [[f32; 2]; 2] {
    let (x, y, w) = (pos.x as f32, pos.y as f32, pos.width as f32);
    let a = (pos.rotation as f32).to_radians();
    // From the spot to the right side; clockwise seen from above (front up).
    let (dx, dy) = (w * a.cos(), -w * a.sin());
    let keep = |v: f32| v.clamp(-1.0, 1.0);
    [[keep(x - dx), keep(y - dy)], [keep(x + dx), keep(y + dy)]]
}

/// Stereo gains for listening: each point folded by where it sits left to
/// right, keeping its loudness.
pub fn fold_gains(pos: &SpacePos, stereo: bool) -> [[f32; 2]; 2] {
    let lr = |x: f64| {
        let (l, r) = share(((x as f32) + 1.0) / 2.0);
        [l, r]
    };
    if !stereo {
        let [l, r] = lr(pos.x);
        return [[l * 0.5, r * 0.5]; 2];
    }
    let [l, r] = sides(pos);
    [lr(f64::from(l[0])), lr(f64::from(r[0]))]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(g: &[f32]) -> f32 {
        g.iter().map(|v| v * v).sum()
    }

    #[test]
    fn a_point_on_a_speaker_plays_only_from_it() {
        let g = point_gains(SpaceLayout::Surround714, 0.0, 1.0, 0.0, 0.0);
        assert!((g[2] - 1.0).abs() < 1e-6, "center: {g:?}");
        assert!((power(&g) - 1.0).abs() < 1e-5);
        let g = point_gains(SpaceLayout::Surround714, -1.0, -1.0, 1.0, 0.0);
        assert!((g[10] - 1.0).abs() < 1e-6, "top rear left: {g:?}");
        // The low-frequency channel never gets a placed sound.
        assert_eq!(g[3], 0.0);
    }

    #[test]
    fn moving_keeps_loudness_and_shares_between_neighbors() {
        for layout in SpaceLayout::ALL {
            for &(x, y, z) in &[
                (0.3, 0.7, 0.0),
                (-0.8, -0.2, 0.4),
                (1.0, 0.0, 1.0),
                (0.0, 0.0, 0.5),
            ] {
                let g = point_gains(layout, x, y, z, 0.0);
                assert!(
                    (power(&g) - 1.0).abs() < 1e-4,
                    "{layout:?} {x} {y} {z}: {g:?}"
                );
                let g = point_gains(layout, x, y, z, 0.6);
                assert!((power(&g) - 1.0).abs() < 1e-4);
            }
        }
        // Halfway between left and center: both at -3 dB.
        let g = point_gains(SpaceLayout::Surround51, -0.5, 1.0, 0.0, 0.0);
        assert!(
            (g[0] - g[2]).abs() < 1e-6 && (g[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3
        );
        // A layout without height keeps a raised sound on the floor.
        let g = point_gains(SpaceLayout::Surround51, 0.0, 1.0, 1.0, 0.0);
        assert!((g[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_turned_pair_moves_its_sides() {
        let pos = SpacePos {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            rotation: 90.0,
            ..SpacePos::default()
        };
        let [l, r] = sides(&pos);
        // A quarter turn clockwise: the left side in front, the right behind.
        assert!(l[0].abs() < 1e-6 && (l[1] - 0.5).abs() < 1e-6, "{l:?}");
        assert!(r[0].abs() < 1e-6 && (r[1] + 0.5).abs() < 1e-6, "{r:?}");
    }

    #[test]
    fn masks_and_folding() {
        assert_eq!(channel_mask(SpaceLayout::Surround51), 0x3F);
        assert_eq!(channel_mask(SpaceLayout::Surround714), 0x2D63F);
        assert_eq!(speakers(SpaceLayout::Surround714).len(), 12);
        let pos = SpacePos {
            x: -1.0,
            ..SpacePos::default()
        };
        let f = fold_gains(&pos, false);
        assert!((f[0][0] - 0.5).abs() < 1e-6 && f[0][1].abs() < 1e-6);
    }

    #[test]
    fn layouts_have_their_channel_counts_and_lfe() {
        let want = [
            (SpaceLayout::Stereo20, 2, None, 0x3),
            (SpaceLayout::Surround51, 6, Some(3), 0x3F),
            (SpaceLayout::Surround71, 8, Some(3), 0x63F),
            (SpaceLayout::Surround512, 8, Some(3), 0x503F),
            (SpaceLayout::Surround712, 10, Some(3), 0x563F),
            (SpaceLayout::Surround514, 10, Some(3), 0x2D03F),
            (SpaceLayout::Surround714, 12, Some(3), 0x2D63F),
        ];
        for (l, n, lfe, mask) in want {
            assert_eq!(speakers(l).len(), n, "{l:?}");
            assert_eq!(lfe_channel(l), lfe, "{l:?}");
            assert_eq!(channel_mask(l), mask, "{l:?}");
        }
        let names: Vec<_> = speakers(SpaceLayout::Surround714)
            .iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(
            names,
            ["L", "R", "C", "LFE", "Lrs", "Rrs", "Lss", "Rss", "Ltf", "Rtf", "Ltr", "Rtr"]
        );
        let names: Vec<_> = speakers(SpaceLayout::Surround51)
            .iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["L", "R", "C", "LFE", "Ls", "Rs"]);
    }

    #[test]
    fn stereo_sources_and_tidy_spots() {
        // Full width at front centre: the sides land on L and R.
        let [l, r] = source_gains(SpaceLayout::Surround51, &SpacePos::default(), true);
        assert!((l[0] - 1.0).abs() < 1e-6 && (r[1] - 1.0).abs() < 1e-6);
        // Mono: half on each side, unit power overall.
        let [a, b] = source_gains(SpaceLayout::Surround51, &SpacePos::default(), false);
        assert!((a[2] - 0.5).abs() < 1e-6 && a == b);
        // Out of range spots are tidied first; nothing reaches the LFE.
        let wild = SpacePos {
            x: f64::NAN,
            y: 7.0,
            z: -3.0,
            ..SpacePos::default()
        };
        for layout in SpaceLayout::ALL {
            let [l, r] = source_gains(layout, &wild, true);
            if let Some(c) = lfe_channel(layout) {
                assert_eq!((l[c], r[c]), (0.0, 0.0));
            }
            assert!((power(&l) - 1.0).abs() < 1e-4);
        }
        // Below ear level plays from the floor.
        let g = point_gains(SpaceLayout::Surround714, 0.0, 1.0, -0.8, 0.0);
        assert!((g[2] - 1.0).abs() < 1e-6);
        // In 2.0, y and z do nothing.
        assert_eq!(
            point_gains(SpaceLayout::Stereo20, 0.3, -1.0, 1.0, 0.0),
            point_gains(SpaceLayout::Stereo20, 0.3, 1.0, 0.0, 0.0)
        );
    }

    #[test]
    fn film_order_puts_sides_before_rears() {
        assert_eq!(film_order(SpaceLayout::Surround51), None);
        assert_eq!(
            film_order(SpaceLayout::Surround714).unwrap(),
            [0, 1, 2, 3, 6, 7, 4, 5, 8, 9, 10, 11]
        );
        let names: Vec<_> = film_order(SpaceLayout::Surround71)
            .unwrap()
            .iter()
            .map(|&i| speakers(SpaceLayout::Surround71)[i].name)
            .collect();
        assert_eq!(names, ["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"]);
        assert!(flac_ok(SpaceLayout::Surround71) && !flac_ok(SpaceLayout::Surround512));
    }
}
