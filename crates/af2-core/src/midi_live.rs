// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Live MIDI input: turning raw MIDI messages into note and controller
//! events (with the sustain pedal), and turning a recorded take into a
//! MIDI clip.

use crate::midi::{ControlEvent, MidiClip, MidiNote, Tempo, AFTERTOUCH, PITCH_BEND, SUSTAIN};

/// One event from a player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyEvent {
    /// A key went down: velocity 0..1.
    On {
        pitch: u8,
        velocity: f32,
        channel: u8,
    },
    Off {
        pitch: u8,
        channel: u8,
    },
    /// A controller (0..127, 128 aftertouch, 129 pitch bend), value 0..1.
    Control {
        controller: u8,
        value: f32,
        channel: u8,
    },
}

/// A key's velocity (0..1) through the velocity curve: `curve` from -1
/// (hard: it takes a firm touch to play loud) through 0 (as played) to 1
/// (soft: a light touch already plays loud).
pub fn velocity_curve(v: f32, curve: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if curve.abs() < 1e-3 {
        return v;
    }
    v.powf(2f32.powf(-1.5 * curve.clamp(-1.0, 1.0)))
}

/// One point of a velocity map: touch `x` (0..1) plays velocity `y`
/// (0..1); `tension` (-1 hard .. 1 soft, as `velocity_curve`) bends the
/// stretch from this point to the next.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VelPoint {
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub tension: f32,
}

/// What a keyboard's touch plays ("note on velocity mapping"): a curve
/// through points from touch 0 to touch 1, each stretch bent by its
/// tension. Keyboards with odd velocities get straightened out here.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VelocityMap {
    pub points: Vec<VelPoint>,
}

impl Default for VelocityMap {
    fn default() -> Self {
        VelocityMap::linear()
    }
}

/// The map's presets, as the editor names them.
pub const VELOCITY_PRESETS: [&str; 6] = ["Linear", "Soft", "Hard", "S-curve", "Narrow", "Fixed"];

impl VelocityMap {
    fn of(points: &[(f32, f32, f32)]) -> VelocityMap {
        VelocityMap {
            points: points
                .iter()
                .map(|&(x, y, tension)| VelPoint { x, y, tension })
                .collect(),
        }
    }

    /// As played.
    pub fn linear() -> VelocityMap {
        VelocityMap::of(&[(0.0, 0.0, 0.0), (1.0, 1.0, 0.0)])
    }

    /// One bent stretch, as the old single velocity curve (-1 .. 1).
    pub fn from_curve(curve: f32) -> VelocityMap {
        VelocityMap::of(&[(0.0, 0.0, curve.clamp(-1.0, 1.0)), (1.0, 1.0, 0.0)])
    }

    /// Every key at `velocity` (1..=127), whatever the touch.
    pub fn fixed(velocity: u8) -> VelocityMap {
        let y = velocity.clamp(1, 127) as f32 / 127.0;
        VelocityMap::of(&[(0.0, y, 0.0), (1.0, y, 0.0)])
    }

    /// A preset by name (VELOCITY_PRESETS); None for an unknown name.
    pub fn preset(name: &str) -> Option<VelocityMap> {
        Some(match name {
            "Linear" => VelocityMap::linear(),
            "Soft" => VelocityMap::from_curve(0.6),
            "Hard" => VelocityMap::from_curve(-0.6),
            // Quiet and loud ends harder to reach, the middle spread out.
            "S-curve" => VelocityMap::of(&[(0.0, 0.0, -0.6), (0.5, 0.5, 0.6), (1.0, 1.0, 0.0)]),
            // A keyboard that only sends about 25..100: that span plays
            // the whole range.
            "Narrow" => VelocityMap::of(&[
                (0.0, 0.0, 0.0),
                (0.2, 0.02, 0.0),
                (0.8, 1.0, 0.0),
                (1.0, 1.0, 0.0),
            ]),
            "Fixed" => VelocityMap::fixed(100),
            _ => return None,
        })
    }

