// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! MIDI tracks' VST3 instruments in the app: the VST3 part of the
//! instrument menu (by maker, with kinds and a search box), the
//! instrument's own window, what it reports going into the session, and
//! playing it from a MIDI keyboard through a copy of its own (and the
//! track's chain), whether or not the session plays.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use egui::RichText;

use af2_core::plugin::{is_plugin, PluginData};
use af2_core::{EffectSlot, TrackId, TrackKind};
use af2_dsp::instrument::{NoteEvent, Use};
use af2_vst3::live::{Status, KEYS};
use af2_vst3::manager;

use crate::state::State;
use crate::theme::{c, Theme};

/// More instruments than this get a search box.
const SEARCH_FROM: usize = 12;

/// What the keys player plays now.
struct Keys {
    track: TrackId,
    /// The instrument, chain and rate it was built from.
    sig: String,
    gain: [f32; 2],
    /// The settings last given to its copies (slot data).
    synced: String,
}

#[derive(Default)]
struct Ui {
    search: String,
    /// Instruments already said to be missing (track, slot copy).
    warned: HashSet<(TrackId, String)>,
    keys: Option<Keys>,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui::default());
}

// ------------------------------------------------------------ names

/// The track's instrument slot, if it plays a VST3 instrument.
pub fn slot_of(st: &State, ti: usize) -> Option<&EffectSlot> {
    st.session
        .tracks
        .get(ti)
        .filter(|t| t.kind == TrackKind::Midi)
        .and_then(|t| t.instrument_plugin.as_ref())
}

/// The track (index) whose instrument a running copy is (playback's or
/// the keys').
pub fn track_of(st: &State, inst: &str) -> Option<usize> {
    let key = inst.strip_suffix(KEYS).unwrap_or(inst);
    st.session.tracks.iter().position(|t| {
        t.instrument_plugin
            .as_ref()
            .is_some_and(|s| s.plugin_data().inst == key)
    })
}

/// The name shown for a track's VST3 instrument.
pub fn name(slot: &EffectSlot) -> String {
    crate::plugins::slot_name(slot)
}

fn rate_of(st: &State) -> u32 {
    st.engine.as_ref().map_or(st.session.rate, |e| e.rate)
}

// ------------------------------------------------------------ choosing

/// Plays track `ti` through VST3 instrument `effect` (`vst3:<id>`), in
/// place of the built-in synth or a SoundFont. One undo step.
pub fn choose(st: &mut State, ti: usize, effect: &str) {
    let before = st.session.clone();
    let Some(t) = st.session.tracks.get_mut(ti) else {
        return;
    };
    if t.instrument_plugin.as_ref().is_some_and(|s| s.effect == effect) {
        return;
    }
    t.instrument_plugin = Some(crate::plugins::new_slot(effect));
    t.soundfont = None;
    st.commit_session("Instrument", before);
}

/// Takes the VST3 instrument off track `ti` (the built-in synth or its
/// SoundFont plays again). Part of the caller's undo step.
pub fn clear(st: &mut State, ti: usize) {
    if let Some(t) = st.session.tracks.get_mut(ti) {
        t.instrument_plugin = None;
    }
}

/// (kind, name, effect id) of one instrument in the menu.
type Entry = (String, String, String);

