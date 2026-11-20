// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Standard MIDI Files (`.mid`), read and written with `midly`
//! (Unlicense).
//!
//! Reading: every SMF track's notes, split by channel (one Audiform27
//! track per track and channel that has notes), with velocity and pan
//! (controller 10, applied to the channel's later notes). Tempo and time
//! signature meta events become the session's tempo map. The map's
//! changes sit on bar lines, so an event partway through a bar moves to
//! the nearest bar line (counted, so the caller can say so). Notes are in
//! quarter notes either way and keep their exact places.
//!
//! Writing: format 1 at 960 ticks per quarter note. Track 0 holds the
//! tempo map (a tempo and a time signature at each stretch), then one
//! track per MIDI track with the notes of its unmuted clips as they play
//! (cut at the clip's end; through each clip's timing, stretch and whole
//! semitones of pitch), pan as controller 10 when it changes.

use std::collections::HashMap;

use af27_core::midi::{MidiClip, MidiNote, Tempo, TICKS_PER_BEAT};
use af27_core::{Session, TrackId, TrackKind};
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

/// One track's worth of notes from a file, in quarter notes from the start.
#[derive(Debug, Clone, PartialEq)]
pub struct SmfTrack {
    pub name: String,
    pub channel: u8,
    pub notes: Vec<MidiNote>,
}

#[derive(Debug, Clone)]
pub struct SmfImport {
    pub tracks: Vec<SmfTrack>,
    pub tempo: Tempo,
    /// The file had tempo or time signature events.
    pub has_tempo: bool,
    /// Events that were partway through a bar and moved to a bar line.
    pub moved: usize,
}

fn pan_of(v: u8) -> f32 {
    ((v as f32 - 64.0) / 63.0).clamp(-1.0, 1.0)
}

fn pan_value(p: f32) -> u8 {
    (64.0 + p.clamp(-1.0, 1.0) * 63.0).round().clamp(0.0, 127.0) as u8
}

/// A note being held: start tick, velocity, pan.
type Held = (u64, u8, f32);

enum MapEvent {
    Tempo(f64),
    Meter(u32, u32),
    /// A glide arriving at this tempo, from the event's tick given.
    Ramp(u64, f64),
}

/// Quarter notes between the tempo events a glide is written as.
const RAMP_STEP: f64 = 0.25;

/// Runs of at least this many evenly spaced tempo events (a beat or less
/// apart) moving one way are read as one glide.
const RAMP_MIN_EVENTS: usize = 4;

