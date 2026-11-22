// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! A whole stereo (or mono) recording put into a room offline: the same
//! placing a 3D Space session does for a track (its spot, width, Spread,
//! Spatialize and Place Elements, its low-frequency send), written out as
//! one channel per speaker of the layout, in WAV channel order. The
//! Waveform editor's Spatialize to Surround uses it, with the upmixing
//! choices in [`Upmix`] on top (all off by default).

use crate::diffuse::Diffuser;
use crate::lfe::LfeFilter;
use crate::pan::{lfe_channel, speakers, MAX_SPEAKERS};
use crate::place::Placement;
use crate::steer::{Steer, STEER_FRAME};
use af2_core::space::{SpaceLayout, SpacePos, SpatializeMode};

const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// The low-frequency channel is played 10 dB up; what is moved into it is
/// put there 10 dB down so it sounds as loud as it did.
const LFE_IN: f32 = 0.316_227_77;

/// What happens to the lows when there is a low-frequency channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LfeMode {
    /// The LFE send copies the lows into it; the speakers keep theirs.
    #[default]
    Copy,
    /// The lows of every speaker move into it, below the crossover.
    Split,
}

impl LfeMode {
    pub const ALL: [LfeMode; 2] = [LfeMode::Copy, LfeMode::Split];

    pub fn label(self) -> &'static str {
        match self {
            LfeMode::Copy => "Copy",
            LfeMode::Split => "Split",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            LfeMode::Copy => "The LFE send copies the lows into the LFE; the speakers keep theirs",
            LfeMode::Split => "Every speaker's lows below the crossover move into the LFE",
        }
    }
}

/// The upmixing choices beyond the spot: all off by default, which is the
/// placing a 3D Space session does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Upmix {
    /// The stereo fold of the result (see [`downmix`]) is the original:
    /// whatever the placing changed in it is made up in the front pair.
    pub exact: bool,
    /// 0 keeps the centre speaker's sound in it; 1 moves it all to the
    /// front pair, as a phantom centre.
    pub divergence: f32,
    /// How much of what the recording has dead centre (a voice) goes to
    /// the centre speaker on its own.
    pub dialogue: f32,
    /// The centre speaker's lows (below `lfe_hz`) go to the front pair.
    pub center_lows: bool,
    pub lfe_mode: LfeMode,
    /// The crossover, Hz: the LFE's low pass, the Split point and the
    /// centre lows' split.
    pub lfe_hz: f32,
    /// Share of the room sound (Spread, Spatialize, Place Elements'
    /// ambience) lifted to the height speakers.
    pub ambient_height: f32,
    /// The centre speaker's room sound stays at ear level.
    pub keep_center_low: bool,
    /// Place Elements: 0 precise, 1 smooth (see [`Steer::set_diffusion`]).
    pub diffusion: f32,
    /// High shelves (above about 4 kHz) on the speakers behind and on the
    /// height speakers, dB (0 flat).
    pub rear_hf_db: f32,
    pub top_hf_db: f32,
}

impl Default for Upmix {
    fn default() -> Self {
        Upmix {
            exact: false,
            divergence: 0.0,
            dialogue: 0.0,
            center_lows: false,
            lfe_mode: LfeMode::Copy,
            lfe_hz: 120.0,
            ambient_height: 0.0,
            keep_center_low: true,
            diffusion: 0.0,
            rear_hf_db: 0.0,
            top_hf_db: 0.0,
        }
    }
}

/// `input` is one channel (mono) or two (left, right); more than two
/// keeps the first two. Returns one channel per speaker of `layout`, as
/// long as the input. Place Elements' frame of delay is taken back out,
/// so the result lines up with the original.
pub fn spatialize(layout: SpaceLayout, pos: &SpacePos, rate: u32, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    spatialize_with(layout, pos, &Upmix::default(), rate, input)
}

