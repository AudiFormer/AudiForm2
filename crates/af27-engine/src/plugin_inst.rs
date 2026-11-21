// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! MIDI tracks that play a plug-in instrument (VST3), in playback and
//! exports ([`PlugTrack`]) and from a MIDI keyboard ([`KeysPlayer`]).
//!
//! Playback: the track's notes and controller moves become events placed
//! at their frame in each block. A jump (a start, a loop) catches up:
//! notes no longer sounding end, notes already sounding at the new spot
//! start there, and every controller gets its last value before it (so
//! a pedal already down is down). An instrument that does not take the
//! sustain pedal has it held here, by holding notes.
//!
//! The instrument's latency is part of the track's (the mix's delay
//! compensation lines it up, as for effects).

use af27_core::midi::{plan_events, PlannedControl, PlannedNote, Tempo, PITCH_BEND, SUSTAIN};
use af27_dsp::instrument::{Instrument, NoteEvent, Timed};
use af27_dsp::live::Chain;

/// Events one block can carry; more wait for the next block.
const MAX_EVENTS: usize = 2048;
/// Notes that can sound at once.
const MAX_SOUNDING: usize = 512;
const CONTROLLERS: usize = PITCH_BEND as usize + 1;

#[derive(Clone, Copy)]
struct Ev {
    t: f64,
    /// Off before controllers before On at one time (a repeated note
    /// ends before it starts again).
    order: u8,
    event: NoteEvent,
}

/// A MIDI track's plug-in instrument playing its clips.
pub struct PlugTrack {
    pub inst: Box<dyn Instrument>,
    notes: Vec<PlannedNote>,
    controls: Vec<PlannedControl>,
    ev: Vec<Ev>,
    next: usize,
    /// Where the next block should start (None: catch up first).
    next_t: Option<f64>,
    /// Notes on in the instrument, (pitch, channel).
    sounding: Vec<(u8, u8)>,
    /// Pedal held here: per channel, and the notes it keeps.
    pedal: [bool; 16],
    held: Vec<(u8, u8)>,
    /// The last value sent for each controller per channel (NaN: none).
    cc: Vec<f32>,
    events: Vec<Timed>,
}

fn order(e: &NoteEvent) -> u8 {
    match e {
        NoteEvent::Off { .. } => 0,
        NoteEvent::Control { .. } => 1,
        NoteEvent::On { .. } => 2,
    }
}