/// Folds dense runs of tempo events into glides: each run becomes one
/// `Ramp` at its last event, from its first. Events in tick order.
fn fold_ramps(events: Vec<(u64, usize, MapEvent)>, ppq: f64) -> Vec<(u64, usize, MapEvent)> {
    let bpm_of = |e: &MapEvent| match e {
        MapEvent::Tempo(b) => Some(*b),
        _ => None,
    };
    // (first, last) index of each run.
    let mut ramps: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let Some(b0) = bpm_of(&events[i].2) else {
            i += 1;
            continue;
        };
        // Extend over tempo events, evenly spaced, one direction.
        let mut end = i;
        let mut step = 0u64;
        let mut dir = 0.0f64;
        let mut prev = (events[i].0, b0);
        let mut j = i + 1;
        while j < events.len() {
            // A time signature inside (at the glide's last bar) is kept.
            let Some(b) = bpm_of(&events[j].2) else {
                j += 1;
                continue;
            };
            let gap = events[j].0 - prev.0;
            if gap == 0 || gap as f64 > ppq || (step != 0 && gap != step) {
                break;
            }
            let d = b - prev.1;
            if d * dir < 0.0 {
                break;
            }
            if d != 0.0 {
                dir = d.signum();
            }
            step = gap;
            prev = (events[j].0, b);
            end = j;
            j += 1;
        }
        if end + 1 - i >= RAMP_MIN_EVENTS && prev.1 != b0 {
            ramps.push((i, end));
            i = end + 1;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(events.len());
    let mut runs = ramps.into_iter().peekable();
    let mut from = 0u64;
    for (k, ev) in events.into_iter().enumerate() {
        match runs.peek() {
            Some(&(a, _)) if k == a => from = ev.0,
            _ => {}
        }
        match runs.peek() {
            Some(&(_, e)) if k == e => {
                let bpm = bpm_of(&ev.2).unwrap_or(120.0);
                out.push((ev.0, ev.1, MapEvent::Ramp(from, bpm)));
                runs.next();
            }
            Some(&(a, _)) if k >= a && bpm_of(&ev.2).is_some() => {}
            _ => out.push(ev),
        }
    }
    out
}

/// Reads a `.mid` file.
pub fn read(bytes: &[u8]) -> Result<SmfImport, String> {
    let smf = Smf::parse(bytes).map_err(|e| format!("Not a MIDI file: {e}"))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int().max(1) as f64,
        Timing::Timecode(..) => {
            return Err("MIDI files timed in SMPTE frames are not supported.".into())
        }
    };
    let mut map: Vec<(u64, usize, MapEvent)> = Vec::new();
    let mut tracks = Vec::new();
    let mut serial = 0usize;
    for (ti, events) in smf.tracks.iter().enumerate() {
        let mut tick = 0u64;
        let mut name = String::new();
        let mut pan = [0f32; 16];
        // Held notes per (channel, key): start tick, velocity, pan; first in, first out.
        let mut held: HashMap<(u8, u8), Vec<Held>> = HashMap::new();
        let mut by_channel: Vec<Vec<MidiNote>> = vec![Vec::new(); 16];
        let close = |by_channel: &mut Vec<Vec<MidiNote>>,
                     ch: u8,
                     key: u8,
                     start: u64,
                     vel: u8,
                     p: f32,
                     end: u64| {
            by_channel[ch as usize].push(MidiNote {
                pitch: key,
                start: start as f64 / ppq,
                length: (end.max(start + 1) - start) as f64 / ppq,
                velocity: vel.clamp(1, 127),
                channel: ch,
                pan: p,
            });
        };
        for ev in events {
            tick += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) if name.is_empty() => {
                    name = String::from_utf8_lossy(n).trim().to_string();
                }
                TrackEventKind::Meta(MetaMessage::Tempo(us)) => {
                    let bpm = 60_000_000.0 / us.as_int().max(1) as f64;
                    serial += 1;
                    map.push((
                        tick,
                        serial,
                        MapEvent::Tempo((bpm * 1000.0).round() / 1000.0),
                    ));
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(num, pow, _, _)) => {
                    serial += 1;
                    map.push((
                        tick,
                        serial,
                        MapEvent::Meter(num.max(1) as u32, 1u32 << pow.min(5)),
                    ));
                }
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            held.entry((ch, key.as_int())).or_default().push((
                                tick,
                                vel.as_int(),
                                pan[ch as usize],
                            ));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            if let Some(v) = held.get_mut(&(ch, key.as_int())) {
                                if !v.is_empty() {
                                    let (s, vel, p) = v.remove(0);
                                    close(&mut by_channel, ch, key.as_int(), s, vel, p, tick);
                                }
                            }
                        }
                        MidiMessage::Controller { controller, value }
                            if controller.as_int() == 10 =>
                        {
                            pan[ch as usize] = pan_of(value.as_int());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // Notes never let go of end with the track.
        let mut rest: Vec<_> = held.into_iter().collect();
        rest.sort_by_key(|((c, k), _)| (*c, *k));
        for ((ch, key), v) in rest {
            for (s, vel, p) in v {
                close(&mut by_channel, ch, key, s, vel, p, tick);
            }
        }
        let used: Vec<usize> = (0..16).filter(|&c| !by_channel[c].is_empty()).collect();
        for &c in &used {
            let mut notes = std::mem::take(&mut by_channel[c]);
            notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
            let base = if name.is_empty() {
                format!("Track {}", ti + 1)
            } else {
                name.clone()
            };
            let name = if used.len() > 1 {
                format!("{base} (Ch {})", c + 1)
            } else {
                base
            };
            tracks.push(SmfTrack {
                name,
                channel: c as u8,
                notes,
            });
        }
    }
    map.sort_by_key(|(t, s, _)| (*t, *s));
    let map = fold_ramps(map, ppq);
    let (tempo, moved) = tempo_from(&map, ppq);
    Ok(SmfImport {
        tracks,
        tempo,
        has_tempo: !map.is_empty(),
        moved,
    })
}