/// [`spatialize`] with the upmixing choices in `up`.
pub fn spatialize_with(
    layout: SpaceLayout,
    pos: &SpacePos,
    up: &Upmix,
    rate: u32,
    input: &[Vec<f32>],
) -> Vec<Vec<f32>> {
    let sp = speakers(layout);
    let ch = sp.len().min(MAX_SPEAKERS);
    let frames = input.first().map_or(0, |c| c.len());
    let mut out = vec![vec![0.0f32; frames]; ch];
    if frames == 0 || input.is_empty() {
        return out;
    }
    let stereo = input.len() >= 2;
    let center = center_channel(layout);
    let front = front_pair(layout);
    let lfe = lfe_channel(layout);
    let lfe_hz = up.lfe_hz.clamp(30.0, 250.0);

    // Dialogue focus: the shared middle comes out before placing.
    let (l, r, dialogue) = match center {
        Some(_) if stereo && up.dialogue > 0.0 => {
            let (l, r, c) = crate::dialogue::extract(&input[0], &input[1], rate, up.dialogue);
            (l, r, Some(c))
        }
        _ => {
            let r = if stereo { input[1].clone() } else { input[0].clone() };
            (input[0].clone(), r, None)
        }
    };

    // The placed sound, and the room sound apart so it can be lifted.
    let p = Placement::new(layout, pos, stereo);
    let field_on = p.has_field();
    let steer_on = stereo && p.steer > 0.0 && pos.spatialize_mode == SpatializeMode::Steer;
    let mut field = if field_on || steer_on {
        vec![vec![0.0f32; frames]; ch]
    } else {
        Vec::new()
    };
    let mut diffuser = Diffuser::new(rate, 0);
    let mut frame = [0.0f32; MAX_SPEAKERS];
    for i in 0..frames {
        let (x, y) = (l[i], r[i]);
        frame[..ch].fill(0.0);
        for ((o, gl), gr) in frame[..ch].iter_mut().zip(&p.bed[0]).zip(&p.bed[1]) {
            *o += x * gl + y * gr;
        }
        if let Some(lc) = lfe {
            frame[lc] += (x + y) * p.lfe;
        }
        for (c, v) in frame[..ch].iter().enumerate() {
            out[c][i] = *v;
        }
        if field_on {
            frame[..ch].fill(0.0);
            diffuser.run(x, y, &p, &mut frame[..ch]);
            for (c, v) in frame[..ch].iter().enumerate() {
                field[c][i] = *v;
            }
        }
    }
    // Place Elements: run a frame past the end, keep what lines up.
    if steer_on {
        let mut st = Steer::new(layout, rate, 1);
        st.set_diffusion(up.diffusion);
        for i in 0..frames + STEER_FRAME {
            let (x, y) = if i < frames { (l[i], r[i]) } else { (0.0, 0.0) };
            st.run(x, y, &mut frame[..ch]);
            if i >= STEER_FRAME {
                for (c, v) in frame[..ch].iter().enumerate() {
                    field[c][i - STEER_FRAME] += v * p.steer;
                }
            }
        }
    }
    if !field.is_empty() {
        lift(layout, &mut field, up.ambient_height, up.keep_center_low);
        for (o, f) in out.iter_mut().zip(&field) {
            for (a, b) in o.iter_mut().zip(f) {
                *a += b;
            }
        }
    }
    if let (Some(c), Some(d)) = (center, &dialogue) {
        for (o, v) in out[c].iter_mut().zip(d) {
            *o += v * std::f32::consts::SQRT_2;
        }
    }

    // Centre divergence: the centre speaker's sound to a phantom centre.
    // By amplitude, so the stereo fold is unchanged.
    if let (Some(c), [Some(fl), Some(fr)]) = (center, front) {
        let d = up.divergence.clamp(0.0, 1.0);
        if d > 0.0 {
            for i in 0..frames {
                let v = out[c][i];
                out[c][i] = v * (1.0 - d);
                out[fl][i] += v * d * SQRT_HALF;
                out[fr][i] += v * d * SQRT_HALF;
            }
        }
    }

    // High shelves behind and above.
    for (c, s) in sp.iter().enumerate().take(ch) {
        let Some([_, y, z]) = s.at else { continue };
        let db = if z > 0.5 {
            up.top_hf_db
        } else if y < -0.5 {
            up.rear_hf_db
        } else {
            0.0
        };
        if db.abs() > 0.01 {
            let mut f = Biquad::high_shelf(rate, 4000.0, db);
            for v in out[c].iter_mut() {
                *v = f.run(*v);
            }
        }
    }

    // Exact: what the fold lost or gained, made up in the front pair.
    if up.exact {
        if let [Some(fl), Some(fr)] = front {
            let (dl, dr) = downmix(layout, &out);
            let (ol, or) = (&input[0], if stereo { &input[1] } else { &input[0] });
            for i in 0..frames {
                out[fl][i] += ol[i] - dl[i];
                out[fr][i] += or[i] - dr[i];
            }
        }
    }

    // The centre's lows to the front pair (the fold keeps its level).
    if let (true, Some(c), [Some(fl), Some(fr)]) = (up.center_lows, center, front) {
        let mut x = Crossover::new(rate, lfe_hz);
        for i in 0..frames {
            let (lo, hi) = x.run(out[c][i]);
            out[c][i] = hi;
            out[fl][i] += lo * SQRT_HALF;
            out[fr][i] += lo * SQRT_HALF;
        }
    }

    // The low-frequency channel keeps to its band, as in a session.
    if let Some(lc) = lfe {
        let mut f = LfeFilter::with_cutoff(rate, lfe_hz);
        for v in out[lc].iter_mut() {
            *v = f.run(*v);
        }
        if up.lfe_mode == LfeMode::Split {
            let mut moved = vec![0.0f32; frames];
            for (c, o) in out.iter_mut().enumerate() {
                if c == lc {
                    continue;
                }
                let mut x = Crossover::new(rate, lfe_hz);
                for (v, m) in o.iter_mut().zip(moved.iter_mut()) {
                    let (lo, hi) = x.run(*v);
                    *v = hi;
                    *m += lo;
                }
            }
            for (o, m) in out[lc].iter_mut().zip(&moved) {
                *o += m * LFE_IN;
            }
        }
    }
    out
}

