// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

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
}