/// The VST3 part of the instrument menu: Maker ▸ instruments (grouped by
/// kind when a maker has several), or a search over all of them.
/// Returns the effect id picked.
pub fn menu_items(ui: &mut egui::Ui, current: Option<&str>, th: &Theme) -> Option<String> {
    let mut picked = None;
    // (maker, [(kind, name, effect id)]), sorted.
    let list: Vec<(String, Vec<Entry>)> = {
        let reg = manager().registry();
        // By maker, ignoring case.
        let mut m: BTreeMap<String, (String, Vec<Entry>)> = BTreeMap::new();
        for f in reg.instruments() {
            // A kind that only repeats the maker ("Instrument|Waves") says
            // nothing.
            let maker = f.maker().to_lowercase();
            let kind = f
                .instrument_kind()
                .filter(|k| !maker.contains(&k.to_lowercase()))
                .unwrap_or_default();
            m.entry(maker.clone())
                .or_insert_with(|| (f.maker(), Vec::new()))
                .1
                .push((
                kind,
                f.info.name.clone(),
                format!("{}{}", af2_vst3::PREFIX, f.info.cid),
            ));
        }
        for (_, v) in m.values_mut() {
            v.sort_by_key(|(k, n, _)| (k.to_lowercase(), n.to_lowercase()));
        }
        m.into_values().collect()
    };
    let total: usize = list.iter().map(|(_, v)| v.len()).sum();
    ui.label(RichText::new("VST3 Instruments").small().color(c(th.text_dim)));
    if total == 0 {
        ui.label(RichText::new("None found yet.").color(c(th.text_dim)));
        if ui.button("Audio Plug-In Manager...").clicked() {
            crate::plugins::open_manager();
            ui.close_menu();
        }
        return None;
    }
    let mut item = |ui: &mut egui::Ui, kind: &str, name: &str, id: &str, maker: Option<&str>| {
        let mut text = name.to_string();
        if let Some(m) = maker {
            text = format!("{name}  ·  {m}");
        }
        let r = ui.selectable_label(current == Some(id), text);
        let r = if kind.is_empty() {
            r
        } else {
            r.on_hover_text(kind)
        };
        if r.clicked() {
            picked = Some(id.to_string());
            ui.close_menu();
        }
    };
    let mut search = UI.with(|u| u.borrow().search.clone());
    if total > SEARCH_FROM {
        ui.add(
            egui::TextEdit::singleline(&mut search)
                .hint_text("Search instruments")
                .desired_width(180.0),
        );
        UI.with(|u| u.borrow_mut().search = search.clone());
    }
    let needle = search.trim().to_lowercase();
    if !needle.is_empty() {
        let mut shown = 0;
        egui::ScrollArea::vertical()
            .id_salt("vsti-search")
            .max_height(320.0)
            .show(ui, |ui| {
                for (maker, items) in &list {
                    for (kind, name, id) in items {
                        let hay = format!("{name} {maker} {kind}").to_lowercase();
                        if hay.contains(&needle) {
                            shown += 1;
                            item(ui, kind, name, id, Some(maker));
                        }
                    }
                }
            });
        if shown == 0 {
            ui.label(RichText::new("No instrument matches.").color(c(th.text_dim)));
        }
        return picked;
    }
    for (maker, items) in &list {
        ui.menu_button(maker, |ui| {
            let kinds: HashSet<&str> = items.iter().map(|(k, _, _)| k.as_str()).collect();
            let headed = kinds.len() > 1;
            let mut last: Option<&str> = None;
            egui::ScrollArea::vertical()
                .id_salt(("vsti-maker", maker))
                .max_height(420.0)
                .show(ui, |ui| {
                    for (kind, name, id) in items {
                        if headed && last != Some(kind.as_str()) {
                            if last.is_some() {
                                ui.separator();
                            }
                            let k = if kind.is_empty() { "Other" } else { kind };
                            ui.label(RichText::new(k).small().color(c(th.text_dim)));
                            last = Some(kind.as_str());
                        }
                        item(ui, kind, name, id, None);
                    }
                });
        });
    }
    picked
}

// ------------------------------------------------------------ window

/// Opens track `ti`'s VST3 instrument in its own window (when it has one).
pub fn open_editor(st: &mut State, ti: usize) {
    let Some(slot) = slot_of(st, ti).cloned() else {
        return;
    };
    let Some(f) = crate::plugins::found(&slot.effect) else {
        st.toast(format!(
            "{} is not installed here (or not scanned yet). The track is silent; its settings are kept.",
            name(&slot)
        ));
        return;
    };
    if !f.info.has_editor {
        st.toast(format!("{} has no window of its own.", f.info.name));
        return;
    }
    let d = slot.plugin_data();
    let Some(inst) = manager()
        .get(&d.inst)
        .or_else(|| manager().live(&slot, rate_of(st)))
    else {
        return;
    };
    if let Status::Stopped(why) = inst.status() {
        st.toast(format!("The instrument stopped: {why} Pick Restart in its menu."));
        return;
    }
    let title = format!("{}: {}", st.session.tracks[ti].name, f.info.name);
    inst.open_editor(&title, crate::plugins::owner(), false);
}