/// Builds a tempo map from tempo and meter events in tick order.
fn tempo_from(events: &[(u64, usize, MapEvent)], ppq: f64) -> (Tempo, usize) {
    let mut t = Tempo::default();
    let mut moved = 0;
    for (tick, _, e) in events {
        let beats = *tick as f64 / ppq;
        let (bar, into, _) = t.bar_at(beats);
        let on_line = into < 1e-6;
        if !on_line {
            moved += 1;
        }
        let bar = if on_line { bar } else { t.nearest_bar(beats) };
        if let (MapEvent::Ramp(from, bpm), true) = (e, bar > 0) {
            let fb = *from as f64 / ppq;
            let (from_bar, into, _) = t.bar_at(fb);
            let from_bar = if into < 1e-6 {
                from_bar
            } else {
                moved += 1;
                t.nearest_bar(fb)
            };
            t.add_ramp(from_bar, bar, Some(*bpm));
            continue;
        }
        let at = (bar > 0).then(|| t.add_change(bar));
        let seg = match at {
            Some(i) => {
                let c = &t.changes[i];
                (c.bpm, c.beats_per_bar, c.beat_unit)
            }
            None => (t.bpm, t.beats_per_bar, t.beat_unit),
        };
        let (bpm, top, unit) = match e {
            MapEvent::Tempo(b) | MapEvent::Ramp(_, b) => (*b, seg.1, seg.2),
            MapEvent::Meter(top, unit) => (seg.0, *top, *unit),
        };
        t.set_values(at, bpm, top, unit);
    }
    // Changes that change nothing go.
    let mut prev = (t.bpm, t.beats_per_bar, t.beat_unit);
    t.changes.retain(|c| {
        let v = (c.bpm, c.beats_per_bar, c.beat_unit);
        let keep = v != prev || !c.kind.is_immediate();
        prev = v;
        keep
    });
    (t, moved)
}

/// Puts imported tracks into `session` as new MIDI tracks, each with one
/// clip from the bar its first note is in to the bar after its last note
/// ends. With `replace_tempo`, the file's tempo map replaces the
/// session's (before the clips are placed, so bars come out right).
pub fn add_to_session(session: &mut Session, imp: &SmfImport, replace_tempo: bool) -> Vec<TrackId> {
    if replace_tempo {
        let metronome = session.tempo.metronome;
        session.tempo = imp.tempo.clone();
        session.tempo.metronome = metronome;
    }
    let mut ids = Vec::new();
    for tr in &imp.tracks {
        let id = session.add_track_of(TrackKind::Midi);
        let first = tr.notes.iter().map(|n| n.start).fold(f64::MAX, f64::min);
        let last = tr
            .notes
            .iter()
            .map(|n| n.start + n.length)
            .fold(0.0, f64::max);
        let tempo = &session.tempo;
        let start = tempo.bar_floor(first.max(0.0));
        let (end_bar, into, _) = tempo.bar_at(last);
        let end = if into < 1e-9 && last > start {
            tempo.bar_start(end_bar)
        } else {
            tempo.bar_start(end_bar + 1)
        };
        let clip_id = session.alloc_id();
        let t = session.track_mut(id).expect("just added");
        t.name = tr.name.clone();
        t.midi.push(MidiClip {
            id: clip_id,
            name: tr.name.clone(),
            start,
            length: (end - start).max(1.0),
            notes: tr
                .notes
                .iter()
                .map(|n| MidiNote {
                    start: n.start - start,
                    ..*n
                })
                .collect(),
            timing: Default::default(),
            stretch: 1.0,
            pitch: 0.0,
            muted: false,
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        });
        ids.push(id);
    }
    ids
}

fn tick(beats: f64) -> u64 {
    (beats * TICKS_PER_BEAT as f64).round().max(0.0) as u64
}

/// Turns (tick, order, event) into delta-timed events, ending the track.
fn finish<'a>(mut evs: Vec<(u64, u8, TrackEventKind<'a>)>) -> Vec<TrackEvent<'a>> {
    evs.sort_by_key(|(t, o, _)| (*t, *o));
    let mut out = Vec::with_capacity(evs.len() + 1);
    let mut last = 0u64;
    for (t, _, kind) in evs {
        out.push(TrackEvent {
            delta: u28::new((t - last).min(0x0FFF_FFFF) as u32),
            kind,
        });
        last = t;
    }
    out.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    out
}