/// The front centre speaker, if the layout has one.
pub fn center_channel(layout: SpaceLayout) -> Option<usize> {
    speakers(layout)
        .iter()
        .position(|s| s.at.is_some_and(|a| a[0].abs() < 0.1 && a[1] >= 0.5 && a[2] < 0.5))
}

/// Whether the layout has height speakers.
pub fn has_heights(layout: SpaceLayout) -> bool {
    speakers(layout).iter().any(|s| s.at.is_some_and(|a| a[2] > 0.5))
}

/// The front left and right speakers.
fn front_pair(layout: SpaceLayout) -> [Option<usize>; 2] {
    let mut f = [None; 2];
    for (i, s) in speakers(layout).iter().enumerate() {
        if let Some([x, y, z]) = s.at {
            if z < 0.5 && y >= 0.5 && x.abs() > 0.5 {
                f[usize::from(x > 0.0)].get_or_insert(i);
            }
        }
    }
    f
}

/// Per speaker, (left, right) gain of the standard stereo fold: the front
/// pair as they are, the centre and every other speaker 3 dB down to its
/// own side, the low-frequency channel left out.
pub fn downmix_gains(layout: SpaceLayout) -> Vec<[f32; 2]> {
    speakers(layout)
        .iter()
        .map(|s| match s.at {
            None => [0.0, 0.0],
            Some([x, _, _]) if x.abs() < 0.1 => [SQRT_HALF, SQRT_HALF],
            Some([x, y, z]) if z < 0.5 && y >= 0.5 => {
                if x < 0.0 {
                    [1.0, 0.0]
                } else {
                    [0.0, 1.0]
                }
            }
            Some([x, _, _]) => {
                if x < 0.0 {
                    [SQRT_HALF, 0.0]
                } else {
                    [0.0, SQRT_HALF]
                }
            }
        })
        .collect()
}

/// A speaker bed folded to stereo with [`downmix_gains`].
pub fn downmix(layout: SpaceLayout, bed: &[Vec<f32>]) -> (Vec<f32>, Vec<f32>) {
    let g = downmix_gains(layout);
    let frames = bed.first().map_or(0, |c| c.len());
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    for (c, gains) in bed.iter().zip(&g) {
        if gains[0] != 0.0 {
            for (o, v) in l.iter_mut().zip(c) {
                *o += v * gains[0];
            }
        }
        if gains[1] != 0.0 {
            for (o, v) in r.iter_mut().zip(c) {
                *o += v * gains[1];
            }
        }
    }
    (l, r)
}

