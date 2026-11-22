// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Clips in the Multitrack editor, several at a time, audio and MIDI alike:
//! the clip selection (Ctrl+click adds or takes away a clip), Copy, Cut and
//! Paste, Duplicate (Ctrl+D), and moving the selected clips together
//! (Alt-drag leaves the originals and moves copies). Clips moved or pasted
//! below the last track get new tracks; a clip that would land on a track
//! of the other kind (MIDI on an audio track) keeps to its own.
//!
//! The selection is `State::selected_clip` (the clip clicked last) and
//! `State::clip_sel` (all of them). `clip_sel` counts only while it holds
//! `selected_clip`, so anything that selects one clip the plain way
//! leaves just that clip selected.

use std::collections::HashMap;

use af2_core::midi::MidiClip;
use af2_core::{Clip, ClipId, Session, TrackKind};

use crate::state::State;

/// A copied clip.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Audio(Clip),
    Midi(MidiClip),
}

impl Item {
    /// The kind of track it goes on.
    fn kind(&self) -> TrackKind {
        match self {
            Item::Audio(_) => TrackKind::Audio,
            Item::Midi(_) => TrackKind::Midi,
        }
    }
}

/// Clips copied with Copy or Cut: each with its track's place below the
/// topmost copied track, and its start in seconds after the earliest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipBoard {
    pub items: Vec<(usize, f64, Item)>,
}

/// Where a clip is: its track, its start and end in seconds, and itself.
fn locate(s: &Session, id: ClipId) -> Option<(usize, f64, f64, Item)> {
    if let Some((ti, ci)) = s.find_clip(id) {
        let c = &s.tracks[ti].clips[ci];
        return Some((ti, c.start, c.end(), Item::Audio(c.clone())));
    }
    let (ti, ci) = s.find_midi(id)?;
    let c = &s.tracks[ti].midi[ci];
    Some((ti, s.tempo.secs(c.start), c.end_secs(&s.tempo), Item::Midi(c.clone())))
}

fn exists(s: &Session, id: ClipId) -> bool {
    s.find_clip(id).is_some() || s.find_midi(id).is_some()
}

// ------------------------------------------------------------ selection

/// The selected clips (audio and MIDI), the one clicked last first.
pub fn selected(st: &State) -> Vec<ClipId> {
    let Some(p) = st.selected_clip else {
        return Vec::new();
    };
    if !st.clip_sel.contains(&p) {
        return vec![p];
    }
    let mut v = vec![p];
    v.extend(
        st.clip_sel
            .iter()
            .copied()
            .filter(|&id| id != p && exists(&st.session, id)),
    );
    v
}

/// Selects `ids`, the first as the one clicked last.
pub fn select(st: &mut State, ids: Vec<ClipId>) {
    st.selected_clip = ids.first().copied();
    st.clip_sel = ids;
}

/// Ctrl+click: adds a clip to the selection, or takes it away.
pub fn toggle(st: &mut State, id: ClipId) {
    let mut sel = selected(st);
    if let Some(i) = sel.iter().position(|&x| x == id) {
        sel.remove(i);
    } else {
        sel.insert(0, id);
    }
    select(st, sel);
}

// ------------------------------------------------------------ placing

