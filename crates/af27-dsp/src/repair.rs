// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Finding and fixing faults in spoken-word recordings: clicks and mouth
//! noise, flattened (clipped) peaks, and the quiet and loud stretches a
//! silence scan works with. The live Click Repair and Clip Repair effects
//! and the Diagnostics panel both use these, so a scan finds what the
//! effect would fix.
//!
//! * A **click** is a few samples that jump far away from where the
//!   waveform was heading. We look at the second difference (how much the
//!   slope changes from one sample to the next), measure how big it
//!   usually is nearby, and flag short bursts that stand far above that.
//! * A **clipped run** is two or more samples in a row stuck at the same
//!   side near full scale.
//! * Both are filled with a smooth curve (cubic Hermite) that leaves the
//!   good sample before with the slope it had and arrives at the good
//!   sample after with the slope it has there, so a clipped peak comes
//!   back rounded and taller than the flat top.

/// A stretch of samples: `start` and `len` in samples (or windows, for
/// the level scans).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub len: usize,
}

impl Span {
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// Reusable scratch for the finders, so the live effects never allocate
/// while running (build one with [`Scratch::with_capacity`] off the
/// audio thread).
#[derive(Debug, Default)]
pub struct Scratch {
    sort: Vec<f32>,
    scale: Vec<f32>,
    pub spans: Vec<Span>,
}

impl Scratch {
    pub fn with_capacity(samples: usize) -> Self {
        Self {
            sort: Vec::with_capacity(SCALE_BLOCK),
            scale: Vec::with_capacity(samples / SCALE_BLOCK + 2),
            spans: Vec::with_capacity(samples / 2 + 16),
        }
    }
}

/// The local size of the second difference is measured per block of
/// this many samples.
const SCALE_BLOCK: usize = 512;

fn second_diff(x: &[f32], n: usize) -> f32 {
    if n < 2 {
        0.0
    } else {
        x[n] - 2.0 * x[n - 1] + x[n - 2]
    }
}

/// How far above the usual change a click must be, from a 0..100
/// sensitivity (higher finds more).
pub fn click_factor(sensitivity: f32) -> f32 {
    30.0 - 22.0 * (sensitivity / 100.0).clamp(0.0, 1.0)
}

/// Finds clicks in `x` (one channel). `longest` is the longest click in
/// samples. The result is in `scr.spans` (capacity permitting).
pub fn find_clicks_into(x: &[f32], sensitivity: f32, longest: usize, scr: &mut Scratch) {
    scr.spans.clear();
    let n = x.len();
    if n < 8 {
        return;
    }
    // Typical size of the second difference per block (a median, which
    // a click itself barely moves), never below a floor so hiss in a
    // quiet pause is not "clicky".
    scr.scale.clear();
    for b in (0..n).step_by(SCALE_BLOCK) {
        scr.sort.clear();
        for i in b.max(2)..(b + SCALE_BLOCK).min(n) {
            scr.sort.push(second_diff(x, i).abs());
        }
        let m = if scr.sort.is_empty() {
            0.0
        } else {
            let k = scr.sort.len() / 2;
            *scr.sort.select_nth_unstable_by(k, f32::total_cmp).1
        };
        scr.scale.push(m * 1.4826);
    }
    let k = click_factor(sensitivity);
    let longest = longest.max(1);
    let mut run: Option<(usize, usize)> = None;
    let push = |spans: &mut Vec<Span>, a: usize, b: usize| {
        let start = a.saturating_sub(2);
        let end = (b + 2).min(n);
        if end - start <= longest + 4 {
            spans.push(Span {
                start,
                len: end - start,
            });
        }
    };
    for i in 2..n {
        let blk = (i / SCALE_BLOCK).min(scr.scale.len().saturating_sub(1));
        let s = scr.scale.get(blk).copied().unwrap_or(0.0);
        let thr = (s * k).max(0.004);
        if second_diff(x, i).abs() > thr {
            run = match run {
                Some((a, last)) if i - last <= longest => Some((a, i)),
                Some((a, last)) => {
                    push(&mut scr.spans, a, last);
                    Some((i, i))
                }
                None => Some((i, i)),
            };
        }
    }
    if let Some((a, last)) = run {
        push(&mut scr.spans, a, last);
    }
}

/// Finds clicks, allocating (for scans off the audio thread).
pub fn find_clicks(x: &[f32], rate: u32, sensitivity: f32, longest_ms: f32) -> Vec<Span> {
    let mut scr = Scratch::default();
    let longest = (longest_ms.max(0.01) / 1000.0 * rate as f32).ceil() as usize;
    find_clicks_into(x, sensitivity, longest, &mut scr);
    scr.spans
}

/// Finds runs of at least two samples at or above `level` (linear) on
/// the same side, no longer than `longest` samples.
pub fn find_clipped_into(x: &[f32], level: f32, longest: usize, out: &mut Vec<Span>) {
    out.clear();
    let level = level.max(1e-6);
    let mut i = 0;
    while i < x.len() {
        if x[i].abs() >= level {
            let sign = x[i] > 0.0;
            let mut j = i + 1;
            while j < x.len() && x[j].abs() >= level && (x[j] > 0.0) == sign {
                j += 1;
            }
            if j - i >= 2 && j - i <= longest.max(2) {
                out.push(Span {
                    start: i,
                    len: j - i,
                });
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

pub fn find_clipped(x: &[f32], rate: u32, level_db: f32, longest_ms: f32) -> Vec<Span> {
    let mut v = Vec::new();
    let longest = (longest_ms.max(0.01) / 1000.0 * rate as f32).ceil() as usize;
    find_clipped_into(x, 10f32.powf(level_db / 20.0), longest, &mut v);
    v
}

/// Fills `span` with a curve joining the good samples on each side
/// (their slopes kept). At the ends of `x` it holds the one good side.
pub fn fill(x: &mut [f32], span: Span) {
    let n = x.len();
    let a = span.start;
    let b = span.end();
    if span.len == 0 || a >= n {
        return;
    }
    let b = b.min(n);
    match (a >= 1, b < n) {
        (true, true) => {
            let p0 = x[a - 1];
            let p1 = x[b];
            let m0 = if a >= 2 { p0 - x[a - 2] } else { 0.0 };
            let m1 = if b + 1 < n { x[b + 1] - p1 } else { 0.0 };
            // Steps from a-1 (t=0) to b (t=1).
            let steps = (b - a + 1) as f32;
            for (k, s) in x[a..b].iter_mut().enumerate() {
                let t = (k + 1) as f32 / steps;
                let (t2, t3) = (t * t, t * t * t);
                let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                let h10 = t3 - 2.0 * t2 + t;
                let h01 = -2.0 * t3 + 3.0 * t2;
                let h11 = t3 - t2;
                *s = h00 * p0 + h10 * m0 * steps + h01 * p1 + h11 * m1 * steps;
            }
        }
        (true, false) => {
            let p0 = x[a - 1];
            x[a..b].iter_mut().for_each(|s| *s = p0);
        }
        (false, true) => {
            let p1 = x[b];
            x[a..b].iter_mut().for_each(|s| *s = p1);
        }
        (false, false) => x.iter_mut().for_each(|s| *s = 0.0),
    }
}

// ------------------------------------------------------------ levels

/// The level of `channels` (all channels together) in windows of
/// `window` samples, in dBFS RMS.
pub fn window_levels(channels: &[&[f32]], window: usize) -> Vec<f32> {
    let n = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    let window = window.max(1);
    (0..n.div_ceil(window))
        .map(|w| {
            let (a, b) = (w * window, ((w + 1) * window).min(n));
            let mut sum = 0.0f64;
            for c in channels {
                for s in &c[a.min(c.len())..b.min(c.len())] {
                    sum += (*s as f64) * (*s as f64);
                }
            }
            let ms = sum / ((b - a) as f64 * channels.len().max(1) as f64);
            (10.0 * ms.max(1e-20).log10()) as f32
        })
        .collect()
}

/// The rules a silence scan follows, in dB and windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceRule {
    /// Below this is quiet.
    pub quiet_db: f32,
    /// Quiet for at least this many windows is a silence.
    pub min_quiet: usize,
    /// Above this is sound.
    pub sound_db: f32,
    /// A burst of sound shorter than this many windows inside a silence
    /// (a cough, a bump) does not end it.
    pub min_sound: usize,
}

/// The silences in a list of window levels (spans in windows).
pub fn find_silences(levels: &[f32], rule: SilenceRule) -> Vec<Span> {
    // Mark each window: is it "sound" that counts?
    let n = levels.len();
    let mut is_sound = vec![false; n];
    let mut i = 0;
    while i < n {
        if levels[i] > rule.sound_db {
            let mut j = i;
            while j < n && levels[j] > rule.quiet_db {
                j += 1;
            }
            // A run above quiet that reached sound level: real sound if
            // long enough.
            if j - i >= rule.min_sound.max(1) {
                is_sound[i..j].iter_mut().for_each(|s| *s = true);
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    // Between bursts of real sound, a stretch that is quiet (or only
    // short sound) for long enough is a silence; trim it to its quiet
    // edges so word tails survive.
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if is_sound[i] {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && !is_sound[j] {
            j += 1;
        }
        let mut a = i;
        while a < j && levels[a] > rule.quiet_db {
            a += 1;
        }
        let mut b = j;
        while b > a && levels[b - 1] > rule.quiet_db {
            b -= 1;
        }
        if b - a >= rule.min_quiet.max(1) {
            out.push(Span {
                start: a,
                len: b - a,
            });
        }
        i = j;
    }
    out
}

/// The parts with sound: everything between the silences.
pub fn find_sound(levels: &[f32], rule: SilenceRule) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0;
    for s in find_silences(levels, rule) {
        if s.start > at {
            out.push(Span {
                start: at,
                len: s.start - at,
            });
        }
        at = s.end();
    }
    if at < levels.len() {
        out.push(Span {
            start: at,
            len: levels.len() - at,
        });
    }
    out
}

/// Suggested quiet and sound levels from the recording itself: the
/// noise floor (10th percentile) and the speaking level (75th), with the
/// quiet level a third of the way up and the sound level two thirds.
pub fn suggest_levels(levels: &[f32]) -> Option<(f32, f32)> {
    let mut v: Vec<f32> = levels.iter().copied().filter(|l| *l > -150.0).collect();
    if v.len() < 4 {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let floor = v[v.len() / 10];
    let speech = v[v.len() * 3 / 4];
    if speech - floor < 6.0 {
        return None;
    }
    let gap = speech - floor;
    Some((floor + gap / 3.0, floor + gap * 2.0 / 3.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.3 * (2.0 * std::f32::consts::PI * 180.0 * t).sin()
                    + 0.1 * (2.0 * std::f32::consts::PI * 910.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn finds_and_fills_a_click() {
        let mut x = voice(48_000);
        let clean = x.clone();
        for k in 0..6 {
            x[20_000 + k] += if k % 2 == 0 { 0.6 } else { -0.5 };
        }
        let spans = find_clicks(&x, 48_000, 50.0, 1.0);
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert!(spans[0].start <= 20_000 && spans[0].end() >= 20_006);
        fill(&mut x, spans[0]);
        let err = (19_990..20_020)
            .map(|i| (x[i] - clean[i]).abs())
            .fold(0.0, f32::max);
        assert!(err < 0.02, "{err}");
    }

    #[test]
    fn clean_speech_has_no_clicks() {
        let x = voice(48_000 * 2);
        assert!(find_clicks(&x, 48_000, 60.0, 1.0).is_empty());
    }

    #[test]
    fn finds_and_rebuilds_a_clipped_peak() {
        let clean: Vec<f32> = (0..4800)
            .map(|i| 1.2 * (2.0 * std::f32::consts::PI * 100.0 * i as f32 / 48_000.0).sin())
            .collect();
        let mut x: Vec<f32> = clean.iter().map(|s| s.clamp(-1.0, 1.0)).collect();
        let spans = find_clipped(&x, 48_000, -0.1, 3.0);
        assert!(spans.len() >= 2, "{spans:?}");
        for s in &spans {
            fill(&mut x, *s);
        }
        let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 1.1, "peak rebuilt above the flat top: {peak}");
    }

    #[test]
    fn silences_ignore_short_bumps_and_keep_word_tails() {
        // 10 ms windows: 1 s speech, 1 s silence with a 50 ms bump, 1 s speech.
        let mut lv = vec![-20.0; 100];
        lv.extend(vec![-70.0; 50]);
        lv.extend(vec![-25.0; 5]);
        lv.extend(vec![-70.0; 45]);
        lv.extend(vec![-20.0; 100]);
        let rule = SilenceRule {
            quiet_db: -50.0,
            min_quiet: 40,
            sound_db: -40.0,
            min_sound: 30,
        };
        let s = find_silences(&lv, rule);
        assert_eq!(s, vec![Span { start: 100, len: 100 }]);
        let snd = find_sound(&lv, rule);
        assert_eq!(snd.len(), 2);
        let (q, l) = suggest_levels(&lv).unwrap();
        assert!(q < l && q > -70.0 && l < -20.0, "{q} {l}");
    }
}