    /// The preset this map is, if any.
    pub fn preset_name(&self) -> Option<&'static str> {
        if self.fixed_velocity().is_some() {
            return Some("Fixed");
        }
        VELOCITY_PRESETS
            .iter()
            .copied()
            .find(|n| VelocityMap::preset(n).as_ref() == Some(self))
    }

    /// The velocity (1..=127) every key plays when the map is flat.
    pub fn fixed_velocity(&self) -> Option<u8> {
        let y0 = self.points.first()?.y;
        self.points
            .iter()
            .all(|p| (p.y - y0).abs() < 1e-4)
            .then(|| (y0 * 127.0).round().clamp(1.0, 127.0) as u8)
    }

    /// Puts the points in order, inside 0..1, from touch 0 to touch 1
    /// (at least those two).
    pub fn tidy(&mut self) {
        for p in &mut self.points {
            p.x = if p.x.is_finite() { p.x.clamp(0.0, 1.0) } else { 0.5 };
            p.y = if p.y.is_finite() { p.y.clamp(0.0, 1.0) } else { 0.5 };
            p.tension = if p.tension.is_finite() { p.tension.clamp(-1.0, 1.0) } else { 0.0 };
        }
        self.points.sort_by(|a, b| a.x.total_cmp(&b.x));
        if self.points.len() < 2 {
            *self = VelocityMap::linear();
            return;
        }
        self.points[0].x = 0.0;
        let n = self.points.len();
        self.points[n - 1].x = 1.0;
    }

    /// What touch `v` (0..1) plays (0..1).
    pub fn apply(&self, v: f32) -> f32 {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let pts = &self.points;
        if pts.len() < 2 {
            return v;
        }
        let k = pts
            .windows(2)
            .position(|w| v <= w[1].x)
            .unwrap_or(pts.len() - 2);
        let (a, b) = (pts[k], pts[k + 1]);
        let span = b.x - a.x;
        let t = if span > 1e-6 { ((v - a.x) / span).clamp(0.0, 1.0) } else { 1.0 };
        (a.y + (b.y - a.y) * velocity_curve(t, a.tension)).clamp(0.0, 1.0)
    }

    /// `apply` in MIDI steps: touch 0..127 plays 1..127.
    pub fn apply_midi(&self, touch: u8) -> u8 {
        (self.apply(touch.min(127) as f32 / 127.0) * 127.0)
            .round()
            .clamp(1.0, 127.0) as u8
    }
}

/// A played event through the velocity map (only key-downs change; a
/// key-down never drops to silence).
pub fn curve_event(e: KeyEvent, map: &VelocityMap) -> KeyEvent {
    match e {
        KeyEvent::On { pitch, velocity, channel } => KeyEvent::On {
            pitch,
            velocity: map.apply(velocity).max(1.0 / 127.0),
            channel,
        },
        other => other,
    }
}

/// The keys a player holds, per input, and the sustain pedal.
///
/// With `pass_pedal` (the instrument has its own pedal) the pedal goes on
/// as controller 64 and notes end when their keys come up. Without it the
/// pedal is held here: keys let go while it is down keep sounding until
/// it comes up, as on a piano (a recording then gets longer notes).
#[derive(Debug, Default, Clone)]
pub struct Keys {
    pub pass_pedal: bool,
    pedal: [bool; 16],
    /// Keys down, (pitch, channel).
    down: Vec<(u8, u8)>,
    /// Let go while the pedal was down, still sounding.
    held: Vec<(u8, u8)>,
}