/// Whether track `ti`'s instrument has stopped (crashed); the reason.
pub fn stopped(st: &State, ti: usize) -> Option<String> {
    let d = slot_of(st, ti)?.plugin_data();
    match manager().get(&d.inst)?.status() {
        Status::Stopped(why) => Some(why),
        _ => None,
    }
}

/// Starts a stopped instrument again (with the next build of the mix).
pub fn restart(st: &mut State, ti: usize) {
    if let Some(d) = slot_of(st, ti).map(|s| s.plugin_data()) {
        manager().restart(&d.inst);
        manager().restart(&format!("{}{KEYS}", d.inst));
        UI.with(|u| u.borrow_mut().keys = None);
        st.session_version += 1;
        st.rebuild_program();
    }
}

// ------------------------------------------------------------ session

/// Changes track `ti`'s instrument slot data (a control moved in its
/// window, fresh settings).
pub fn edit(st: &mut State, ti: usize, f: impl FnOnce(&mut PluginData)) {
    if let Some(s) = st
        .session
        .tracks
        .get_mut(ti)
        .and_then(|t| t.instrument_plugin.as_mut())
    {
        let mut d = s.plugin_data();
        f(&mut d);
        s.set_plugin_data(&d);
        st.session_dirty = true;
    }
}

/// After an edit or undo: running instrument copies follow the session,
/// and are kept (with their keys copies) from being stopped.
pub fn keep(st: &State, keep: &mut HashSet<String>) {
    for t in &st.session.tracks {
        let Some(s) = t.instrument_plugin.as_ref().filter(|s| is_plugin(&s.effect)) else {
            continue;
        };
        let d = s.plugin_data();
        if let Some(i) = manager().get(&d.inst) {
            i.sync(&d);
        }
        keep.insert(format!("{}{KEYS}", d.inst));
        keep.insert(d.inst);
    }
    for p in &st.sessions.parked {
        for t in &p.session.tracks {
            if let Some(s) = &t.instrument_plugin {
                keep.insert(s.plugin_data().inst);
            }
        }
    }
    // The keys player's effect copies.
    for t in &st.session.tracks {
        for s in t.chain.iter().filter(|s| is_plugin(&s.effect)) {
            keep.insert(format!("{}{KEYS}", s.plugin_data().inst));
        }
    }
}

/// Before saving: each running instrument's settings into its slot.
pub fn capture(st: &mut State) {
    for ti in 0..st.session.tracks.len() {
        let Some(d) = slot_of(st, ti).map(|s| s.plugin_data()) else {
            continue;
        };
        let Some(live) = manager().get(&d.inst).filter(|l| l.running()) else {
            continue;
        };
        if let Some(s) = live.state_now() {
            if d.state.as_deref() != Some(&s) {
                edit(st, ti, |d| d.state = Some(s));
            }
        }
    }
}

/// Every frame: says once when a track's instrument is not installed,
/// and keeps the keys player in step with its track.
pub fn tick(st: &mut State) {
    let mut missing = Vec::new();
    for t in &st.session.tracks {
        let Some(s) = t.instrument_plugin.as_ref().filter(|_| t.kind == TrackKind::Midi) else {
            continue;
        };
        if crate::plugins::found(&s.effect).is_none() {
            let key = (t.id, s.plugin_data().inst);
            if UI.with(|u| !u.borrow().warned.contains(&key)) {
                missing.push((key, t.name.clone(), name(s)));
            }
        }
    }
    // The plug-in list may still be loading at start: only after a scan
    // has run once is a plug-in missing.
    if !missing.is_empty() && !manager().registry().plugins.is_empty() {
        for (key, track, inst) in missing {
            UI.with(|u| u.borrow_mut().warned.insert(key));
            st.error(format!(
                "\"{track}\" plays {inst}, which is not installed here (or not scanned yet). The track is silent; its instrument and settings are kept."
            ));
        }
    }
    keys_follow(st);
}

