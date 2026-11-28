// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! MIDI input: armed MIDI tracks listen to one input or all of them, what
//! is played is heard through the first armed track's instrument (on the
//! MIDI thread, straight to the audio thread), and recording turns what
//! each track heard into a bar-aligned MIDI clip.
//!
//! Timing: each event is stamped when it arrives and placed on the
//! timeline through the playhead the audio device reports, sampled every
//! frame while recording, so a slow start of playback does not shift the
//! take.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use af2_core::midi_live::{curve_event, KeyEvent, Keys, Take};
use af2_core::{TrackId, TrackKind};
use af2_engine::LiveNote;

use crate::state::State;

const CLIENT: &str = "Audiform";
/// How often the list of inputs is read again (new devices).
const RESCAN: Duration = Duration::from_secs(2);

struct Route {
    track: TrackId,
    /// None: every input.
    input: Option<String>,
    keys: Keys,
}

#[derive(Default)]
struct Shared {
    routes: Vec<Route>,
    /// The track whose instrument plays what is heard.
    monitor: Option<TrackId>,
    sender: Option<rtrb::Producer<LiveNote>>,
    /// Recording: (arrival, track, event).
    capture: Option<Vec<(Instant, TrackId, KeyEvent)>>,
    /// VST3 instruments (crate::instruments): the monitored track plays
    /// one, through the engine's keys player and this sender.
    keys_on: bool,
    keys: Option<rtrb::Producer<af2_dsp::instrument::NoteEvent>>,
    /// Preferences > Multitrack > MIDI's note on velocity mapping.
    velocity: af2_core::midi_live::VelocityMap,
}

impl Shared {
    fn take_in(&mut self, port: &str, msg: &[u8]) {
        let now = Instant::now();
        let mut events = Vec::new();
        let Shared {
            routes,
            monitor,
            sender,
            capture,
            keys_on,
            keys,
            velocity,
        } = self;
        for r in routes.iter_mut() {
            if r.input.as_deref().is_some_and(|p| p != port) {
                continue;
            }
            events.clear();
            r.keys.feed(msg, &mut events);
            // What is heard and what is recorded both go through the
            // velocity curve.
            for e in events.iter_mut() {
                *e = curve_event(*e, velocity);
            }
            for e in &events {
                // A VST3 instrument plays it (crate::instruments).
                if *monitor == Some(r.track) && *keys_on {
                    if let Some(k) = keys.as_mut() {
                        let _ = k.push(crate::instruments::note_event(e));
                    }
                }
                if *monitor == Some(r.track) && !*keys_on {
                    if let Some(s) = sender.as_mut() {
                        let n = match *e {
                            KeyEvent::On {
                                pitch, velocity, ..
                            } => Some(LiveNote::On {
                                pitch,
                                velocity: (velocity * 127.0).round().clamp(1.0, 127.0) as u8,
                                pan: 0.0,
                            }),
                            KeyEvent::Off { pitch, .. } => Some(LiveNote::Off(pitch)),
                            KeyEvent::Control {
                                controller, value, ..
                            } => Some(LiveNote::Control { controller, value }),
                        };
                        if let Some(n) = n {
                            let _ = s.push(n);
                        }
                    }
                }
                if let Some(c) = capture.as_mut() {
                    c.push((now, r.track, *e));
                }
            }
        }
    }
}

/// The MIDI input's state in the app.
pub struct MidiIn {
    shared: Arc<Mutex<Shared>>,
    conns: Vec<(String, midir::MidiInputConnection<String>)>,
    ports: Vec<String>,
    scanned: Option<Instant>,
    /// While recording: (when, playhead) pairs from the device clock.
    clock: Vec<(Instant, f64)>,
    pub recording: bool,
    error_shown: bool,
    /// Inputs that would not open (said once each), and what to tell.
    failed: Vec<String>,
    notices: Vec<String>,
}

impl Default for MidiIn {
    fn default() -> Self {
        MidiIn {
            shared: Arc::new(Mutex::new(Shared::default())),
            conns: Vec::new(),
            ports: Vec::new(),
            scanned: None,
            clock: Vec::new(),
            recording: false,
            error_shown: false,
            failed: Vec::new(),
            notices: Vec::new(),
        }
    }
}

/// The MIDI inputs present now, by name.
pub fn inputs() -> Vec<String> {
    let Ok(m) = midir::MidiInput::new(CLIENT) else {
        return Vec::new();
    };
    m.ports()
        .iter()
        .filter_map(|p| m.port_name(p).ok())
        .collect()
}