impl Keys {
    /// Feeds one MIDI message; events go to `out`.
    pub fn feed(&mut self, msg: &[u8], out: &mut Vec<KeyEvent>) {
        let Some(&status) = msg.first() else {
            return;
        };
        let channel = status & 0x0f;
        let d1 = msg.get(1).copied().unwrap_or(0) & 0x7f;
        let d2 = msg.get(2).copied().unwrap_or(0) & 0x7f;
        match status & 0xf0 {
            0x90 if d2 > 0 => {
                let key = (d1, channel);
                if self.down.contains(&key) || self.held.contains(&key) {
                    // Struck again: the old note ends first.
                    self.down.retain(|k| *k != key);
                    self.held.retain(|k| *k != key);
                    out.push(KeyEvent::Off { pitch: d1, channel });
                }
                self.down.push(key);
                out.push(KeyEvent::On {
                    pitch: d1,
                    velocity: d2 as f32 / 127.0,
                    channel,
                });
            }
            0x80 | 0x90 => {
                let key = (d1, channel);
                if !self.down.contains(&key) {
                    return;
                }
                self.down.retain(|k| *k != key);
                if self.pedal[channel as usize] && !self.pass_pedal {
                    self.held.push(key);
                } else {
                    out.push(KeyEvent::Off { pitch: d1, channel });
                }
            }
            0xb0 => match d1 {
                SUSTAIN if self.pass_pedal => out.push(KeyEvent::Control {
                    controller: SUSTAIN,
                    value: d2 as f32 / 127.0,
                    channel,
                }),
                SUSTAIN => {
                    let down = d2 >= 64;
                    self.pedal[channel as usize] = down;
                    if !down {
                        let (gone, keep): (Vec<_>, Vec<_>) =
                            self.held.iter().partition(|k| k.1 == channel);
                        self.held = keep;
                        for (pitch, ch) in gone {
                            out.push(KeyEvent::Off { pitch, channel: ch });
                        }
                    }
                }
                120 | 123 => self.release_channel(channel, out),
                c => out.push(KeyEvent::Control {
                    controller: c,
                    value: d2 as f32 / 127.0,
                    channel,
                }),
            },
            0xd0 => out.push(KeyEvent::Control {
                controller: AFTERTOUCH,
                value: d1 as f32 / 127.0,
                channel,
            }),
            0xe0 => out.push(KeyEvent::Control {
                controller: PITCH_BEND,
                value: (((d2 as u16) << 7) | d1 as u16) as f32 / 16383.0,
                channel,
            }),
            _ => {}
        }
    }

    fn release_channel(&mut self, channel: u8, out: &mut Vec<KeyEvent>) {
        for (pitch, ch) in self.down.iter().chain(&self.held) {
            if *ch == channel {
                out.push(KeyEvent::Off {
                    pitch: *pitch,
                    channel,
                });
            }
        }
        self.down.retain(|k| k.1 != channel);
        self.held.retain(|k| k.1 != channel);
    }

    /// Ends every note and lets every pedal up (an input switched).
    pub fn release_all(&mut self, out: &mut Vec<KeyEvent>) {
        for (pitch, channel) in self.down.drain(..).chain(self.held.drain(..)) {
            out.push(KeyEvent::Off { pitch, channel });
        }
        self.pedal = [false; 16];
    }
}

/// A recorded take: events at timeline seconds.
#[derive(Debug, Default, Clone)]
pub struct Take {
    pub events: Vec<(f64, KeyEvent)>,
}

/// The shortest note a take writes (beats).
pub const MIN_NOTE: f64 = 1.0 / 64.0;