/// Copies of clips put into a session: each item on track `row` (rows
/// past the last track get new tracks) at `start` seconds. A row of the
/// other kind gets a new track too when `mismatch_new`, else the item
/// stays on track `home`. Returns the new clips' ids, in order.
fn place(
    s: &mut Session,
    items: Vec<(usize, usize, f64, Item)>,
    mismatch_new: bool,
    ids: Option<&[ClipId]>,
) -> Vec<ClipId> {
    let n = s.tracks.len();
    let fits = |ti: usize, kind: TrackKind, s: &Session| match kind {
        TrackKind::Midi => s.tracks[ti].kind == TrackKind::Midi,
        _ => s.tracks[ti].kind == TrackKind::Audio,
    };
    // Each item's track: one there is, or a new one by the row (and kind)
    // it stands for.
    enum To {
        Track(usize),
        New(usize, bool),
    }
    let to: Vec<To> = items
        .iter()
        .map(|(row, home, _, item)| {
            let kind = item.kind();
            if *row < n && fits(*row, kind, s) {
                To::Track(*row)
            } else if *row < n && !mismatch_new {
                To::Track((*home).min(n.saturating_sub(1)))
            } else {
                To::New(*row, kind == TrackKind::Midi)
            }
        })
        .collect();
    // New tracks in the order of their rows.
    let mut keys: Vec<(usize, bool)> = to
        .iter()
        .filter_map(|t| match t {
            To::New(r, m) => Some((*r, *m)),
            To::Track(_) => None,
        })
        .collect();
    keys.sort();
    keys.dedup();
    let mut made: HashMap<(usize, bool), usize> = HashMap::new();
    for (r, midi) in keys {
        s.add_track_of(if midi { TrackKind::Midi } else { TrackKind::Audio });
        made.insert((r, midi), s.tracks.len() - 1);
    }
    let tempo = s.tempo.clone();
    let mut out = Vec::new();
    for (k, ((_, _, start, item), to)) in items.into_iter().zip(to).enumerate() {
        let ti = match to {
            To::Track(ti) => ti,
            To::New(r, m) => made[&(r, m)],
        };
        let id = match ids {
            Some(ids) => ids[k],
            None => s.alloc_id(),
        };
        let tr = &mut s.tracks[ti];
        match item {
            Item::Audio(mut c) => {
                c.id = id;
                c.ext_id = None;
                c.start = start.max(0.0);
                tr.clips.push(c);
            }
            Item::Midi(mut c) => {
                c.id = id;
                c.ext_id = None;
                // MIDI clips sit on whole beats.
                c.start = tempo.beats(start.max(0.0)).round().max(0.0);
                tr.midi.push(c);
            }
        }
        out.push(id);
    }
    out
}

/// Takes clips out of a session.
fn remove(s: &mut Session, ids: &[ClipId]) {
    for t in &mut s.tracks {
        t.clips.retain(|c| !ids.contains(&c.id));
        t.midi.retain(|c| !ids.contains(&c.id));
    }
}

/// Track colours for tracks added from `from` on, from the theme.
fn color_new_tracks(st: &mut State, from: usize) {
    let cols = st.theme.track_colors.clone();
    if cols.is_empty() {
        return;
    }
    for (i, t) in st.session.tracks.iter_mut().enumerate().skip(from) {
        t.color = cols[i % cols.len()];
    }
}

// ------------------------------------------------------------ commands

/// Copies the selected clips. False when none is selected.
pub fn copy(st: &mut State) -> bool {
    let found: Vec<_> = selected(st)
        .into_iter()
        .filter_map(|id| locate(&st.session, id))
        .collect();
    if found.is_empty() {
        return false;
    }
    let top = found.iter().map(|f| f.0).min().unwrap_or(0);
    let first = found.iter().map(|f| f.1).fold(f64::MAX, f64::min);
    st.clip_board = Some(ClipBoard {
        items: found
            .into_iter()
            .map(|(ti, start, _, item)| (ti - top, start - first, item))
            .collect(),
    });
    true
}

/// Takes the selected clips out of the session, as one undo step.
pub fn delete_selected(st: &mut State, label: &str) -> bool {
    let sel = selected(st);
    if sel.is_empty() {
        return false;
    }
    let before = st.session.clone();
    remove(&mut st.session, &sel);
    if st.piano_clip.is_some_and(|p| sel.contains(&p)) {
        st.piano_clip = None;
    }
    select(st, Vec::new());
    if st.session == before {
        return false;
    }
    st.commit_session(label, before);
    true
}

pub fn cut(st: &mut State) {
    if copy(st) {
        let n = st.clip_board.as_ref().map_or(0, |b| b.items.len());
        delete_selected(st, if n > 1 { "Cut clips" } else { "Cut clip" });
    }
}

/// Pastes the copied clips at the playhead, the topmost on the selected
/// track (or where they were copied from).
pub fn paste(st: &mut State) {
    let Some(board) = st.clip_board.clone() else {
        st.toast("Copy some clips first");
        return;
    };
    if board.items.is_empty() {
        return;
    }
    let base = st
        .selected_track
        .and_then(|id| st.session.tracks.iter().position(|t| t.id == id))
        .unwrap_or(0);
    let at = st.session_cursor.max(0.0);
    let before = st.session.clone();
    let n = st.session.tracks.len();
    let items = board
        .items
        .into_iter()
        .map(|(row, rel, item)| (base + row, base, at + rel, item))
        .collect();
    let ids = place(&mut st.session, items, true, None);
    color_new_tracks(st, n);
    let label = if ids.len() > 1 { "Paste clips" } else { "Paste clip" };
    select(st, ids);
    st.commit_session(label, before);
}