impl PlugTrack {
    /// `inst` playing `clips` through `tempo`.
    pub fn new(inst: Box<dyn Instrument>, clips: &[af27_core::midi::MidiClip], tempo: &Tempo) -> PlugTrack {
        let (notes, controls) = plan_events(clips, tempo);
        let mut ev: Vec<Ev> = Vec::with_capacity(notes.len() * 2 + controls.len());
        for n in &notes {
            let on = NoteEvent::On {
                pitch: n.pitch,
                velocity: n.velocity,
                channel: n.channel,
                tuning: n.detune * 100.0,
            };
            let off = NoteEvent::Off {
                pitch: n.pitch,
                channel: n.channel,
            };
            ev.push(Ev {
                t: n.start,
                order: 2,
                event: on,
            });
            ev.push(Ev {
                t: n.end,
                order: 0,
                event: off,
            });
        }
        for c in &controls {
            let e = NoteEvent::Control {
                controller: c.controller,
                value: c.value,
                channel: c.channel,
            };
            ev.push(Ev {
                t: c.at,
                order: order(&e),
                event: e,
            });
        }
        ev.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.order.cmp(&b.order)));
        PlugTrack {
            inst,
            notes,
            controls,
            ev,
            next: 0,
            next_t: None,
            sounding: Vec::with_capacity(MAX_SOUNDING),
            pedal: [false; 16],
            held: Vec::with_capacity(MAX_SOUNDING),
            cc: vec![f32::NAN; 16 * CONTROLLERS],
            events: Vec::with_capacity(MAX_EVENTS),
        }
    }

    pub fn latency(&self) -> usize {
        self.inst.latency()
    }

    /// Stops every note (a start or a jump): the instrument is cleared
    /// and the next block catches up from scratch.
    pub fn reset(&mut self) {
        self.inst.reset();
        self.sounding.clear();
        self.held.clear();
        self.pedal = [false; 16];
        self.cc.fill(f32::NAN);
        self.next_t = None;
    }

    /// Takes over a playing track's state (a rebuilt mix while playing):
    /// the next block catches up to the new notes without restarting the
    /// ones that still sound.
    pub fn adopt(&mut self, old: &mut PlugTrack) {
        std::mem::swap(&mut self.sounding, &mut old.sounding);
        std::mem::swap(&mut self.held, &mut old.held);
        std::mem::swap(&mut self.cc, &mut old.cc);
        self.pedal = old.pedal;
        self.next_t = None;
    }

    fn push(&mut self, offset: u32, event: NoteEvent) {
        if self.events.len() < self.events.capacity() {
            self.events.push(Timed { offset, event });
        }
    }

    fn pedal_here(&self) -> bool {
        !self.inst.takes(SUSTAIN)
    }

    /// Hands one event to the instrument, keeping the books.
    fn send(&mut self, offset: u32, e: NoteEvent) {
        match e {
            NoteEvent::On { pitch, channel, .. } => {
                if self.sounding.len() < MAX_SOUNDING {
                    self.sounding.push((pitch, channel));
                }
                self.held.retain(|k| *k != (pitch, channel));
                self.push(offset, e);
            }
            NoteEvent::Off { pitch, channel } => {
                let key = (pitch, channel);
                let Some(k) = self.sounding.iter().position(|x| *x == key) else {
                    return;
                };
                if self.pedal[channel as usize & 15] && self.pedal_here() {
                    if !self.held.contains(&key) {
                        self.held.push(key);
                    }
                    return;
                }
                self.sounding.swap_remove(k);
                self.push(offset, e);
            }
            NoteEvent::Control {
                controller,
                value,
                channel,
            } => {
                let i = (channel as usize & 15) * CONTROLLERS + (controller as usize).min(CONTROLLERS - 1);
                self.cc[i] = value;
                if controller == SUSTAIN && self.pedal_here() {
                    let down = value >= 0.5;
                    self.pedal[channel as usize & 15] = down;
                    if !down {
                        let mut k = 0;
                        while k < self.held.len() {
                            if self.held[k].1 == channel {
                                let (p, ch) = self.held.swap_remove(k);
                                if let Some(s) = self.sounding.iter().position(|x| *x == (p, ch)) {
                                    self.sounding.swap_remove(s);
                                }
                                self.push(offset, NoteEvent::Off { pitch: p, channel: ch });
                            } else {
                                k += 1;
                            }
                        }
                    }
                    return;
                }
                self.push(offset, e);
            }
        }
    }

    /// Catches up to `ts`: notes that should not sound end, notes that
    /// should start, controllers take their last value before it.
    fn catch_up(&mut self, ts: f64) {
        self.next = self.ev.partition_point(|e| e.t < ts);
        // End what should not sound now.
        let mut k = 0;
        while k < self.sounding.len() {
            let (p, ch) = self.sounding[k];
            let keep = self
                .notes
                .iter()
                .any(|n| n.pitch == p && n.channel == ch && n.start < ts && n.end > ts);
            if keep {
                k += 1;
            } else {
                self.sounding.swap_remove(k);
                self.held.retain(|x| *x != (p, ch));
                self.push(0, NoteEvent::Off { pitch: p, channel: ch });
            }
        }
        // Controllers first (a pedal down holds the notes that follow).
        let before = self.controls.partition_point(|c| c.at < ts);
        for j in 0..before {
            let c = self.controls[j];
            // Only the last move of each controller counts.
            if self.controls[j + 1..before]
                .iter()
                .any(|d| d.controller == c.controller && d.channel == c.channel)
            {
                continue;
            }
            let i = (c.channel as usize) * CONTROLLERS + c.controller as usize;
            if self.cc[i] != c.value {
                self.send(
                    0,
                    NoteEvent::Control {
                        controller: c.controller,
                        value: c.value,
                        channel: c.channel,
                    },
                );
            }
        }
        // Start notes already under way.
        for j in 0..self.notes.partition_point(|n| n.start < ts) {
            let n = self.notes[j];
            if n.end > ts && !self.sounding.contains(&(n.pitch, n.channel)) {
                self.send(
                    0,
                    NoteEvent::On {
                        pitch: n.pitch,
                        velocity: n.velocity,
                        channel: n.channel,
                        tuning: n.detune * 100.0,
                    },
                );
            }
        }
    }

    /// Renders one block starting at source time `ts` over `l` and `r`.
    /// No allocation.
    pub fn render(&mut self, ts: f64, dt: f64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        self.events.clear();
        if self.next_t.is_none_or(|e| (e - ts).abs() > dt * 0.5) {
            self.catch_up(ts);
        }
        let t1 = ts + dt * n as f64;
        self.next_t = Some(t1);
        while self.next < self.ev.len() && self.ev[self.next].t < t1 {
            if self.events.len() + 1 >= self.events.capacity() {
                break;
            }
            let e = self.ev[self.next];
            self.next += 1;
            let at = (((e.t - ts) / dt).floor().max(0.0) as usize).min(n.saturating_sub(1));
            self.send(at as u32, e.event);
        }
        // Sorted by frame (catching up put events at 0 first).
        self.events.sort_by_key(|e| e.offset);
        let PlugTrack { inst, events, .. } = self;
        inst.render(events, &mut l[..n], &mut r[..n]);
    }
}

