// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Spoken-word dynamics written for AudiForm 2: the Speech Leveler and
//! the Breath Gate. Their slot ids and control keys are the ones Audiform
//! sessions save (`altaudio.speech_leveler`, `altaudio.breath_gate`), so a
//! voice chain from an `.audfrm` file plays here; the sound is our own.
//!
//! * **Speech Leveler** listens to the level of the voice over a short
//!   window and slowly turns the track up or down towards a target, like
//!   a hand riding the fader. Below "Ignore below" it is a pause: the
//!   gain holds where it was, so room tone is never pumped up.
//! * **Breath Gate** turns the sound down to a closed level when it falls
//!   under the threshold (between phrases), with attack, hold and release
//!   so word ends are not chopped. A sidechain key can open it instead.

use super::{num, EffectInfo, Params, Processor};

pub const LEVELER: &str = "af.speech_leveler";
pub const GATE: &str = "af.breath_gate";

pub fn leveler_info() -> EffectInfo {
    EffectInfo {
        id: LEVELER,
        sidechain: false,
        name: "Speech Leveler",
        category: "Dynamics",
        description: "Evens out a speaker who drifts louder and softer: rides the voice towards a target level and holds still during pauses.",
        params: vec![
            num("target", "Target level", "dB", -40.0, -6.0, -20.0, 0.1),
            num("max_gain", "Most boost", "dB", 0.0, 30.0, 10.0, 0.1),
            num("max_cut", "Most cut", "dB", 0.0, 30.0, 10.0, 0.1),
            num("speed", "Response", "ms", 100.0, 10_000.0, 1500.0, 1.0).log(),
            num("floor", "Ignore below", "dB", -80.0, -20.0, -50.0, 0.1),
        ],
    }
}

pub fn gate_info() -> EffectInfo {
    EffectInfo {
        id: GATE,
        sidechain: true,
        name: "Breath Gate",
        category: "Dynamics",
        description: "Drops breaths, clicks and room noise between phrases by closing when the voice goes quiet.",
        params: vec![
            num("threshold", "Threshold", "dB", -80.0, 0.0, -40.0, 0.1),
            num("attack", "Attack", "ms", 0.1, 100.0, 2.0, 0.1).log(),
            num("hold", "Hold", "ms", 0.0, 1000.0, 50.0, 1.0),
            num("release", "Release", "ms", 5.0, 2000.0, 200.0, 1.0).log(),
            num("range", "Closed level", "dB", -100.0, 0.0, -60.0, 0.5),
        ],
    }
}

fn coef(ms: f64, rate: u32) -> f32 {
    let n = (ms.max(0.01) / 1000.0) * rate as f64;
    (-1.0 / n.max(1.0)).exp() as f32
}

fn to_db(x: f32) -> f32 {
    10.0 * x.max(1e-20).log10()
}

// ------------------------------------------------------------ Leveler

pub struct SpeechLeveler {
    rate: u32,
    target: f32,
    max_gain: f32,
    max_cut: f32,
    floor: f32,
    /// One-pole on the mean square (about 60 ms): the "level now".
    ms_coef: f32,
    /// How quickly the gain (in dB) moves towards where it wants to be.
    ride_coef: f32,
    power: f32,
    /// A quicker follower (about 10 ms) that tells speech from pauses.
    fast: f32,
    fast_coef: f32,
    gain_db: f32,
}

impl SpeechLeveler {
    pub fn new(p: &Params, rate: u32) -> Self {
        let mut s = Self {
            rate,
            target: 0.0,
            max_gain: 0.0,
            max_cut: 0.0,
            floor: 0.0,
            ms_coef: coef(60.0, rate),
            ride_coef: 0.0,
            power: 0.0,
            fast: 0.0,
            fast_coef: coef(10.0, rate),
            gain_db: 0.0,
        };
        s.set(p);
        s
    }

    fn set(&mut self, p: &Params) {
        self.target = p.get("target") as f32;
        self.max_gain = p.get("max_gain") as f32;
        self.max_cut = p.get("max_cut") as f32;
        self.floor = p.get("floor") as f32;
        self.ride_coef = coef(p.get("speed"), self.rate);
    }