/// Ctrl+D: copies of the selected clips right after them, on their own
/// tracks; the copies are then selected.
pub fn duplicate(st: &mut State) -> bool {
    let found: Vec<_> = selected(st)
        .into_iter()
        .filter_map(|id| locate(&st.session, id))
        .collect();
    if found.is_empty() {
        return false;
    }
    let first = found.iter().map(|f| f.1).fold(f64::MAX, f64::min);
    let last = found.iter().map(|f| f.2).fold(f64::MIN, f64::max);
    let span = (last - first).max(0.0);
    let before = st.session.clone();
    let items = found
        .into_iter()
        .map(|(ti, start, _, item)| (ti, ti, start + span, item))
        .collect();
    let ids = place(&mut st.session, items, false, None);
    let label = if ids.len() > 1 { "Duplicate clips" } else { "Duplicate clip" };
    select(st, ids);
    st.commit_session(label, before);
    true
}

// ------------------------------------------------------------ moving

/// Selected clips being dragged together (kept in the editor's drag).
#[derive(Clone, Debug, Default)]
pub struct GroupDrag {
    /// The clips, the one grabbed first.
    pub ids: Vec<ClipId>,
    /// Alt-drag: the copies' ids (the originals stay).
    pub copies: Option<Vec<ClipId>>,
    /// The grabbed clip's track and start (seconds) when the drag began.
    pub row0: usize,
    pub start0: f64,
}

impl GroupDrag {
    /// Starts dragging `ids` (the grabbed one first); `copy` for Alt.
    /// The copies' ids come from `before`, the session the drag began on.
    pub fn begin(before: &mut Session, ids: Vec<ClipId>, copy: bool) -> Option<GroupDrag> {
        let (row0, start0, _, _) = locate(before, *ids.first()?)?;
        let copies = copy.then(|| ids.iter().map(|_| before.alloc_id()).collect());
        Some(GroupDrag {
            ids,
            copies,
            row0,
            start0,
        })
    }
}

/// The session with the dragged clips moved `dt` seconds and `rows`
/// tracks (copies of them, for Alt), from the session the drag began on.
/// Returns the clips now selected.
pub fn moved(before: &Session, g: &GroupDrag, dt: f64, rows: isize) -> (Session, Vec<ClipId>) {
    let mut s = before.clone();
    if dt == 0.0 && rows == 0 {
        // Not moved (yet): nothing copied either.
        return (s, g.ids.clone());
    }
    // The earliest clip stops at zero; the rest keep their spacing.
    let found: Vec<_> = g.ids.iter().filter_map(|&id| locate(before, id)).collect();
    let earliest = found.iter().map(|f| f.1).fold(f64::MAX, f64::min);
    let dt = dt.max(-earliest);
    let top = found.iter().map(|f| f.0).min().unwrap_or(0) as isize;
    let rows = rows.max(-top);
    if g.copies.is_none() {
        remove(&mut s, &g.ids);
    }
    let items = found
        .into_iter()
        .map(|(ti, start, _, item)| ((ti as isize + rows) as usize, ti, start + dt, item))
        .collect();
    let ids = place(&mut s, items, false, Some(g.copies.as_deref().unwrap_or(&g.ids)));
    (s, ids)
}

/// One frame of a group drag: the grabbed clip would start at
/// `new_start` seconds, on track `row` (past the last track for new
/// ones).
pub fn drag_to(st: &mut State, before: &Session, g: &GroupDrag, new_start: f64, row: usize) {
    let n = before.tracks.len();
    let (s, ids) = moved(before, g, new_start - g.start0, row as isize - g.row0 as isize);
    st.session = s;
    color_new_tracks(st, n);
    st.session_version += 1;
    if let Some(&first) = ids.first() {
        if let Some((ti, _, _, _)) = locate(&st.session, first) {
            st.selected_track = Some(st.session.tracks[ti].id);
        }
    }
    select(st, ids);
}

#[cfg(test)]
mod tests {
    use super::*;
    use af2_core::midi::MidiNote;