impl MidiIn {
    /// The input names last seen.
    pub fn ports(&self) -> &[String] {
        &self.ports
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens exactly the inputs `wanted` names (None in it: every input).
    fn open(&mut self, wanted: &[Option<String>]) {
        let all = wanted.iter().any(Option::is_none);
        let want: Vec<String> = if all {
            self.ports.clone()
        } else {
            wanted.iter().flatten().cloned().collect()
        };
        self.conns.retain(|(name, _)| want.contains(name));
        for name in want {
            if self.conns.iter().any(|(n, _)| *n == name) || !self.ports.contains(&name) {
                continue;
            }
            let Ok(mut input) = midir::MidiInput::new(CLIENT) else {
                continue;
            };
            input.ignore(midir::Ignore::SysexAndTime);
            let Some(port) = input
                .ports()
                .into_iter()
                .find(|p| input.port_name(p).ok().as_deref() == Some(name.as_str()))
            else {
                continue;
            };
            let shared = self.shared.clone();
            match input.connect(
                &port,
                "Audiform input",
                move |_stamp, msg, port_name: &mut String| {
                    if let Ok(mut s) = shared.lock() {
                        s.take_in(port_name, msg);
                    }
                },
                name.clone(),
            ) {
                Ok(c) => {
                    self.failed.retain(|f| *f != name);
                    self.conns.push((name, c));
                }
                Err(e) => {
                    // Said once per input: often another program holds it
                    // (Windows lets only one program have a MIDI input).
                    if !self.failed.contains(&name) {
                        self.failed.push(name.clone());
                        self.notices.push(format!(
                            "The MIDI input \"{name}\" could not be opened ({e}). If another program is using it, close that program."
                        ));
                    }
                }
            }
        }
    }
}

/// A take's notes and controller moves put into `target` (both in beats,
/// shifted to its start); what starts before the clip is left out, and the
/// clip grows to the end of the bar of its last note.
fn merge_take(
    target: &mut af2_core::midi::MidiClip,
    take: af2_core::midi::MidiClip,
    tempo: &af2_core::midi::Tempo,
    replace: Option<(f64, f64)>,
) {
    // Replace: the notes starting in the span recorded over (clip beats)
    // go first, and the controller moves in it.
    if let Some((a, b)) = replace {
        target.notes.retain(|n| n.start < a || n.start >= b);
        target.controls.retain(|e| e.at < a || e.at >= b);
    }
    let off = take.start - target.start;
    let mut end = target.length;
    for mut n in take.notes {
        n.start += off;
        if n.start < 0.0 {
            continue;
        }
        end = end.max(n.start + n.length);
        target.notes.push(n);
    }
    for mut e in take.controls {
        e.at += off;
        if e.at >= 0.0 {
            target.controls.push(e);
        }
    }
    target.notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    target.controls.sort_by(|a, b| a.at.total_cmp(&b.at));
    if end > target.length + 1e-9 {
        let (bar, into, _) = tempo.bar_at(target.start + end);
        let bar = if into < 1e-6 { bar } else { bar + 1 };
        target.length = tempo.bar_start(bar) - target.start;
    }
}

impl State {
    /// Each frame: armed MIDI tracks get their inputs, the monitored
    /// track its instrument, and a recording its clock.
    pub fn midi_tick(&mut self) {
        let armed: Vec<(usize, TrackId, Option<String>)> = self
            .session
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TrackKind::Midi && t.armed)
            .filter(|(_, t)| t.midi_in.as_deref() != Some("none"))
            .map(|(i, t)| (i, t.id, t.midi_in.clone()))
            .collect();
        let rescan = match self.midi.scanned {
            Some(t) => t.elapsed() > RESCAN,
            None => true,
        };
        if armed.is_empty() && self.midi.conns.is_empty() && !rescan {
            return;
        }
        if rescan {
            self.midi.ports = inputs();
            self.midi.scanned = Some(Instant::now());
        }
        // The instrument that plays what is heard.
        let monitor = armed.first().filter(|_| self.prefs.midi_monitor).cloned();
        if let Some((ti, _, _)) = &monitor {
            self.audition_for(*ti);
        }
        let sender = {
            let s = self.midi.lock();
            match s.sender.as_ref() {
                Some(p) => p.is_abandoned(),
                None => true,
            }
        };
        let fresh = if sender {
            self.engine.as_mut().and_then(|e| e.take_note_sender())
        } else {
            None
        };
        // VST3 instruments (crate::instruments): the keys player and its
        // sender, and whether each armed track's instrument takes the
        // sustain pedal itself.
        let keys_on = match &monitor {
            Some((ti, _, _)) => crate::instruments::keys_for(self, *ti),
            None => {
                crate::instruments::keys_stop(self);
                false
            }
        };
        let keys_gone = self.midi.lock().keys.as_ref().is_none_or(|p| p.is_abandoned());
        let fresh_keys = if keys_gone {
            self.engine.as_mut().and_then(|e| e.take_keys_sender())
        } else {
            None
        };
        if fresh_keys.is_some() {
            crate::instruments::keys_forget();
        }
        let pedals: Vec<(TrackId, bool)> = armed
            .iter()
            .filter_map(|(_, id, _)| Some((*id, crate::instruments::pass_pedal(self, *id)?)))
            .collect();
        {
            let mut s = self.midi.lock();
            if fresh.is_some() {
                s.sender = fresh;
            }
            if fresh_keys.is_some() {
                s.keys = fresh_keys;
            }
            s.keys_on = keys_on;
            if s.velocity != self.prefs.midi_velocity_map {
                s.velocity = self.prefs.midi_velocity_map.clone();
            }
            let old_monitor = s.monitor;
            s.monitor = monitor.map(|m| m.1);
            if old_monitor != s.monitor {
                if let Some(p) = s.sender.as_mut() {
                    let _ = p.push(LiveNote::AllOff);
                }
            }
            // Keep each route's keys while its track and input stay.
            let mut old = std::mem::take(&mut s.routes);
            for (_, id, input) in &armed {
                let keys = match old.iter().position(|r| r.track == *id && r.input == *input) {
                    Some(k) => old.remove(k).keys,
                    // The pedals go on to the instrument (which holds
                    // notes while they are down), and a recording keeps
                    // them as controller moves with the keys' own lengths.
                    None => {
                        let mut k = Keys::default();
                        k.pass_pedal = true;
                        k
                    }
                };
                s.routes.push(Route {
                    track: *id,
                    input: input.clone(),
                    keys,
                });
                if let Some(&(_, pass)) = pedals.iter().find(|p| p.0 == *id) {
                    if let Some(r) = s.routes.last_mut() {
                        r.keys.pass_pedal = pass;
                    }
                }
            }
        }
        let wanted: Vec<Option<String>> = armed.iter().map(|a| a.2.clone()).collect();
        self.midi.open(&wanted);
        for n in std::mem::take(&mut self.midi.notices) {
            self.error(n);
        }
        // A track set to an input that is not there now.
        for (_, _, input) in &armed {
            if let Some(name) = input {
                if !self.midi.ports.contains(name) && !self.midi.failed.contains(name) {
                    self.midi.failed.push(name.clone());
                    self.error(format!(
                        "The MIDI input \"{name}\" set on an armed track is not connected. Pick another in the track's input menu."
                    ));
                }
            }
        }
        if !armed.is_empty() && self.midi.ports.is_empty() && !self.midi.error_shown {
            self.midi.error_shown = true;
            self.error(
                "No MIDI input was found. Plug in a MIDI keyboard to play armed MIDI tracks.",
            );
        }
        if self.midi.recording {
            let pos = self.position();
            self.midi.clock.push((Instant::now(), pos));
            self.replace_as_it_goes(pos);
        }
    }