/// Writes every MIDI track of `session` as a format 1 `.mid` file.
pub fn write(session: &Session) -> Result<Vec<u8>, String> {
    let names: Vec<Vec<u8>> = std::iter::once(session.name.clone().into_bytes())
        .chain(
            session
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Midi || !t.midi.is_empty())
                .map(|t| t.name.clone().into_bytes()),
        )
        .collect();
    let mut smf = Smf::new(Header::new(
        Format::Parallel,
        Timing::Metrical(u15::new(TICKS_PER_BEAT as u16)),
    ));
    // Track 0: the tempo map.
    let mut evs = vec![(
        0,
        0,
        TrackEventKind::Meta(MetaMessage::TrackName(&names[0])),
    )];
    let us = |bpm: f64| (60_000_000.0 / bpm).round().clamp(1.0, 16_777_215.0) as u32;
    for s in session.tempo.segments() {
        let t = tick(s.beat);
        if s.is_ramp() {
            // A glide as a run of tempo events, one every sixteenth, each
            // the steady tempo that lasts that step's exact time.
            let steps = (s.ramp_beats / RAMP_STEP).round().max(1.0) as u32;
            let h = s.ramp_beats / steps as f64;
            for k in 0..steps {
                let b = s.beat + k as f64 * h;
                let secs = s.secs_at(b + h) - s.secs_at(b);
                let bpm = 60.0 * h / secs;
                evs.push((
                    tick(b),
                    2,
                    TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us(bpm)))),
                ));
            }
            continue;
        }
        let pow = s.beat_unit.max(1).trailing_zeros() as u8;
        evs.push((
            t,
            1,
            TrackEventKind::Meta(MetaMessage::TimeSignature(
                s.beats_per_bar.min(255) as u8,
                pow,
                24,
                8,
            )),
        ));
        evs.push((
            t,
            2,
            TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us(s.bpm)))),
        ));
    }
    smf.tracks.push(finish(evs));

    let midi_tracks = session
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Midi || !t.midi.is_empty());
    for (k, tr) in midi_tracks.enumerate() {
        // The notes as they play: through each clip's timing and stretch,
        // transposed by its whole semitones of pitch shift (a fraction of
        // a semitone has no place in a note number and is left out).
        let tempo = &session.tempo;
        let mut notes: Vec<(u64, u64, MidiNote)> = Vec::new();
        for c in tr.midi.iter().filter(|c| !c.muted) {
            for n in c.notes.iter().filter(|n| n.start < c.length) {
                let s = tick(c.beats_at(n.start, tempo));
                let end = (n.start + n.length).min(c.length);
                let e = tick(c.beats_at(end, tempo)).max(s + 1);
                let pitch = c.shifted(n.pitch);
                notes.push((s, e, MidiNote { pitch, ..*n }));
            }
        }
        notes.sort_by_key(|(s, _, n)| (*s, n.channel, n.pitch));
        let mut evs = vec![(
            0,
            0,
            TrackEventKind::Meta(MetaMessage::TrackName(&names[k + 1])),
        )];
        let mut pan = [64u8; 16];
        for (s, e, n) in notes {
            let ch = u4::new(n.channel.min(15));
            let p = pan_value(n.pan);
            if pan[n.channel.min(15) as usize] != p {
                pan[n.channel.min(15) as usize] = p;
                evs.push((
                    s,
                    2,
                    TrackEventKind::Midi {
                        channel: ch,
                        message: MidiMessage::Controller {
                            controller: u7::new(10),
                            value: u7::new(p),
                        },
                    },
                ));
            }
            evs.push((
                s,
                3,
                TrackEventKind::Midi {
                    channel: ch,
                    message: MidiMessage::NoteOn {
                        key: u7::new(n.pitch.min(127)),
                        vel: u7::new(n.velocity.clamp(1, 127)),
                    },
                },
            ));
            evs.push((
                e,
                1,
                TrackEventKind::Midi {
                    channel: ch,
                    message: MidiMessage::NoteOff {
                        key: u7::new(n.pitch.min(127)),
                        vel: u7::new(64),
                    },
                },
            ));
        }
        smf.tracks.push(finish(evs));
    }
    let mut out = Vec::new();
    smf.write_std(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use af27_core::midi::TempoChange;

    fn note(pitch: u8, start: f64, length: f64, velocity: u8, channel: u8, pan: f32) -> MidiNote {
        MidiNote {
            pitch,
            start,
            length,
            velocity,
            channel,
            pan,
        }
    }

    fn session() -> Session {
        let mut s = Session::new("Song", 48_000, 0);
        s.tempo = Tempo::constant(96.0, 3, 4);
        s.tempo.changes.push(TempoChange::new(2, 140.0, 6, 8));
        s.tempo.changes.push(TempoChange::new(4, 90.0, 4, 4));
        let a = s.add_track_of(TrackKind::Midi);
        let b = s.add_track_of(TrackKind::Midi);
        let clip = |id, start, notes| MidiClip {
            id,
            name: "c".into(),
            start,
            length: 32.0,
            notes,
            timing: Default::default(),
            stretch: 1.0,
            pitch: 0.0,
            muted: false,
            gain_db: 0.0,
            controls: Vec::new(),
            ext_id: None,
        };
        let ta = s.track_mut(a).unwrap();
        ta.name = "Lead".into();
        ta.midi.push(clip(
            100,
            3.0,
            vec![
                note(60, 0.0, 1.0, 100, 0, 0.0),
                note(64, 0.5, 0.25, 90, 0, -1.0),
                note(67, 1.0 + 1.0 / 960.0, 2.5, 127, 0, 21.0 / 63.0),
                note(60, 4.0, 1.0, 1, 0, 1.0),
            ],
        ));
        let tb = s.track_mut(b).unwrap();
        tb.name = "Drums".into();
        tb.midi.push(clip(
            101,
            0.0,
            vec![
                note(36, 0.0, 0.5, 110, 9, 0.0),
                note(38, 1.0, 0.5, 80, 9, 0.0),
            ],
        ));
        s
    }

    /// Absolute notes per track.
    fn abs(s: &Session) -> Vec<(String, Vec<MidiNote>)> {
        s.tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Midi)
            .map(|t| {
                let mut v: Vec<MidiNote> = t
                    .midi
                    .iter()
                    .flat_map(|c| {
                        c.notes.iter().map(move |n| MidiNote {
                            start: c.start + n.start,
                            ..*n
                        })
                    })
                    .collect();
                v.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
                (t.name.clone(), v)
            })
            .collect()
    }

    fn same_notes(a: &[MidiNote], b: &[MidiNote]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert_eq!(
                (x.pitch, x.velocity, x.channel),
                (y.pitch, y.velocity, y.channel)
            );
            assert!((x.start - y.start).abs() < 1e-9, "{x:?} {y:?}");
            assert!((x.length - y.length).abs() < 1e-9, "{x:?} {y:?}");
            assert!((x.pan - y.pan).abs() < 1e-6, "{x:?} {y:?}");
        }
    }

    #[test]
    fn export_then_import_is_the_same() {
        let s = session();
        let bytes = write(&s).unwrap();
        let imp = read(&bytes).unwrap();
        assert!(imp.has_tempo);
        assert_eq!(imp.moved, 0);
        assert_eq!(imp.tempo, s.tempo);
        assert_eq!(imp.tracks.len(), 2);
        let mut back = Session::new("Song", 48_000, 0);
        add_to_session(&mut back, &imp, true);
        assert_eq!(back.tempo, s.tempo);
        let (want, got) = (abs(&s), abs(&back));
        for ((wn, w), (gn, g)) in want.iter().zip(&got) {
            assert_eq!(wn, gn);
            same_notes(w, g);
        }
        // Clips start on the bar of their first note.
        let lead = &back.tracks[0].midi[0];
        assert_eq!(lead.start, 3.0);
        assert!(lead.end() >= 3.0 + 5.0);
        // And it writes the same bytes again.
        assert_eq!(write(&back).unwrap()[..], bytes[..]);
    }

    #[test]
    fn glides_are_written_as_tempo_runs_and_read_back_as_glides() {
        use af27_core::midi::TempoKind;
        let mut s = session();
        s.tempo = Tempo::default();
        s.tempo.add_ramp(2, 6, Some(80.0));
        s.tempo.changes.push(TempoChange::new(8, 100.0, 3, 4));
        let i = s.tempo.add_ramp(9, 11, Some(150.0));
        s.tempo.changes[i].beats_per_bar = 5;
        s.tempo.normalize();
        let bytes = write(&s).unwrap();
        let imp = read(&bytes).unwrap();
        assert_eq!(imp.moved, 0);
        let (a, b) = (&imp.tempo, &s.tempo);
        assert_eq!(a.changes.len(), b.changes.len(), "{a:?}");
        for (x, y) in a.changes.iter().zip(&b.changes) {
            assert_eq!(
                (x.bar, x.kind, x.beats_per_bar),
                (y.bar, y.kind, y.beats_per_bar)
            );
            assert!((x.bpm - y.bpm).abs() < 1e-3);
        }
        assert_eq!(a.changes[0].kind, TempoKind::Ramp { from_bar: 2 });
        // The run keeps each step's time, so seconds agree closely.
        for k in 0..60 {
            let q = k as f64;
            assert!((a.secs(q) - b.secs(q)).abs() < 1e-3, "at {q}");
        }
        // A tempo every sixteenth across the glides.
        let smf = Smf::parse(&bytes).unwrap();
        let tempos = smf.tracks[0]
            .iter()
            .filter(|e| matches!(e.kind, TrackEventKind::Meta(MetaMessage::Tempo(_))))
            .count();
        assert_eq!(tempos, 1 + 64 + 1 + 1 + 24 + 1);
    }

    #[test]
    fn export_writes_the_notes_as_they_play() {
        use af27_core::midi::ClipTiming;
        let mut s = Session::new("Song", 48_000, 0);
        s.tempo.changes.push(TempoChange::new(1, 60.0, 4, 4));
        let a = s.add_track_of(TrackKind::Midi);
        let mut c = MidiClip::new(5, "c", 0.0, 8.0);
        c.notes.push(note(60, 1.0, 1.0, 100, 0, 0.0));
        c.notes.push(note(64, 6.0, 1.0, 100, 0, 0.0));
        c.stretch = 0.5;
        c.pitch = 3.4;
        s.track_mut(a).unwrap().midi.push(c.clone());
        // A kept-speed copy at 120 BPM on its own track.
        let b = s.add_track_of(TrackKind::Midi);
        let mut k = MidiClip::new(6, "k", 0.0, 8.0);
        k.notes.push(note(60, 6.0, 1.0, 100, 0, 0.0));
        k.timing = ClipTiming::KeepSpeed { bpm: 120.0 };
        s.track_mut(b).unwrap().midi.push(k);
        let imp = read(&write(&s).unwrap()).unwrap();
        let n = &imp.tracks[0].notes;
        // At 50% beat 1 is half a beat in, transposed up three.
        assert_eq!((n[0].pitch, n[0].start, n[0].length), (63, 0.5, 0.5));
        assert_eq!((n[1].pitch, n[1].start), (67, 3.0));
        // Kept at 120: 6 of its beats are 3 s, which is bar 1 (2 s at
        // 120) plus one beat at 60: session beat 5.
        let kn = &imp.tracks[1].notes;
        assert!((kn[0].start - 5.0).abs() < 1e-3, "{kn:?}");
        assert!((kn[0].length - 0.5).abs() < 1e-3, "{kn:?}");
    }

    #[test]
    fn events_inside_a_bar_move_to_a_bar_line() {
        // A tempo change at beat 5 of 4/4 (bar 2 starts at 4): nearest bar 1.
        let ev = vec![
            (0u64, 0usize, MapEvent::Tempo(100.0)),
            (5 * 480, 1, MapEvent::Tempo(150.0)),
            (8 * 480, 2, MapEvent::Meter(3, 4)),
            (8 * 480, 3, MapEvent::Tempo(150.0)),
        ];
        let (t, moved) = tempo_from(&ev, 480.0);
        assert_eq!(moved, 1);
        assert_eq!(t.bpm, 100.0);
        assert_eq!(t.changes.len(), 2);
        assert_eq!((t.changes[0].bar, t.changes[0].bpm), (1, 150.0));
        assert_eq!((t.changes[1].bar, t.changes[1].beats_per_bar), (2, 3));
    }
}