    /// A state with an audio clip on track 0 (2..4 s), and a MIDI track
    /// (last) holding a MIDI clip at beat 4.
    fn setup() -> (State, ClipId, ClipId) {
        let mut st = State::new();
        st.mode = crate::state::Mode::Multitrack;
        let doc = st.add_doc(
            "a.wav".into(),
            None,
            af2_core::AudioBuffer::new(48_000, 1, 48_000 * 4),
        );
        let m = st.media_for_doc(doc).unwrap();
        let t0 = st.session.tracks[0].id;
        let a = st.session.add_clip(t0, m, 2.0).unwrap();
        let (ti, ci) = st.session.find_clip(a).unwrap();
        st.session.tracks[ti].clips[ci].length = 2.0;
        let t = st.add_track_kind(TrackKind::Midi);
        let mid = st.session.alloc_id();
        let mut clip = MidiClip::new(mid, "m", 4.0, 4.0);
        clip.notes.push(MidiNote {
            pitch: 60,
            start: 0.0,
            length: 1.0,
            velocity: 100,
            channel: 0,
            pan: 0.0,
        });
        st.session.track_mut(t).unwrap().midi.push(clip);
        (st, a, mid)
    }

    fn midi_count(s: &Session) -> usize {
        s.tracks.iter().map(|t| t.midi.len()).sum()
    }
    fn audio_count(s: &Session) -> usize {
        s.tracks.iter().map(|t| t.clips.len()).sum()
    }

    #[test]
    fn ctrl_click_builds_a_selection_and_a_plain_pick_ends_it() {
        let (mut st, a, mid) = setup();
        st.selected_clip = Some(a);
        toggle(&mut st, mid);
        assert_eq!(selected(&st), vec![mid, a]);
        toggle(&mut st, mid);
        assert_eq!(selected(&st), vec![a]);
        toggle(&mut st, mid);
        // Picking one of them keeps both (to drag them together)...
        st.selected_clip = Some(a);
        assert_eq!(selected(&st).len(), 2);
        // ...picking a clip outside the selection leaves just that one.
        st.selected_clip = Some(999);
        assert_eq!(selected(&st), vec![999]);
    }

    #[test]
    fn copy_and_paste_audio_and_midi_clips_at_the_playhead() {
        let (mut st, a, mid) = setup();
        select(&mut st, vec![a, mid]);
        assert!(copy(&mut st));
        let midi_track = st.session.tracks.len() - 1;
        st.selected_track = Some(st.session.tracks[0].id);
        st.session_cursor = 10.0;
        paste(&mut st);
        assert_eq!(audio_count(&st.session), 2);
        assert_eq!(midi_count(&st.session), 2);
        let new = selected(&st);
        assert_eq!(new.len(), 2);
        assert!(!new.contains(&a) && !new.contains(&mid));
        // The audio clip (earliest, 2 s) lands at the playhead on track 0;
        // the MIDI clip keeps its distance (beat 4 = 2 s at 120) and its
        // track: the MIDI track.
        let (ti, ci) = st.session.find_clip(new.iter().copied().find(|&i| st.session.find_clip(i).is_some()).unwrap()).unwrap();
        assert_eq!(ti, 0);
        assert!((st.session.tracks[ti].clips[ci].start - 10.0).abs() < 1e-9);
        let mnew = new.iter().copied().find(|&i| st.session.find_midi(i).is_some()).unwrap();
        let (mti, mci) = st.session.find_midi(mnew).unwrap();
        let tempo = st.session.tempo.clone();
        let expect = tempo.beats(10.0 + (tempo.secs(4.0) - 2.0)).round();
        assert_eq!(st.session.tracks[mti].midi[mci].start, expect);
        assert_eq!(mti, midi_track);
        assert_eq!(st.session.tracks[mti].midi[mci].notes.len(), 1);
        // One undo takes the paste away.
        st.undo();
        assert_eq!(audio_count(&st.session) + midi_count(&st.session), 2);
    }

    #[test]
    fn cut_takes_the_clips_away_and_paste_brings_them_back() {
        let (mut st, a, mid) = setup();
        select(&mut st, vec![mid, a]);
        cut(&mut st);
        assert_eq!(audio_count(&st.session) + midi_count(&st.session), 0);
        assert!(selected(&st).is_empty());
        st.session_cursor = 0.0;
        st.selected_track = Some(st.session.tracks[0].id);
        paste(&mut st);
        assert_eq!(audio_count(&st.session), 1);
        assert_eq!(midi_count(&st.session), 1);
    }

    #[test]
    fn pasting_a_midi_clip_on_an_audio_track_makes_a_midi_track() {
        let (mut st, _a, mid) = setup();
        select(&mut st, vec![mid]);
        copy(&mut st);
        st.selected_track = Some(st.session.tracks[0].id);
        let n = st.session.tracks.len();
        paste(&mut st);
        assert_eq!(st.session.tracks.len(), n + 1);
        assert_eq!(st.session.tracks[n].kind, TrackKind::Midi);
        assert_eq!(st.session.tracks[n].midi.len(), 1);
    }