    /// The gain applied now, in dB (for tests and meters).
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }
}

impl Processor for SpeechLeveler {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.as_chunks_mut::<2>().0 {
            let sq = (f[0] * f[0] + f[1] * f[1]) * 0.5;
            self.power = sq + (self.power - sq) * self.ms_coef;
            self.fast = sq + (self.fast - sq) * self.fast_coef;
            let level = to_db(self.power);
            if to_db(self.fast) > self.floor {
                // Speech: aim for the target, within the limits.
                let want = (self.target - level).clamp(-self.max_cut, self.max_gain);
                self.gain_db = want + (self.gain_db - want) * self.ride_coef;
            }
            let g = 10f32.powf(self.gain_db / 20.0);
            f[0] *= g;
            f[1] *= g;
        }
    }

    fn reset(&mut self) {
        self.power = 0.0;
        self.fast = 0.0;
        self.gain_db = 0.0;
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        true
    }
}

// --------------------------------------------------------------- Gate

pub struct BreathGate {
    rate: u32,
    open_at: f32,
    /// The gate closes again a little lower (3 dB), so a voice hovering
    /// at the threshold does not chatter.
    close_at: f32,
    /// dB the gain moves per sample opening and closing: the whole
    /// range in the attack or release time.
    attack: f32,
    release: f32,
    hold: usize,
    /// The closed level, in dB.
    closed: f32,
    /// Peak follower of the detector (fast up, 10 ms down).
    env: f32,
    env_fall: f32,
    open: bool,
    held: usize,
    /// The gain now, in dB.
    gain: f32,
}

impl BreathGate {
    pub fn new(p: &Params, rate: u32) -> Self {
        let mut g = Self {
            rate,
            open_at: 0.0,
            close_at: 0.0,
            attack: 0.0,
            release: 0.0,
            hold: 0,
            closed: 0.0,
            env: 0.0,
            env_fall: coef(10.0, rate),
            open: false,
            held: 0,
            gain: 0.0,
        };
        g.set(p);
        g.gain = g.closed;
        g
    }

    fn set(&mut self, p: &Params) {
        let t = p.get("threshold") as f32;
        self.open_at = 10f32.powf(t / 20.0);
        self.close_at = 10f32.powf((t - 3.0) / 20.0);
        let range = (p.get("range") as f32).min(-0.01);
        let per = |ms: f64| -range / ((ms / 1000.0) * self.rate as f64).max(1.0) as f32;
        self.attack = per(p.get("attack"));
        self.release = per(p.get("release"));
        self.hold = (p.get("hold") / 1000.0 * self.rate as f64) as usize;
        self.closed = range;
    }

    fn run(&mut self, buf: &mut [f32], key: Option<&[f32]>) {
        let frames = buf.len() / 2;
        for i in 0..frames {
            let (dl, dr) = match key {
                Some(k) if k.len() >= 2 * i + 2 => (k[2 * i], k[2 * i + 1]),
                _ => (buf[2 * i], buf[2 * i + 1]),
            };
            let x = dl.abs().max(dr.abs());
            self.env = if x > self.env {
                x
            } else {
                x + (self.env - x) * self.env_fall
            };
            if self.env >= self.open_at {
                self.open = true;
                self.held = self.hold;
            } else if self.env < self.close_at {
                if self.held > 0 {
                    self.held -= 1;
                } else {
                    self.open = false;
                }
            }
            self.gain = if self.open {
                (self.gain + self.attack).min(0.0)
            } else {
                (self.gain - self.release).max(self.closed)
            };
            let g = 10f32.powf(self.gain / 20.0);
            buf[2 * i] *= g;
            buf[2 * i + 1] *= g;
        }
    }
}

impl Processor for BreathGate {
    fn process(&mut self, buf: &mut [f32]) {
        self.run(buf, None);
    }

    fn process_keyed(&mut self, buf: &mut [f32], key: &[f32]) {
        self.run(buf, Some(key));
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.open = false;
        self.held = 0;
        self.gain = self.closed;
    }