// ------------------------------------------------------------ keys

/// A slot as the keys player's copy of it: plug-ins get a running copy
/// of their own (key `<slot's>#keys`), apart from playback's.
fn keys_copy(s: &EffectSlot) -> EffectSlot {
    let mut s = s.clone();
    if is_plugin(&s.effect) {
        let mut d = s.plugin_data();
        d.inst = format!("{}{KEYS}", d.inst);
        s.set_plugin_data(&d);
    }
    s
}

fn keys_sig(t: &af2_core::Track, rate: u32) -> String {
    let inst = t.instrument_plugin.as_ref().map(|s| (s.effect.clone(), s.plugin_data().inst));
    let chain: Vec<(&str, bool, Option<String>)> = t
        .chain
        .iter()
        .map(|s| (s.effect.as_str(), s.bypass, is_plugin(&s.effect).then(|| s.plugin_data().inst)))
        .collect();
    format!("{inst:?}|{chain:?}|{:?}|{rate}", t.rack)
}

/// The track's settings the keys copies should have: (instrument slot
/// data, each plug-in effect's data), as one text.
fn keys_settings(t: &af2_core::Track) -> String {
    let mut s = t
        .instrument_plugin
        .as_ref()
        .and_then(|s| s.data.clone())
        .unwrap_or_default();
    for e in t.chain.iter().filter(|e| is_plugin(&e.effect)) {
        s.push('|');
        s.push_str(e.data.as_deref().unwrap_or(""));
    }
    s
}

/// Makes the engine's keys player play track `ti`'s VST3 instrument (and
/// its chain), building it when the track or its effects changed. False
/// when the track does not play a VST3 instrument (the built-in synth or
/// a SoundFont plays it), or there is no audio output.
pub fn keys_for(st: &mut State, ti: usize) -> bool {
    let Some(t) = st.session.tracks.get(ti) else {
        return false;
    };
    let Some(slot) = t.instrument_plugin.clone().filter(|_| t.kind == TrackKind::Midi) else {
        keys_stop(st);
        return false;
    };
    let rate = rate_of(st);
    let sig = keys_sig(t, rate);
    let gain = {
        let (l, r) = af2_engine::track_gains(t, false);
        [l, r]
    };
    let id = t.id;
    let same = UI.with(|u| {
        u.borrow()
            .keys
            .as_ref()
            .is_some_and(|k| k.track == id && k.sig == sig)
    });
    if same {
        return st.engine.is_some();
    }
    let Some(e) = st.engine.as_mut() else {
        return false;
    };
    if let Some(eb) = e.real_buffer().or(e.buffer) {
        manager().set_keys_buffer(Some(eb));
    }
    let Some(inst) = af2_dsp::instrument::build_instrument(&slot, rate, Use::Keys) else {
        // Not installed: nothing plays (said by `tick`).
        e.set_keys_player(None);
        UI.with(|u| u.borrow_mut().keys = None);
        return true;
    };
    let chain_slots: Vec<EffectSlot> = t
        .chain
        .iter()
        // A keyed effect would need its key track playing: left out.
        .filter(|s| s.sidechain.is_none())
        .map(keys_copy)
        .collect();
    let mut chain = af2_dsp::live::build_chain(&chain_slots, rate, af2_engine::BLOCK);
    chain.set_rack(&t.rack);
    let synced = keys_settings(t);
    e.set_keys_player(Some(af2_engine::plugin_inst::KeysPlayer::new(
        inst, chain, gain,
    )));
    UI.with(|u| {
        u.borrow_mut().keys = Some(Keys {
            track: id,
            sig,
            gain,
            synced,
        })
    });
    true
}

/// No keys player (no armed track plays a VST3 instrument).
pub fn keys_stop(st: &mut State) {
    if UI.with(|u| u.borrow_mut().keys.take().is_some()) {
        if let Some(e) = st.engine.as_mut() {
            e.set_keys_player(None);
        }
    }
}