impl Take {
    /// The take as a clip, or None when nothing was played. Notes still
    /// down end at `stop` (seconds). With a punch range only notes that
    /// start inside it are kept. The clip runs from the start of the bar
    /// the first note starts in to the end of the bar the last note ends
    /// in (a note ending on a bar line adds no bar). Controller moves
    /// inside the clip come along, and each controller's last value
    /// before the clip start is carried to it (a pedal already down).
    pub fn to_clip(
        &self,
        tempo: &Tempo,
        stop: f64,
        punch: Option<(f64, f64)>,
        id: u64,
        name: &str,
    ) -> Option<MidiClip> {
        // Notes in seconds.
        let mut open: Vec<(u8, u8, f64, f32)> = Vec::new();
        let mut notes: Vec<(f64, f64, u8, u8, f32)> = Vec::new();
        let mut controls: Vec<(f64, u8, f32, u8)> = Vec::new();
        for &(t, e) in &self.events {
            match e {
                KeyEvent::On {
                    pitch,
                    velocity,
                    channel,
                } => open.push((pitch, channel, t, velocity)),
                KeyEvent::Off { pitch, channel } => {
                    if let Some(i) = open.iter().position(|o| o.0 == pitch && o.1 == channel) {
                        let (p, ch, s, v) = open.remove(i);
                        notes.push((s, t, p, ch, v));
                    }
                }
                KeyEvent::Control {
                    controller,
                    value,
                    channel,
                } => controls.push((t, controller, value, channel)),
            }
        }
        for (p, ch, s, v) in open {
            notes.push((s, stop.max(s), p, ch, v));
        }
        if let Some((a, b)) = punch {
            notes.retain(|n| n.0 >= a && n.0 < b);
        }
        if notes.is_empty() {
            return None;
        }
        notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let first = tempo.beats(notes[0].0);
        let last = notes
            .iter()
            .map(|n| tempo.beats(n.1))
            .fold(f64::MIN, f64::max);
        let start = tempo.bar_floor(first);
        let (bar, into, _) = tempo.bar_at(last);
        let end = if into.abs() < 1e-9 {
            tempo.bar_start(bar)
        } else {
            tempo.bar_start(bar + 1)
        };
        let length = (end - start).max(MIN_NOTE);
        let clip_notes = notes
            .iter()
            .map(|&(s, e, pitch, channel, v)| {
                let b0 = tempo.beats(s) - start;
                MidiNote {
                    pitch,
                    start: b0.max(0.0),
                    length: (tempo.beats(e) - start - b0).max(MIN_NOTE),
                    velocity: (v * 127.0).round().clamp(1.0, 127.0) as u8,
                    channel,
                    pan: 0.0,
                }
            })
            .collect();
        let start_s = tempo.secs(start);
        let end_s = tempo.secs(end);
        let mut out_controls: Vec<ControlEvent> = Vec::new();
        // The last value of each controller before the clip, at its start.
        let mut before: Vec<(u8, u8, f32)> = Vec::new();
        for &(t, c, v, ch) in &controls {
            if t < start_s {
                match before.iter_mut().find(|b| b.0 == c && b.1 == ch) {
                    Some(b) => b.2 = v,
                    None => before.push((c, ch, v)),
                }
            }
        }
        for (controller, channel, value) in before {
            out_controls.push(ControlEvent {
                at: 0.0,
                controller,
                value,
                channel,
            });
        }
        for &(t, controller, value, channel) in &controls {
            if t >= start_s && t < end_s {
                out_controls.push(ControlEvent {
                    at: tempo.beats(t) - start,
                    controller,
                    value,
                    channel,
                });
            }
        }
        Some(MidiClip {
            id,
            name: name.into(),
            start,
            length,
            notes: clip_notes,
            timing: Default::default(),
            stretch: 1.0,
            pitch: 0.0,
            muted: false,
            gain_db: 0.0,
            controls: out_controls,
            ext_id: None,
        })
    }
}

/// How many frames the live MIDI voice keeps queued for a device buffer
/// of `buffer` frames (None: not fixed): two device buffers and a block.
pub fn live_queue_frames(buffer: Option<u32>, block: usize) -> usize {
    let b = buffer.map_or(1024, |b| b as usize);
    (2 * b + block).max(256)
}

#[cfg(test)]
mod curve_tests {
    use super::*;