    /// Recording into the Piano Roll's clip with Replace on: from the first
    /// key played, the clip's notes (and controller moves) are cleared as
    /// the playhead passes them, so what is replaced shows as it goes.
    fn replace_as_it_goes(&mut self, now: f64) {
        let Some(clip) = self.piano_rec_clip.filter(|_| self.piano_rec_replace) else {
            return;
        };
        if self.piano_rec_from.is_none() {
            // The first key down so far, placed by the clock.
            let first = self.midi.lock().capture.as_ref().and_then(|c| {
                c.iter()
                    .find(|e| e.1 == self.track_of_clip(clip).unwrap_or(e.1) && matches!(e.2, KeyEvent::On { .. }))
                    .map(|e| e.0)
            });
            if let Some(at) = first {
                let k = self.midi.clock.partition_point(|c| c.0 <= at);
                let (inst, pos) = if k > 0 { self.midi.clock[k - 1] } else { self.midi.clock[0] };
                let t = if at >= inst { pos + (at - inst).as_secs_f64() } else { pos - (inst - at).as_secs_f64() };
                self.piano_rec_from = Some(t.max(0.0));
            }
        }
        let Some(from) = self.piano_rec_from else { return };
        let tempo = self.session.tempo.clone();
        if let Some(c) = self.session.tracks.iter_mut().flat_map(|t| t.midi.iter_mut()).find(|m| m.id == clip) {
            // From a hair before the first key: a note starting right where
            // it was pressed goes too.
            let (a, b) = (tempo.beats(from) - c.start - 0.02, tempo.beats(now) - c.start);
            let n0 = c.notes.len() + c.controls.len();
            c.notes.retain(|n| n.start < a || n.start >= b);
            c.controls.retain(|e| e.at < a || e.at >= b);
            if c.notes.len() + c.controls.len() != n0 {
                self.session_version += 1;
                self.session_dirty = true;
            }
        }
    }

