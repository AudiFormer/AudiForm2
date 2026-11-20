// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

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

use af27_core::midi_live::{KeyEvent, Keys, Take};
use af27_core::{TrackId, TrackKind};
use af27_engine::LiveNote;

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
        } = self;
        for r in routes.iter_mut() {
            if r.input.as_deref().is_some_and(|p| p != port) {
                continue;
            }
            events.clear();
            r.keys.feed(msg, &mut events);
            for e in &events {
                if *monitor == Some(r.track) {
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
                            KeyEvent::Control { .. } => None,
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
                Ok(c) => self.conns.push((name, c)),
                Err(e) => eprintln!("midi input {name}: {e}"),
            }
        }
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
        {
            let mut s = self.midi.lock();
            if fresh.is_some() {
                s.sender = fresh;
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
                    None => Keys::default(),
                };
                s.routes.push(Route {
                    track: *id,
                    input: input.clone(),
                    keys,
                });
            }
        }
        let wanted: Vec<Option<String>> = armed.iter().map(|a| a.2.clone()).collect();
        self.midi.open(&wanted);
        if !armed.is_empty() && self.midi.ports.is_empty() && !self.midi.error_shown {
            self.midi.error_shown = true;
            self.error(
                "No MIDI input was found. Plug in a MIDI keyboard to play armed MIDI tracks.",
            );
        }
        if self.midi.recording {
            let pos = self.position();
            self.midi.clock.push((Instant::now(), pos));
        }
    }

    /// Any armed MIDI track will record.
    pub fn midi_armed(&self) -> bool {
        self.session
            .tracks
            .iter()
            .any(|t| t.kind == TrackKind::Midi && t.armed)
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
        let before = self.session.clone();
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
            if let Some(t) = self.session.track_mut(id) {
                t.midi.push(clip);
                added += 1;
            }
        }
        if added > 0 {
            self.commit_session("Record MIDI", before);
        }
    }
}