    #[test]
    fn duplicate_puts_copies_right_after_the_selection() {
        let (mut st, a, mid) = setup();
        select(&mut st, vec![a]);
        assert!(duplicate(&mut st));
        assert_eq!(st.session.tracks[0].clips.len(), 2);
        let c = &st.session.tracks[0].clips[1];
        assert!((c.start - 4.0).abs() < 1e-9, "{}", c.start);
        assert_ne!(c.id, a);
        assert_eq!(selected(&st), vec![c.id]);
        // A MIDI clip on its own: right after itself.
        select(&mut st, vec![mid]);
        duplicate(&mut st);
        let ti = st.session.tracks.len() - 1;
        let m = &st.session.tracks[ti].midi;
        assert_eq!(m.len(), 2);
        let tempo = st.session.tempo.clone();
        assert_eq!(m[1].start, m[0].end_beats(&tempo).round());
    }

    #[test]
    fn dragging_several_clips_moves_them_together_and_below_makes_tracks() {
        let (mut st, a, mid) = setup();
        let mut before = st.session.clone();
        let n = before.tracks.len();
        let g = GroupDrag::begin(&mut before, vec![a, mid], false).unwrap();
        assert_eq!((g.row0, g.start0), (0, 2.0));
        // One second later, and the grabbed clip (track 0) dragged to just
        // below the last track: both go down n tracks, onto new tracks
        // (an audio one, then a MIDI one under it).
        drag_to(&mut st, &before, &g, 3.0, n);
        assert_eq!(st.session.tracks.len(), n + 2);
        assert_eq!(st.session.tracks[n].kind, TrackKind::Audio);
        assert_eq!(st.session.tracks[n + 1].kind, TrackKind::Midi);
        let (ati, aci) = st.session.find_clip(a).unwrap();
        assert_eq!(st.session.tracks[ati].kind, TrackKind::Audio);
        assert!(ati >= n);
        assert!((st.session.tracks[ati].clips[aci].start - 3.0).abs() < 1e-9);
        let (mti, _) = st.session.find_midi(mid).unwrap();
        assert!(mti >= n);
        assert_eq!(st.session.tracks[mti].kind, TrackKind::Midi);
        assert_eq!(selected(&st), vec![a, mid]);
        // Back where they began: no new tracks, nothing moved.
        drag_to(&mut st, &before, &g, 2.0, 0);
        assert_eq!(st.session, before);
    }

    #[test]
    fn a_midi_clip_moves_to_another_midi_track() {
        let (mut st, _a, mid) = setup();
        let t2 = st.add_track_kind(TrackKind::Midi);
        let row2 = st.session.tracks.iter().position(|t| t.id == t2).unwrap();
        let mut before = st.session.clone();
        let g = GroupDrag::begin(&mut before, vec![mid], false).unwrap();
        drag_to(&mut st, &before, &g, g.start0, row2);
        let (mti, _) = st.session.find_midi(mid).unwrap();
        assert_eq!(mti, row2);
    }

    #[test]
    fn alt_drag_leaves_the_originals_and_moves_copies() {
        let (mut st, a, mid) = setup();
        let mut before = st.session.clone();
        let g = GroupDrag::begin(&mut before, vec![a, mid], true).unwrap();
        drag_to(&mut st, &before, &g, 6.0, 0);
        assert_eq!(audio_count(&st.session), 2);
        assert_eq!(midi_count(&st.session), 2);
        // The originals where they were.
        let (ti, ci) = st.session.find_clip(a).unwrap();
        assert!((st.session.tracks[ti].clips[ci].start - 2.0).abs() < 1e-9);
        assert_eq!(st.session.tracks[st.session.find_midi(mid).unwrap().0].midi[0].start, 4.0);
        // The copies selected, 4 s on.
        let sel = selected(&st);
        assert_eq!(sel, g.copies.clone().unwrap());
        let (ti, ci) = st.session.find_clip(sel[0]).unwrap();
        assert!((st.session.tracks[ti].clips[ci].start - 6.0).abs() < 1e-9);
        // Let go without moving: no copies.
        drag_to(&mut st, &before, &g, 2.0, 0);
        assert_eq!(audio_count(&st.session), 1);
    }
}