    #[test]
    fn the_curve_bends_the_middle_and_keeps_the_ends() {
        for c in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            assert!((velocity_curve(1.0, c) - 1.0).abs() < 1e-6);
            assert!(velocity_curve(0.0, c).abs() < 1e-6);
        }
        assert!((velocity_curve(0.5, 0.0) - 0.5).abs() < 1e-6);
        assert!(velocity_curve(0.5, 1.0) > 0.7, "soft plays light touches louder");
        assert!(velocity_curve(0.5, -1.0) < 0.3, "hard needs a firmer touch");
        let e = curve_event(
            KeyEvent::On { pitch: 60, velocity: 0.001, channel: 0 },
            &VelocityMap::from_curve(-1.0),
        );
        assert!(matches!(e, KeyEvent::On { velocity, .. } if velocity >= 1.0 / 127.0));
    }

    #[test]
    fn fixed_velocity_ignores_the_touch_and_keeps_the_channel() {
        let fixed = VelocityMap::fixed(96);
        for v in [0.05, 0.5, 1.0] {
            match curve_event(KeyEvent::On { pitch: 60, velocity: v, channel: 9 }, &fixed) {
                KeyEvent::On { pitch, velocity, channel } => {
                    assert_eq!((pitch, channel), (60, 9));
                    assert!((velocity - 96.0 / 127.0).abs() < 1e-6);
                }
                _ => panic!("not a key-down"),
            }
        }
        assert_eq!(fixed.fixed_velocity(), Some(96));
        assert_eq!(fixed.preset_name(), Some("Fixed"));
        // Off events pass through untouched.
        let off = KeyEvent::Off { pitch: 60, channel: 3 };
        assert_eq!(curve_event(off, &fixed), off);
    }

    #[test]
    fn the_velocity_map_follows_its_points_and_tension() {
        let lin = VelocityMap::linear();
        for t in 0..=127u8 {
            assert_eq!(lin.apply_midi(t), t.max(1));
        }
        // Soft and hard sit either side of linear; the ends stay put.
        let soft = VelocityMap::preset("Soft").unwrap();
        let hard = VelocityMap::preset("Hard").unwrap();
        for k in 1..10 {
            let x = k as f32 / 10.0;
            assert!(soft.apply(x) > x && hard.apply(x) < x, "{x}");
        }
        assert_eq!((soft.apply(0.0), soft.apply(1.0)), (0.0, 1.0));
        // The old single curve maps to the same values.
        assert!((VelocityMap::from_curve(0.4).apply(0.3) - velocity_curve(0.3, 0.4)).abs() < 1e-6);
        // Points: a straight stretch interpolates, a bent one bends.
        let mut m = VelocityMap::of(&[(0.0, 0.2, 0.0), (0.5, 0.4, 0.0), (1.0, 1.0, 0.0)]);
        assert!((m.apply(0.25) - 0.3).abs() < 1e-6);
        assert!((m.apply(0.75) - 0.7).abs() < 1e-6);
        m.points[1].tension = 1.0;
        assert!(m.apply(0.75) > 0.7);
        // S-curve: below linear low, above it high.
        let s = VelocityMap::preset("S-curve").unwrap();
        assert!(s.apply(0.25) < 0.25 && s.apply(0.75) > 0.75);
        // Narrow: touch 25..100 covers nearly everything.
        let n = VelocityMap::preset("Narrow").unwrap();
        assert!(n.apply_midi(25) <= 3 && n.apply_midi(100) >= 120);
        // Every preset reads back as itself.
        for name in VELOCITY_PRESETS {
            assert_eq!(VelocityMap::preset(name).unwrap().preset_name(), Some(name));
        }
        // Tidy: sorted, ends at 0 and 1, junk fixed.
        let mut t = VelocityMap::of(&[(0.7, 2.0, 5.0), (0.1, 0.1, 0.0), (0.4, f32::NAN, 0.0)]);
        t.tidy();
        let xs: Vec<f32> = t.points.iter().map(|p| p.x).collect();
        assert_eq!(xs, vec![0.0, 0.4, 1.0]);
        assert_eq!((t.points[2].y, t.points[2].tension, t.points[1].y), (1.0, 1.0, 0.5));
        // Serde: a missing map is linear; a saved one loads back.
        let back: VelocityMap = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let empty: VelocityMap = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, VelocityMap::linear());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(k: &mut Keys, msgs: &[[u8; 3]]) -> Vec<KeyEvent> {
        let mut out = Vec::new();
        for m in msgs {
            k.feed(m, &mut out);
        }
        out
    }

    #[test]
    fn keys_start_and_end_notes() {
        let mut k = Keys::default();
        let ev = feed(
            &mut k,
            &[
                [0x90, 60, 127],
                [0x80, 60, 0],
                [0x90, 62, 64],
                [0x90, 62, 0],
            ],
        );
        assert_eq!(
            ev,
            vec![
                KeyEvent::On {
                    pitch: 60,
                    velocity: 1.0,
                    channel: 0
                },
                KeyEvent::Off {
                    pitch: 60,
                    channel: 0
                },
                KeyEvent::On {
                    pitch: 62,
                    velocity: 64.0 / 127.0,
                    channel: 0
                },
                KeyEvent::Off {
                    pitch: 62,
                    channel: 0
                },
            ]
        );
        // A key struck again ends its note first; a stray off is ignored.
        let ev = feed(&mut k, &[[0x91, 64, 90], [0x91, 64, 90], [0x81, 65, 0]]);
        assert_eq!(ev.len(), 3);
        assert!(matches!(
            ev[1],
            KeyEvent::Off {
                pitch: 64,
                channel: 1
            }
        ));
    }

    #[test]
    fn the_pedal_holds_notes_until_it_comes_up() {
        let mut k = Keys::default();
        let ev = feed(&mut k, &[[0xb0, 64, 127], [0x90, 60, 100], [0x80, 60, 0]]);
        assert_eq!(ev.len(), 1, "no off while the pedal is down: {ev:?}");
        let ev = feed(&mut k, &[[0xb0, 64, 0]]);
        assert_eq!(
            ev,
            vec![KeyEvent::Off {
                pitch: 60,
                channel: 0
            }]
        );
        // All Notes Off ends held notes too.
        let ev = feed(
            &mut k,
            &[
                [0xb0, 64, 127],
                [0x90, 61, 100],
                [0x80, 61, 0],
                [0xb0, 123, 0],
            ],
        );
        assert!(ev.contains(&KeyEvent::Off {
            pitch: 61,
            channel: 0
        }));
    }

    #[test]
    fn an_instrument_that_takes_the_pedal_gets_it_as_a_controller() {
        let mut k = Keys {
            pass_pedal: true,
            ..Default::default()
        };
        let ev = feed(&mut k, &[[0xb0, 64, 127], [0x90, 60, 100], [0x80, 60, 0]]);
        assert_eq!(ev.len(), 3);
        assert!(matches!(ev[0], KeyEvent::Control { controller: 64, .. }));
        assert!(matches!(ev[2], KeyEvent::Off { pitch: 60, .. }));
    }

    #[test]
    fn controllers_aftertouch_and_pitch_bend_pass_on() {
        let mut k = Keys::default();
        let ev = feed(&mut k, &[[0xb2, 1, 127], [0xd2, 64, 0], [0xe2, 0x7f, 0x7f]]);
        assert_eq!(
            ev,
            vec![
                KeyEvent::Control {
                    controller: 1,
                    value: 1.0,
                    channel: 2
                },
                KeyEvent::Control {
                    controller: AFTERTOUCH,
                    value: 64.0 / 127.0,
                    channel: 2
                },
                KeyEvent::Control {
                    controller: PITCH_BEND,
                    value: 1.0,
                    channel: 2
                },
            ]
        );
    }

    #[test]
    fn live_queue_covers_two_device_buffers() {
        assert_eq!(live_queue_frames(Some(512), 128), 1152);
        assert_eq!(live_queue_frames(Some(32), 128), 256);
        assert_eq!(live_queue_frames(None, 128), 2176);
    }

    #[test]
    fn takes_become_bar_aligned_clips_with_controls_carried() {
        // 120 BPM 4/4: a bar is 2 s.
        let tempo = Tempo::constant(120.0, 4, 4);
        let on = |pitch| KeyEvent::On {
            pitch,
            velocity: 0.5,
            channel: 0,
        };
        let off = |pitch| KeyEvent::Off { pitch, channel: 0 };
        let take = Take {
            events: vec![
                (
                    2.5,
                    KeyEvent::Control {
                        controller: 64,
                        value: 1.0,
                        channel: 0,
                    },
                ),
                (4.5, on(60)),
                (5.0, off(60)),
                (5.5, on(62)),
                (
                    6.5,
                    KeyEvent::Control {
                        controller: 1,
                        value: 0.25,
                        channel: 0,
                    },
                ),
            ],
        };
        let c = take.to_clip(&tempo, 8.0, None, 1, "Take").unwrap();
        // From the start of bar 3 (4 s, 8 beats) to the end of bar 4 (the
        // open note ends at 8 s, on a bar line: no extra bar).
        assert_eq!((c.start, c.length), (8.0, 8.0));
        assert_eq!(c.notes.len(), 2);
        assert_eq!((c.notes[0].start, c.notes[0].length), (1.0, 1.0));
        assert_eq!(c.notes[0].velocity, 64);
        assert_eq!(c.notes[1].length, 5.0);
        // The pedal pressed before the clip is carried to its start.
        assert_eq!(c.controls[0].at, 0.0);
        assert_eq!(c.controls[0].controller, 64);
        assert_eq!(c.controls[1].controller, 1);
        assert_eq!(c.controls[1].at, 5.0);
        // Punch keeps only notes that start inside it.
        let p = take
            .to_clip(&tempo, 8.0, Some((5.2, 9.0)), 1, "Take")
            .unwrap();
        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].pitch, 62);
        assert!(Take::default().to_clip(&tempo, 1.0, None, 1, "x").is_none());
    }

    /// AF1's channel colours: notes played on several MIDI channels keep
    /// them through the velocity curve, the recorded clip, edits in the
    /// roll and the playback plan (what the instrument is sent).
    #[test]
    fn notes_keep_their_channel_from_recording_through_editing_to_playback() {
        use crate::noteops as ops;
        let tempo = Tempo::constant(120.0, 4, 4);
        let mut k = Keys::default();
        let mut take = Take::default();
        let msgs: [(f64, [u8; 3]); 6] = [
            (0.0, [0x90, 60, 100]), // channel 1
            (0.1, [0x92, 64, 90]),  // channel 3
            (0.2, [0x9f, 67, 80]),  // channel 16
            (1.0, [0x80, 60, 0]),
            (1.0, [0x82, 64, 0]),
            (1.0, [0x8f, 67, 0]),
        ];
        for (t, m) in msgs {
            let mut out = Vec::new();
            k.feed(&m, &mut out);
            for e in out {
                take.events.push((t, curve_event(e, &VelocityMap::from_curve(0.5))));
            }
        }
        let mut c = take.to_clip(&tempo, 2.0, None, 1, "Take").unwrap();
        let ch = |c: &MidiClip, p: u8| c.notes.iter().find(|n| n.pitch == p).unwrap().channel;
        assert_eq!((ch(&c, 60), ch(&c, 64), ch(&c, 67)), (0, 2, 15));
        // Moving, copying and pasting keep each note's channel; recolouring
        // picked notes is setting their channel.
        let orig = c.notes.clone();
        let all: Vec<usize> = (0..c.notes.len()).collect();
        ops::move_from(&mut c.notes, &orig, &all, 0.5, 0);
        let copied = ops::copy(&c.notes, &all);
        let pasted = ops::paste(&mut c.notes, &copied, 2.0);
        assert_eq!(pasted.len(), 3);
        let i60 = c.notes.iter().position(|n| n.pitch == 60).unwrap();
        ops::set_channel(&mut c.notes, &[i60], 5);
        c.length = 8.0;
        let (plan, _) = crate::midi::plan_events(std::slice::from_ref(&c), &tempo);
        let mut sent: Vec<(u8, u8)> = plan.iter().map(|n| (n.pitch, n.channel)).collect();
        sent.sort();
        assert_eq!(sent, vec![(60, 0), (60, 5), (64, 2), (64, 2), (67, 15), (67, 15)]);
    }
}