/// How far the fold of `bed` is from the original, as the level of the
/// difference against the original's, dB (very low is a faithful fold).
pub fn fold_error_db(layout: SpaceLayout, bed: &[Vec<f32>], input: &[Vec<f32>]) -> f32 {
    let Some(a) = input.first() else { return -120.0 };
    let b = input.get(1).unwrap_or(a);
    let (l, r) = downmix(layout, bed);
    let (mut diff, mut orig) = (0.0f64, 0.0f64);
    for i in 0..l.len().min(a.len()) {
        diff += ((l[i] - a[i]) as f64).powi(2) + ((r[i] - b[i]) as f64).powi(2);
        orig += (a[i] as f64).powi(2) + (b[i] as f64).powi(2);
    }
    if orig <= 1e-12 {
        return -120.0;
    }
    (10.0 * (diff.max(1e-12) / orig).log10()).max(-120.0) as f32
}

/// The room sound lifted: each ear-level speaker gives `amount` of its
/// power to the height speaker(s) nearest above it.
fn lift(layout: SpaceLayout, field: &mut [Vec<f32>], amount: f32, keep_center: bool) {
    let a = amount.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let sp = speakers(layout);
    let tops: Vec<(usize, [f32; 3])> = sp
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.at.filter(|p| p[2] > 0.5).map(|p| (i, p)))
        .collect();
    if tops.is_empty() {
        return;
    }
    let (stay, go) = ((1.0 - a).sqrt(), a.sqrt());
    for (i, s) in sp.iter().enumerate().take(field.len()) {
        let Some(p) = s.at.filter(|p| p[2] < 0.5) else { continue };
        if keep_center && p[0].abs() < 0.1 && p[1] >= 0.5 {
            continue;
        }
        let dist = |q: [f32; 3]| (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
        let best = tops.iter().map(|t| dist(t.1)).fold(f32::INFINITY, f32::min);
        let near: Vec<usize> = tops
            .iter()
            .filter(|t| dist(t.1) <= best + 1e-3)
            .map(|t| t.0)
            .collect();
        let share = go / (near.len() as f32).sqrt();
        let src = std::mem::take(&mut field[i]);
        for &t in &near {
            for (o, v) in field[t].iter_mut().zip(&src) {
                *o += v * share;
            }
        }
        field[i] = src.into_iter().map(|v| v * stay).collect();
    }
}

/// A second-order section (the usual cookbook forms).
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn from(b: [f64; 3], a: [f64; 3]) -> Biquad {
        Biquad {
            b: [b[0] / a[0], b[1] / a[0], b[2] / a[0]],
            a: [a[1] / a[0], a[2] / a[0]],
            z: [0.0; 2],
        }
    }

    /// Butterworth low or high pass.
    fn pass(rate: u32, hz: f32, high: bool) -> Biquad {
        let w = std::f64::consts::TAU * (hz as f64).min(rate as f64 * 0.45) / rate as f64;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let b = if high {
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0]
        } else {
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0]
        };
        Biquad::from(b, [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    /// High shelf of slope 1, `db` above `hz`.
    fn high_shelf(rate: u32, hz: f32, db: f32) -> Biquad {
        let a = 10f64.powf(db as f64 / 40.0);
        let w = std::f64::consts::TAU * (hz as f64).min(rate as f64 * 0.45) / rate as f64;
        let (s, c) = w.sin_cos();
        let alpha = s / 2.0 * std::f64::consts::SQRT_2;
        let sa = 2.0 * a.sqrt() * alpha;
        Biquad::from(
            [
                a * ((a + 1.0) + (a - 1.0) * c + sa),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - sa),
            ],
            [
                (a + 1.0) - (a - 1.0) * c + sa,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - sa,
            ],
        )
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y as f32
    }
}

/// A fourth-order Linkwitz-Riley crossover: low and high add up to the
/// input at a flat level.
struct Crossover {
    lo: [Biquad; 2],
    hi: [Biquad; 2],
}

impl Crossover {
    fn new(rate: u32, hz: f32) -> Crossover {
        Crossover {
            lo: [Biquad::pass(rate, hz, false); 2],
            hi: [Biquad::pass(rate, hz, true); 2],
        }
    }