/// The engine was opened again: the keys player is built afresh.
pub fn keys_forget() {
    UI.with(|u| u.borrow_mut().keys = None);
}

/// The keys copies follow the track's fader, pan and settings.
fn keys_follow(st: &mut State) {
    let Some((id, gain0, synced0)) = UI.with(|u| {
        u.borrow()
            .keys
            .as_ref()
            .map(|k| (k.track, k.gain, k.synced.clone()))
    }) else {
        return;
    };
    let Some(t) = st.session.tracks.iter().find(|t| t.id == id) else {
        keys_stop(st);
        return;
    };
    let (l, r) = af2_engine::track_gains(t, false);
    if [l, r] != gain0 {
        if let Some(e) = st.engine.as_mut() {
            e.set_keys_gain([l, r]);
        }
        UI.with(|u| {
            if let Some(k) = u.borrow_mut().keys.as_mut() {
                k.gain = [l, r];
            }
        });
    }
    keys_mirror(t);
    let now = keys_settings(t);
    if now != synced0 {
        let mut all: Vec<&EffectSlot> = t.instrument_plugin.iter().collect();
        all.extend(t.chain.iter().filter(|s| is_plugin(&s.effect)));
        for s in all {
            let d = s.plugin_data();
            if let Some(i) = manager().get(&format!("{}{KEYS}", d.inst)) {
                i.sync(&d);
            }
        }
        UI.with(|u| {
            if let Some(k) = u.borrow_mut().keys.as_mut() {
                k.synced = now;
            }
        });
    }
}

thread_local! {
    static MIRRORED_AT: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// The keys copies take playback's copies' settings: once when they start,
/// then every couple of seconds while playback's window is open (a patch
/// or library picked there often never shows as a change). Without this
/// a sampler loaded in its window stays silent from the keyboard until
/// notes in a clip play it through playback's copy.
fn keys_mirror(t: &af2_core::Track) {
    let due = MIRRORED_AT.with(|m| m.get().is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(2)));
    let mut all: Vec<&EffectSlot> = t.instrument_plugin.iter().collect();
    all.extend(t.chain.iter().filter(|s| is_plugin(&s.effect)));
    for s in all {
        let d = s.plugin_data();
        let (Some(play), Some(keys)) = (
            manager().get(&d.inst),
            manager().get(&format!("{}{KEYS}", d.inst)),
        ) else {
            continue;
        };
        let first = !keys.mirrored.load(std::sync::atomic::Ordering::Relaxed);
        let open = play.editor_open.load(std::sync::atomic::Ordering::Relaxed);
        if first || (open && due) {
            keys.mirror(&play);
        }
    }
    if due {
        MIRRORED_AT.with(|m| m.set(Some(std::time::Instant::now())));
    }
}

/// Whether the keys should pass the sustain pedal on to track `id`'s
/// VST3 instrument (it takes controller 64), or hold notes themselves.
/// None for a track that does not play one.
pub fn pass_pedal(st: &State, id: TrackId) -> Option<bool> {
    let t = st.session.tracks.iter().find(|t| t.id == id)?;
    let d = t.instrument_plugin.as_ref()?.plugin_data();
    Some(
        manager()
            .get(&format!("{}{KEYS}", d.inst))
            .or_else(|| manager().get(&d.inst))
            .is_none_or(|i| i.takes(af2_core::midi::SUSTAIN)),
    )
}

/// A key played, as the instrument hears it.
pub fn note_event(e: &af2_core::midi_live::KeyEvent) -> NoteEvent {
    use af2_core::midi_live::KeyEvent;
    match *e {
        KeyEvent::On {
            pitch,
            velocity,
            channel,
        } => NoteEvent::On {
            pitch,
            velocity,
            channel,
            tuning: 0.0,
        },
        KeyEvent::Off { pitch, channel } => NoteEvent::Off { pitch, channel },
        KeyEvent::Control {
            controller,
            value,
            channel,
        } => NoteEvent::Control {
            controller,
            value,
            channel,
        },
    }
}