    fn update(&mut self, p: &Params) -> bool {
        self.set(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build, params_from};

    fn tone(frames: usize, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (i as f32 * 0.0575).sin() * amp;
                [v, v]
            })
            .collect()
    }

    fn peak(b: &[f32]) -> f32 {
        b.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    fn run(id: &str, set: &[(&str, f64)], buf: &mut [f32]) {
        let pairs: Vec<(String, f64)> = set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let p = params_from(id, &pairs).unwrap();
        let mut fx = build(id, 48_000, &p).unwrap();
        for b in buf.chunks_mut(512) {
            fx.process(b);
        }
    }

    #[test]
    fn the_leveler_brings_quiet_and_loud_speech_towards_the_target() {
        // -30 dB RMS-ish speech and -10 dB speech, target -20.
        let mut quiet = tone(48_000 * 6, 0.045);
        let mut loud = tone(48_000 * 6, 0.45);
        let set = [("speed", 300.0)];
        run(LEVELER, &set, &mut quiet);
        run(LEVELER, &set, &mut loud);
        let n = quiet.len();
        let (q, l) = (peak(&quiet[n - 4800..]), peak(&loud[n - 4800..]));
        // Both end near the same level (the target, about 0.14 peak).
        assert!((q / l - 1.0).abs() < 0.15, "{q} vs {l}");
        assert!((0.1..0.2).contains(&q), "{q}");
    }

    #[test]
    fn the_leveler_holds_in_pauses() {
        let p = params_from(LEVELER, &[("speed".into(), 200.0)]).unwrap();
        let mut fx = SpeechLeveler::new(&p, 48_000);
        let mut speech = tone(48_000 * 3, 0.05);
        fx.process(&mut speech);
        let boosted = fx.gain_db();
        assert!(boosted > 5.0, "{boosted}");
        // A long pause of faint room noise: once the level has dropped
        // under "Ignore below" (a few tens of ms) the gain stays put.
        let mut room = tone(4800, 0.0005);
        fx.process(&mut room);
        let held = fx.gain_db();
        assert!((held - boosted).abs() < 1.0, "{held} vs {boosted}");
        let mut room = tone(48_000 * 3, 0.0005);
        fx.process(&mut room);
        assert!((fx.gain_db() - held).abs() < 0.01);
    }

    #[test]
    fn the_leveler_never_goes_past_its_limits() {
        let mut faint = tone(48_000 * 8, 0.01);
        run(LEVELER, &[("max_gain", 6.0), ("speed", 200.0)], &mut faint);
        let n = faint.len();
        let g = peak(&faint[n - 4800..]) / 0.01;
        assert!((g - 2.0).abs() < 0.1, "6 dB is about x2: {g}");
    }

    #[test]
    fn the_gate_closes_between_phrases_and_opens_for_the_voice() {
        let mut b = tone(24_000, 0.5);
        b.extend(tone(48_000, 0.002));
        b.extend(tone(24_000, 0.5));
        run(GATE, &[("threshold", -30.0), ("range", -60.0)], &mut b);
        let speech = peak(&b[2 * 4000..2 * 20_000]);
        let gap = peak(&b[2 * 40_000..2 * 60_000]);
        let back = peak(&b[2 * 82_000..2 * 94_000]);
        assert!(speech > 0.45 && back > 0.45, "{speech} {back}");
        assert!(gap < 0.002 * 0.01, "closed to -60 dB: {gap}");
    }

    #[test]
    fn the_gate_follows_its_key() {
        let p = params_from(GATE, &[("threshold".into(), -30.0)]).unwrap();
        let mut fx = BreathGate::new(&p, 48_000);
        let mut quiet = tone(24_000, 0.01);
        let key = tone(24_000, 0.5);
        for (b, k) in quiet.chunks_mut(512).zip(key.chunks(512)) {
            fx.process_keyed(b, k);
        }
        assert!(peak(&quiet[24_000..]) > 0.009, "the key held it open");
    }
}