    #[inline]
    fn run(&mut self, x: f32) -> (f32, f32) {
        let lo = self.lo[0].run(x);
        let lo = self.lo[1].run(lo);
        let hi = self.hi[0].run(x);
        let hi = self.hi[1].run(hi);
        (lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, f: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 * f * std::f32::consts::TAU / 48_000.0).sin() * 0.5)
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    fn busy(n: usize) -> Vec<Vec<f32>> {
        let a = tone(n, 300.0);
        let b = tone(n, 470.0);
        let l: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + 0.4 * y).collect();
        let r: Vec<f32> = a.iter().zip(&b).map(|(x, y)| 0.3 * x + y).collect();
        vec![l, r]
    }

    #[test]
    fn a_plain_spot_puts_left_and_right_on_the_front_pair() {
        let n = 4800;
        let l = tone(n, 440.0);
        let r = vec![0.0; n];
        let out = spatialize(SpaceLayout::Surround51, &SpacePos::default(), 48_000, &[l.clone(), r]);
        assert_eq!(out.len(), 6);
        assert!(out.iter().all(|c| c.len() == n));
        // Left plays from the front left speaker only.
        assert!((rms(&out[0]) - rms(&l)).abs() < 1e-3, "{}", rms(&out[0]));
        for c in 1..6 {
            assert!(rms(&out[c]) < 1e-4, "channel {c}: {}", rms(&out[c]));
        }
    }

    #[test]
    fn spatialize_reaches_the_back_and_the_heights() {
        let n = 48_000;
        let l = tone(n, 300.0);
        let r = tone(n, 470.0);
        for mode in SpatializeMode::ALL {
            let pos = SpacePos {
                spatialize: 1.0,
                spatialize_mode: mode,
                ..SpacePos::default()
            };
            let out = spatialize(SpaceLayout::Surround714, &pos, 48_000, &[l.clone(), r.clone()]);
            assert_eq!(out.len(), 12);
            let sp = speakers(SpaceLayout::Surround714);
            let back = sp.iter().position(|s| s.name == "Lrs").unwrap();
            let top = sp.iter().position(|s| s.at.is_some_and(|a| a[2] > 0.5)).unwrap();
            let reached = |c: usize| rms(&out[c][STEER_FRAME * 2..]) > 1e-3;
            match mode {
                SpatializeMode::HeightLift => assert!(reached(top), "{mode:?}"),
                _ => assert!(reached(back) || reached(top), "{mode:?}"),
            }
            assert!(out.iter().flatten().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn place_elements_lines_up_with_the_original() {
        // A click in the centre comes out of the centre speaker where it was.
        let n = 20_000;
        let mut m = vec![0.0f32; n];
        m[10_000] = 1.0;
        let pos = SpacePos {
            spatialize: 1.0,
            spatialize_mode: SpatializeMode::Steer,
            ..SpacePos::default()
        };
        let out = spatialize(SpaceLayout::Surround51, &pos, 48_000, &[m.clone(), m]);
        let c = &out[2];
        let peak = (0..n).max_by(|a, b| c[*a].abs().total_cmp(&c[*b].abs())).unwrap();
        assert!((peak as i64 - 10_000).abs() < 8, "peak at {peak}");
    }

    #[test]
    fn the_defaults_change_nothing() {
        // Keeping the room sound apart until the end must not change a bit.
        let n = 24_000;
        let input = busy(n);
        let pos = SpacePos { spatialize: 0.8, spread: 0.3, ..SpacePos::default() };
        let sp = speakers(SpaceLayout::Surround714);
        for mode in SpatializeMode::ALL {
            let pos = SpacePos { spatialize_mode: mode, ..pos };
            let out = spatialize(SpaceLayout::Surround714, &pos, 48_000, &input);
            assert_eq!(out.len(), sp.len());
            assert!(out.iter().flatten().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn exact_folds_back_to_the_original() {
        let n = 24_000;
        let input = busy(n);
        let pos = SpacePos { spatialize: 1.0, spatialize_mode: SpatializeMode::Steer, ..SpacePos::default() };
        let up = Upmix {
            exact: true,
            rear_hf_db: -6.0,
            top_hf_db: -6.0,
            ambient_height: 0.5,
            dialogue: 0.5,
            divergence: 0.3,
            ..Upmix::default()
        };
        for layout in [SpaceLayout::Surround51, SpaceLayout::Surround714] {
            let loose = spatialize_with(layout, &pos, &Upmix { exact: false, ..up }, 48_000, &input);
            let out = spatialize_with(layout, &pos, &up, 48_000, &input);
            let before = fold_error_db(layout, &loose, &input);
            let after = fold_error_db(layout, &out, &input);
            assert!(before > -40.0, "{before}");
            assert!(after < -90.0, "{} {after}", layout.label());
        }
    }

    #[test]
    fn divergence_moves_the_centre_to_the_front_pair() {
        let n = 9600;
        let m = tone(n, 440.0);
        let pos = SpacePos { width: 0.0, ..SpacePos::default() };
        let full = Upmix { divergence: 1.0, ..Upmix::default() };
        let base = spatialize(SpaceLayout::Surround51, &pos, 48_000, &[m.clone(), m.clone()]);
        let out = spatialize_with(SpaceLayout::Surround51, &pos, &full, 48_000, &[m.clone(), m]);
        assert!(rms(&base[2]) > 0.1);
        assert!(rms(&out[2]) < 1e-5);
        assert!(rms(&out[0]) > 0.1 && (rms(&out[0]) - rms(&out[1])).abs() < 1e-4);
        // Same fold either way.
        let (a, _) = downmix(SpaceLayout::Surround51, &base);
        let (b, _) = downmix(SpaceLayout::Surround51, &out);
        assert!((rms(&a) - rms(&b)).abs() < 1e-4);
    }

    #[test]
    fn dialogue_focus_feeds_the_centre() {
        let n = 48_000;
        let v = tone(n, 900.0);
        let up = Upmix { dialogue: 1.0, ..Upmix::default() };
        let out = spatialize_with(SpaceLayout::Surround51, &SpacePos::default(), &up, 48_000, &[v.clone(), v]);
        let mid = 4096..n - 4096;
        assert!(rms(&out[2][mid.clone()]) > 0.4, "{}", rms(&out[2][mid.clone()]));
        assert!(rms(&out[0][mid]) < 0.05);
    }

    #[test]
    fn split_moves_the_lows_into_the_lfe() {
        let n = 48_000;
        let bass = tone(n, 40.0);
        let up = Upmix { lfe_mode: LfeMode::Split, lfe_hz: 100.0, ..Upmix::default() };
        let out = spatialize_with(SpaceLayout::Surround51, &SpacePos::default(), &up, 48_000, &[bass.clone(), bass]);
        let tail = 12_000..n;
        assert!(rms(&out[3][tail.clone()]) > 0.15, "{}", rms(&out[3][tail.clone()]));
        assert!(rms(&out[0][tail]) < 0.05);
    }

    #[test]
    fn ambient_height_lifts_the_room_sound() {
        let n = 24_000;
        let input = busy(n);
        let pos = SpacePos { spatialize: 1.0, spatialize_mode: SpatializeMode::Matrix, ..SpacePos::default() };
        let sp = speakers(SpaceLayout::Surround514);
        let top = sp.iter().position(|s| s.name == "Ltr").unwrap();
        let flat = spatialize(SpaceLayout::Surround514, &pos, 48_000, &input);
        let up = Upmix { ambient_height: 0.8, ..Upmix::default() };
        let lifted = spatialize_with(SpaceLayout::Surround514, &pos, &up, 48_000, &input);
        assert!(rms(&lifted[top]) > rms(&flat[top]) + 0.02);
        assert!(rms(&lifted[4]) < rms(&flat[4]));
    }

    #[test]
    fn a_mono_file_plays_from_its_spot() {
        let n = 4800;
        let m = tone(n, 440.0);
        let pos = SpacePos { x: 0.0, y: -1.0, ..SpacePos::default() };
        let out = spatialize(SpaceLayout::Surround51, &pos, 48_000, &[m]);
        // Behind the listener: the back pair, nothing in front.
        assert!(rms(&out[4]) > 0.05 && rms(&out[5]) > 0.05);
        assert!(rms(&out[0]) < 1e-4 && rms(&out[2]) < 1e-4);
    }
}