/// The piano roll plays a note on track `ti`: through its VST3 instrument
/// when it has one (true), else the caller's audition plays it.
pub fn audition_on(st: &mut State, ti: usize, pitch: u8, velocity: u8, channel: u8) -> bool {
    if slot_of(st, ti).is_none() || !keys_for(st, ti) {
        return false;
    }
    if let Some(e) = st.engine.as_mut() {
        e.keys_event(NoteEvent::On {
            pitch,
            velocity: velocity as f32 / 127.0,
            channel: channel & 15,
            tuning: 0.0,
        });
    }
    true
}

pub fn audition_off(st: &mut State, pitch: u8, channel: u8) {
    if UI.with(|u| u.borrow().keys.is_some()) {
        if let Some(e) = st.engine.as_mut() {
            e.keys_event(NoteEvent::Off { pitch, channel: channel & 15 });
        }
    }
}

pub fn audition_all_off(st: &mut State) {
    if UI.with(|u| u.borrow().keys.is_some()) {
        if let Some(e) = st.engine.as_mut() {
            e.keys_all_off();
        }
    }
}

// ------------------------------------------------------------ shots

thread_local! {
    static SHOT_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `AF2_OPEN=instrument-menu[:search]`, for screenshots: the Multitrack
/// with a MIDI track playing a VST3 instrument (Dexed when it is there,
/// else the first one), its instrument menu open (searching for `search`
/// when given).
pub fn shot(st: &mut State, arg: &str) {
    st.mode = crate::state::Mode::Multitrack;
    st.session.tracks.clear();
    let id = st.add_track_kind(TrackKind::Midi);
    let pick = {
        let reg = manager().registry();
        let list = reg.instruments();
        list.iter()
            .find(|f| f.info.name.eq_ignore_ascii_case("dexed"))
            .or(list.first())
            .map(|f| format!("{}{}", af2_vst3::PREFIX, f.info.cid))
    };
    let cid = st.session.alloc_id();
    if let Some(t) = st.session.track_mut(id) {
        t.instrument_plugin = pick.map(|e| crate::plugins::new_slot(&e));
        let mut c = af2_core::midi::MidiClip::new(cid, "Chords", 0.0, 8.0);
        for (k, p) in [60u8, 64, 67, 62, 65, 69].iter().enumerate() {
            c.notes.push(af2_core::midi::MidiNote {
                pitch: *p,
                start: (k / 3) as f64 * 4.0,
                length: 3.5,
                velocity: 100,
                channel: 0,
                pan: 0.0,
            });
        }
        t.midi.push(c);
        t.height = 150.0;
    }
    let search = arg.split_once(':').map_or("", |x| x.1).to_string();
    UI.with(|u| u.borrow_mut().search = search);
    SHOT_OPEN.with(|o| o.set(true));
    // Nothing to save when the shot's window closes.
    st.session_dirty = false;
}

/// Once, for a screenshot: the instrument menu should open.
pub fn take_shot_open() -> bool {
    SHOT_OPEN.with(|o| o.replace(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_copy_of_a_plug_in_slot_runs_apart() {
        let s = crate::plugins::new_slot("vst3:00112233445566778899AABBCCDDEEFF");
        let k = keys_copy(&s);
        assert_eq!(k.plugin_data().inst, format!("{}{KEYS}", s.plugin_data().inst));
        let plain = crate::rack::new_slot("compressor");
        assert_eq!(keys_copy(&plain), plain);
    }

    #[test]
    fn keys_hear_controllers_and_velocity_as_played() {
        use af2_core::midi_live::KeyEvent;
        let e = note_event(&KeyEvent::On {
            pitch: 60,
            velocity: 0.5,
            channel: 2,
        });
        assert_eq!(
            e,
            NoteEvent::On {
                pitch: 60,
                velocity: 0.5,
                channel: 2,
                tuning: 0.0
            }
        );
        let e = note_event(&KeyEvent::Control {
            controller: 64,
            value: 1.0,
            channel: 0,
        });
        assert!(matches!(e, NoteEvent::Control { controller: 64, .. }));
    }
}