/// A MIDI track's instrument played from a MIDI keyboard: a copy of its
/// own, then the track's chain, at the track's fader and pan. Heard
/// whether or not the transport runs.
pub struct KeysPlayer {
    inst: Box<dyn Instrument>,
    chain: Chain,
    /// Left and right gain (fader and pan).
    pub gain: [f32; 2],
    queued: Vec<Timed>,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl KeysPlayer {
    pub fn new(inst: Box<dyn Instrument>, chain: Chain, gain: [f32; 2]) -> KeysPlayer {
        KeysPlayer {
            inst,
            chain,
            gain,
            queued: Vec::with_capacity(MAX_EVENTS),
            l: vec![0.0; crate::BLOCK],
            r: vec![0.0; crate::BLOCK],
        }
    }

    /// An event for the start of the next block. No allocation.
    pub fn queue(&mut self, e: NoteEvent) {
        if self.queued.len() < self.queued.capacity() {
            self.queued.push(Timed { offset: 0, event: e });
        }
    }

    /// Every note stops.
    pub fn all_off(&mut self) {
        self.queued.clear();
        self.inst.reset();
    }

    /// Adds its sound to the first two channels of interleaved `out`
    /// (`width` wide). No allocation.
    pub fn render(&mut self, out: &mut [f32], width: usize) {
        let width = width.max(1);
        let frames = out.len() / width;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.l.len());
            let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
            self.inst.render(&self.queued, l, r);
            self.queued.clear();
            self.chain.process(l, r);
            for i in 0..n {
                let o = &mut out[(done + i) * width..(done + i + 1) * width];
                if width == 1 {
                    o[0] += (l[i] * self.gain[0] + r[i] * self.gain[1]) * 0.5;
                } else {
                    o[0] += l[i] * self.gain[0];
                    o[1] += r[i] * self.gain[1];
                }
            }
            done += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use af27_core::midi::{ControlEvent, MidiClip, MidiNote};
    use std::sync::{Arc, Mutex};

    /// Records what it is handed (block start frame, event).
    struct Spy {
        log: Arc<Mutex<Vec<(u64, NoteEvent)>>>,
        frame: u64,
        pedal: bool,
        resets: Arc<Mutex<usize>>,
    }

    impl Instrument for Spy {
        fn render(&mut self, events: &[Timed], l: &mut [f32], r: &mut [f32]) {
            for e in events {
                self.log.lock().unwrap().push((self.frame + e.offset as u64, e.event));
            }
            self.frame += l.len() as u64;
            l.fill(0.0);
            r.fill(0.0);
        }
        fn reset(&mut self) {
            *self.resets.lock().unwrap() += 1;
        }
        fn takes(&self, c: u8) -> bool {
            c != SUSTAIN || self.pedal
        }
    }

    fn clip(notes: &[(u8, f64, f64)], controls: &[(f64, u8, f32)]) -> MidiClip {
        let mut c = MidiClip::new(1, "c", 0.0, 8.0);
        for &(pitch, start, length) in notes {
            c.notes.push(MidiNote {
                pitch,
                start,
                length,
                velocity: 100,
                channel: 0,
                pan: 0.0,
            });
        }
        for &(at, controller, value) in controls {
            c.controls.push(ControlEvent {
                at,
                controller,
                value,
                channel: 0,
            });
        }
        c
    }

    type Log = Arc<Mutex<Vec<(u64, NoteEvent)>>>;

    fn player(c: MidiClip, pedal: bool) -> (PlugTrack, Log) {
        let log: Log = Arc::default();
        let spy = Spy {
            log: log.clone(),
            frame: 0,
            pedal,
            resets: Arc::default(),
        };
        // 120 BPM: a beat is half a second; 100 frames a second.
        (PlugTrack::new(Box::new(spy), &[c], &Tempo::default()), log)
    }

    fn run(p: &mut PlugTrack, from: f64, blocks: usize, n: usize) {
        let dt = 0.01;
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        for b in 0..blocks {
            p.render(from + dt * (b * n) as f64, dt, &mut l, &mut r);
        }
    }

    fn on(pitch: u8) -> impl Fn(&NoteEvent) -> bool {
        move |e| matches!(e, NoteEvent::On { pitch: p, .. } if *p == pitch)
    }

    fn off(pitch: u8) -> impl Fn(&NoteEvent) -> bool {
        move |e| matches!(e, NoteEvent::Off { pitch: p, .. } if *p == pitch)
    }

    #[test]
    fn notes_land_on_their_frames_and_a_repeat_ends_before_it_starts() {
        // C4 at beat 1 for a beat, then again at beat 2.
        let (mut p, log) = player(clip(&[(60, 1.0, 1.0), (60, 2.0, 1.0)], &[]), true);
        run(&mut p, 0.0, 20, 16);
        let log = log.lock().unwrap().clone();
        let at = |f: &dyn Fn(&NoteEvent) -> bool| -> Vec<u64> {
            log.iter().filter(|(_, e)| f(e)).map(|x| x.0).collect()
        };
        assert_eq!(at(&on(60)), vec![50, 100]);
        assert_eq!(at(&off(60)), vec![100, 150]);
        let k_off = log.iter().position(|(t, e)| *t == 100 && off(60)(e)).unwrap();
        let k_on = log.iter().position(|(t, e)| *t == 100 && on(60)(e)).unwrap();
        assert!(k_off < k_on, "{log:?}");
    }

    #[test]
    fn a_start_inside_a_note_plays_it_and_chases_the_pedal() {
        let (mut p, log) = player(
            clip(&[(64, 0.0, 4.0)], &[(0.5, SUSTAIN, 1.0), (1.0, 1, 0.25), (1.5, 1, 0.75)]),
            true,
        );
        // Start at 1 s (beat 2): the note is under way, the pedal down and
        // the mod wheel at its last value.
        run(&mut p, 1.0, 2, 16);
        let log = log.lock().unwrap().clone();
        assert!(log.iter().any(|(t, e)| *t == 0 && on(64)(e)), "{log:?}");
        assert!(log.iter().any(|(t, e)| *t == 0
            && matches!(e, NoteEvent::Control { controller: SUSTAIN, value, .. } if *value == 1.0)));
        let mods: Vec<f32> = log
            .iter()
            .filter_map(|(_, e)| match e {
                NoteEvent::Control { controller: 1, value, .. } => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(mods, vec![0.75], "only the last move");
    }

    #[test]
    fn a_pedal_the_instrument_does_not_take_holds_notes_here() {
        // The note ends at beat 1 with the pedal down from beat 0.5 until
        // beat 2.
        let (mut p, log) = player(
            clip(&[(60, 0.0, 1.0)], &[(0.5, SUSTAIN, 1.0), (2.0, SUSTAIN, 0.0)]),
            false,
        );
        run(&mut p, 0.0, 10, 16);
        let log = log.lock().unwrap().clone();
        assert!(!log.iter().any(|(_, e)| matches!(e, NoteEvent::Control { .. })));
        let offs: Vec<u64> = log.iter().filter(|(_, e)| off(60)(e)).map(|x| x.0).collect();
        assert_eq!(offs, vec![100], "held until the pedal comes up");
        // An instrument that takes it hears the pedal and the note ends.
        let (mut p, log) = player(
            clip(&[(60, 0.0, 1.0)], &[(0.5, SUSTAIN, 1.0), (2.0, SUSTAIN, 0.0)]),
            true,
        );
        run(&mut p, 0.0, 10, 16);
        let log = log.lock().unwrap().clone();
        let offs: Vec<u64> = log.iter().filter(|(_, e)| off(60)(e)).map(|x| x.0).collect();
        assert_eq!(offs, vec![50]);
        assert_eq!(
            log.iter().filter(|(_, e)| matches!(e, NoteEvent::Control { .. })).count(),
            2
        );
    }

    #[test]
    fn a_loop_back_ends_sounding_notes_and_a_rebuild_keeps_them() {
        let (mut p, log) = player(clip(&[(60, 0.0, 8.0), (62, 2.0, 1.0)], &[]), true);
        run(&mut p, 1.0, 4, 16);
        // Jump to 0.2 s: 60 still sounds (kept), nothing else.
        run(&mut p, 0.2, 1, 16);
        {
            let log = log.lock().unwrap();
            assert_eq!(log.iter().filter(|(_, e)| on(60)(e)).count(), 1, "{log:?}");
            assert!(!log.iter().any(|(_, e)| off(60)(e)));
        }
        // A rebuilt player adopting this one does not strike 60 again.
        let (mut q, log2) = player(clip(&[(60, 0.0, 8.0)], &[]), true);
        q.adopt(&mut p);
        run(&mut q, 0.36, 1, 16);
        assert!(log2.lock().unwrap().is_empty(), "{:?}", log2.lock().unwrap());
    }
}