    fn track_of_clip(&self, clip: af2_core::ClipId) -> Option<TrackId> {
        self.session.tracks.iter().find(|t| t.midi.iter().any(|m| m.id == clip)).map(|t| t.id)
    }

    /// Any armed MIDI track will record.
    pub fn midi_armed(&self) -> bool {
        self.session
            .tracks
            .iter()
            .any(|t| t.kind == TrackKind::Midi && t.armed)
    }

    /// A note played on the computer keyboard (the Piano Roll), kept by a
    /// recording as if a MIDI keyboard on `track` played it.
    pub fn midi_capture(&mut self, track: TrackId, e: KeyEvent) {
        if let Some(c) = self.midi.lock().capture.as_mut() {
            c.push((Instant::now(), track, e));
        }
    }

    /// Records into the Piano Roll's clip: what the computer keyboard (and
    /// any MIDI keyboard on armed MIDI tracks) plays, from the playhead.
    pub fn piano_record_start(&mut self, clip: af2_core::ClipId) {
        if self.is_recording() {
            return;
        }
        self.mode = crate::state::Mode::Multitrack;
        self.piano_rec_clip = Some(clip);
        self.piano_rec_from = None;
        self.piano_rec_before = Some(self.session.clone());
        self.midi_record_start();
        self.play();
    }

    /// Starts capturing what armed MIDI tracks hear.
    pub fn midi_record_start(&mut self) {
        self.midi.clock.clear();
        self.midi.recording = true;
        self.midi.lock().capture = Some(Vec::new());
    }

    /// Stops capturing: each track's take becomes a clip on it (one undo
    /// step). Existing clips stay (MIDI records over them, overdub).
    pub fn midi_record_stop(&mut self, stop: f64) {
        if !self.midi.recording {
            return;
        }
        self.midi.recording = false;
        let into = self.piano_rec_clip.take();
        if into.is_some() {
            // Catch up the clearing to the end of the take.
            self.piano_rec_clip = into;
            self.replace_as_it_goes(stop);
            self.piano_rec_clip = None;
        }
        let from = self.piano_rec_from.take();
        let started_as = self.piano_rec_before.take();
        let events = self.midi.lock().capture.take().unwrap_or_default();
        let clock = std::mem::take(&mut self.midi.clock);
        let place = |at: Instant| -> f64 {
            // The latest clock sample at or before the event, else the first.
            let k = clock.partition_point(|c| c.0 <= at);
            let (inst, pos) = if k > 0 {
                clock[k - 1]
            } else {
                clock.first().copied().unwrap_or((at, stop))
            };
            if at >= inst {
                pos + (at - inst).as_secs_f64()
            } else {
                pos - (inst - at).as_secs_f64()
            }
        };
        let mut tracks: Vec<TrackId> = events.iter().map(|e| e.1).collect();
        tracks.dedup();
        tracks.sort_unstable();
        tracks.dedup();
        let before = started_as.clone().unwrap_or_else(|| self.session.clone());
        let mut added = 0;
        for id in tracks {
            let take = Take {
                events: events
                    .iter()
                    .filter(|e| e.1 == id)
                    .map(|e| (place(e.0).max(0.0), e.2))
                    .collect(),
            };
            let cid = self.session.alloc_id();
            let n = self
                .session
                .tracks
                .iter()
                .map(|t| t.midi.len())
                .sum::<usize>()
                + 1;
            let Some(clip) =
                take.to_clip(&self.session.tempo, stop, None, cid, &format!("Take {n}"))
            else {
                continue;
            };
            // Recorded from the Piano Roll: into its clip, on its track.
            let tempo = self.session.tempo.clone();
            // Replace clears from the first key played (already cleared as
            // the playhead went; the take's own notes go in after).
            let began = from.unwrap_or(stop);
            let replace = self.piano_rec_replace && from.is_some();
            if let Some(target) = into.and_then(|c| {
                self.session.track_mut(id).and_then(|t| t.midi.iter_mut().find(|m| m.id == c))
            }) {
                let span = replace.then(|| {
                    (tempo.beats(began) - target.start, tempo.beats(stop) - target.start)
                });
                merge_take(target, clip, &tempo, span);
                added += 1;
                continue;
            }
            if let Some(t) = self.session.track_mut(id) {
                t.midi.push(clip);
                added += 1;
            }
        }
        if added > 0 || (started_as.is_some() && before != self.session) {
            self.commit_session("Record MIDI", before);
        }
    }
}
