// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Overlapping clips on one track, as the Multitrack edits them.
//!
//! A track's clips list is its stacking order: a later clip lies on top
//! and covers the clips under it (the engine plays them only under the
//! top clip's fades, see `af2_engine::mix::covers_of`). This module has
//! the pure rules the editor applies:
//!
//! - [`restack`]: a dragged clip lands on top of the clips it now
//!   overlaps, unless it was tucked under one of them before the drag.
//! - [`auto_crossfade`] / [`auto_crossfade_drag`]: a partial overlap
//!   made by a move or trim becomes a crossfade (the earlier clip fades
//!   out and the later fades in across exactly the overlap); a crossfade
//!   made that way goes once the overlap does.
//! - [`crossfade_partner`] / [`link_fade`]: the two halves of a
//!   crossfade bend together, the partner the opposite way.
//! - [`hidden_edges`]: the ends of lower clips that lie under a clip on
//!   top, which can still be grabbed low in the lane to trim them.
//! - [`source_range`]: the part of its file a clip plays.

use af2_core::{Clip, ClipId, Session};

/// A clip's span on the timeline (start, end) in seconds.
pub fn span(c: &Clip) -> (f64, f64) {
    (c.start, c.end())
}

/// How far clip `c`'s fades let it through at timeline time `t`: 0..1,
/// on the curve playback uses ([`af2_core::session::fade_shape`], skew
/// and bell included); gain and mute are not part of it.
pub fn fade_level(c: &Clip, t: f64) -> f64 {
    use af2_core::session::fade_shape;
    let local = (t - c.start).clamp(0.0, c.length);
    let mut g = 1.0;
    if c.fade_in > 0.0 && local < c.fade_in {
        g *= fade_shape(
            local / c.fade_in,
            c.props.fade_in_skew,
            c.props.fade_in_bell,
        );
    }
    let to_end = c.length - local;
    if c.fade_out > 0.0 && to_end < c.fade_out {
        g *= fade_shape(
            to_end / c.fade_out,
            c.props.fade_out_skew,
            c.props.fade_out_bell,
        );
    }
    g.clamp(0.0, 1.0)
}

/// The two spans share some time (touching ends do not count).
pub fn overlaps(a: (f64, f64), b: (f64, f64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// How long `a` and `b` overlap when neither lies wholly inside the other
/// (the overlap a crossfade covers), or None.
pub fn partial_overlap(a: (f64, f64), b: (f64, f64)) -> Option<f64> {
    if !overlaps(a, b) {
        return None;
    }
    let inside = (a.0 <= b.0 && a.1 >= b.1) || (b.0 <= a.0 && b.1 >= a.1);
    if inside {
        return None;
    }
    Some(a.1.min(b.1) - a.0.max(b.0))
}

/// The clips above `id` in `clips` that overlap it: the ones it is
/// tucked under.
pub fn tucked_under(clips: &[Clip], id: ClipId) -> Vec<ClipId> {
    let Some(i) = clips.iter().position(|c| c.id == id) else {
        return Vec::new();
    };
    let s = span(&clips[i]);
    clips[i + 1..]
        .iter()
        .filter(|c| overlaps(span(c), s))
        .map(|c| c.id)
        .collect()
}

/// Keeps in `tucked` only the clips that are still in `clips` (the
/// dragged clip's track now) and still overlap clip `id`: once the clip
/// is dragged off one, dragging it back on puts it over.
pub fn prune_tucked(clips: &[Clip], id: ClipId, tucked: &mut Vec<ClipId>) {
    let Some(m) = clips.iter().find(|c| c.id == id) else {
        tucked.clear();
        return;
    };
    let s = span(m);
    tucked.retain(|t| clips.iter().any(|c| c.id == *t && overlaps(span(c), s)));
}

/// Puts clip `id` on top of the clips it overlaps in `clips`, except the
/// ones in `tucked`, which stay above it. Clips it does not overlap keep
/// their places relative to it.
pub fn restack(clips: &mut Vec<Clip>, id: ClipId, tucked: &[ClipId]) {
    let Some(i) = clips.iter().position(|c| c.id == id) else {
        return;
    };
    let m = clips.remove(i);
    let s = span(&m);
    let over = |c: &Clip| overlaps(span(c), s);
    let at = match clips
        .iter()
        .rposition(|c| over(c) && !tucked.contains(&c.id))
    {
        Some(k) => i.max(k + 1),
        None => i,
    };
    clips.insert(at, m);
    // Clips it stays tucked under that now sit below it go just above it.
    let mut lifted = Vec::new();
    let mut k = 0;
    while k < at {
        if tucked.contains(&clips[k].id) && over(&clips[k]) {
            lifted.push(clips.remove(k));
        } else {
            k += 1;
        }
    }
    let at = clips.iter().position(|c| c.id == id).unwrap_or(0);
    for (n, c) in lifted.into_iter().enumerate() {
        clips.insert(at + 1 + n, c);
    }
}

/// Exactly the same fade length (a fade set to an overlap is the same
/// subtraction both times, so this is a tight check).
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs()))
}

fn set_fade_out(c: &mut Clip, len: f64) {
    c.fade_out = len.clamp(0.0, c.length);
    if c.fade_in + c.fade_out > c.length {
        c.fade_in = (c.length - c.fade_out).max(0.0);
    }
}

fn set_fade_in(c: &mut Clip, len: f64) {
    c.fade_in = len.clamp(0.0, c.length);
    if c.fade_in + c.fade_out > c.length {
        c.fade_out = (c.length - c.fade_in).max(0.0);
    }
}

/// Automatic crossfades between clip `m` and the clips `others` on one
/// track, after `m` moved or was trimmed. `prev` is where `m` was before
/// (None when it was not on this track); `here` says whether it is on
/// this track now.
///
/// - A partial overlap that is new (or whose order flipped) becomes a
///   crossfade: the earlier clip fades out and the later fades in across
///   exactly the overlap.
/// - A partial overlap that changed length takes its crossfade along,
///   fade by fade, where the fade was exactly the old overlap.
/// - Where the overlap is gone (clear, or one clip wholly inside the
///   other), fades exactly as long as the old overlap are removed; fades
///   of any other length were set by hand and stay.
pub fn auto_crossfade(m: &mut Clip, prev: Option<(f64, f64)>, here: bool, others: &mut [Clip]) {
    for o in others.iter_mut() {
        let os = span(o);
        // (overlap, m is the earlier clip)
        let before = prev.and_then(|p| partial_overlap(p, os).map(|l| (l, p.0 < os.0)));
        let now = if here {
            partial_overlap(span(m), os).map(|l| (l, m.start < os.0))
        } else {
            None
        };
        if let Some((lp, m_first)) = before {
            let (e, l): (&mut Clip, &mut Clip) = if m_first {
                (&mut *m, &mut *o)
            } else {
                (&mut *o, &mut *m)
            };
            if let Some((ln, _)) = now.filter(|n| n.1 == m_first) {
                if same(e.fade_out, lp) {
                    set_fade_out(e, ln);
                }
                if same(l.fade_in, lp) {
                    set_fade_in(l, ln);
                }
                continue;
            }
            if same(e.fade_out, lp) {
                e.fade_out = 0.0;
            }
            if same(l.fade_in, lp) {
                l.fade_in = 0.0;
            }
        }
        if let Some((ln, m_first)) = now {
            let (e, l): (&mut Clip, &mut Clip) = if m_first {
                (&mut *m, &mut *o)
            } else {
                (&mut *o, &mut *m)
            };
            set_fade_out(e, ln);
            set_fade_in(l, ln);
        }
    }
}

/// [`auto_crossfade`] for a gesture under way on clip `id`: every fade
/// on the tracks involved is first put back as it was in `before` (the
/// session when the gesture began), then the crossfades are worked out
/// from where the clip was then to where it is now. So a clip dragged
/// across others on its way leaves their fades as they were, and the
/// whole gesture stays one undo step.
pub fn auto_crossfade_drag(s: &mut Session, before: &Session, id: ClipId) {
    let (Some((ti, ci)), Some((bti, bci))) = (s.find_clip(id), before.find_clip(id)) else {
        return;
    };
    let old_track = before.tracks[bti].id;
    let oti = s.tracks.iter().position(|t| t.id == old_track);
    let mut involved = vec![ti];
    if let Some(o) = oti.filter(|&o| o != ti) {
        involved.push(o);
    }
    for &k in &involved {
        for c in s.tracks[k].clips.iter_mut() {
            if let Some((a, b)) = before.find_clip(c.id) {
                let bc = &before.tracks[a].clips[b];
                c.fade_in = bc.fade_in.min(c.length);
                c.fade_out = bc.fade_out.min(c.length - c.fade_in);
            }
        }
    }
    let bc = &before.tracks[bti].clips[bci];
    let prev = Some(span(bc));
    let mut m = s.tracks[ti].clips.remove(ci);
    match oti {
        Some(o) if o == ti => auto_crossfade(&mut m, prev, true, &mut s.tracks[ti].clips),
        Some(o) => {
            auto_crossfade(&mut m, prev, false, &mut s.tracks[o].clips);
            auto_crossfade(&mut m, None, true, &mut s.tracks[ti].clips);
        }
        None => auto_crossfade(&mut m, None, true, &mut s.tracks[ti].clips),
    }
    s.tracks[ti].clips.insert(ci, m);
}

/// The other half of a crossfade with clip `id`'s fade in (`fade_in`) or
/// fade out: the clip on the same track whose opposite fade covers the
/// same stretch of time.
pub fn crossfade_partner(clips: &[Clip], id: ClipId, fade_in: bool) -> Option<ClipId> {
    const TOL: f64 = 1e-6;
    let c = clips.iter().find(|c| c.id == id)?;
    let (a, b) = if fade_in {
        (c.start, c.start + c.fade_in)
    } else {
        (c.end() - c.fade_out, c.end())
    };
    if b - a <= TOL {
        return None;
    }
    clips
        .iter()
        .filter(|p| p.id != id)
        .find(|p| {
            let (pa, pb) = if fade_in {
                (p.end() - p.fade_out, p.end())
            } else {
                (p.start, p.start + p.fade_in)
            };
            pb - pa > TOL && (pa - a).abs() < TOL && (pb - b).abs() < TOL
        })
        .map(|p| p.id)
}

/// A fade's shape: its skew and whether it is a bell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadeLook {
    pub skew: f32,
    pub bell: bool,
}

impl FadeLook {
    pub fn of(c: &Clip, fade_in: bool) -> FadeLook {
        if fade_in {
            FadeLook {
                skew: c.props.fade_in_skew,
                bell: c.props.fade_in_bell,
            }
        } else {
            FadeLook {
                skew: c.props.fade_out_skew,
                bell: c.props.fade_out_bell,
            }
        }
    }

    pub fn put(self, c: &mut Clip, fade_in: bool) {
        if fade_in {
            c.props.fade_in_skew = self.skew;
            c.props.fade_in_bell = self.bell;
        } else {
            c.props.fade_out_skew = self.skew;
            c.props.fade_out_bell = self.bell;
        }
    }

    /// The partner's shape in a linked crossfade: the same kind, bent the
    /// opposite way (as one rises faster the other falls slower).
    pub fn mirrored(self) -> FadeLook {
        FadeLook {
            skew: if self.skew == 0.0 { 0.0 } else { -self.skew },
            bell: self.bell,
        }
    }
}

/// Sets clip `id`'s fade in (or out) to `look` and, with `linked`, its
/// crossfade partner's opposite fade to the mirrored look. Returns the
/// partner it set, if any.
pub fn link_fade(
    clips: &mut [Clip],
    id: ClipId,
    fade_in: bool,
    look: FadeLook,
    linked: bool,
) -> Option<ClipId> {
    let partner = if linked {
        crossfade_partner(clips, id, fade_in)
    } else {
        None
    };
    if let Some(c) = clips.iter_mut().find(|c| c.id == id) {
        look.put(c, fade_in);
    }
    if let Some(p) = partner.and_then(|p| clips.iter_mut().find(|c| c.id == p)) {
        look.mirrored().put(p, !fade_in);
    }
    partner
}

/// An end of a clip that lies under a clip above it on the same track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HiddenEdge {
    /// The lower clip whose edge it is.
    pub clip: ClipId,
    /// Its end (true) or its start.
    pub end: bool,
    /// Where the edge is, timeline seconds.
    pub t: f64,
    /// How far the covering clip reaches over the lower clip from the
    /// edge: the covering clip's start for a hidden end, its end for a
    /// hidden start.
    pub to: f64,
}

/// The hidden edges of a track's clips (list order is stacking order).
pub fn hidden_edges(clips: &[Clip]) -> Vec<HiddenEdge> {
    let mut out = Vec::new();
    for (i, c) in clips.iter().enumerate() {
        for (end, t) in [(false, c.start), (true, c.end())] {
            let cover = clips[i + 1..]
                .iter()
                .rev()
                .find(|u| u.start < t && t < u.end());
            if let Some(u) = cover {
                out.push(HiddenEdge {
                    clip: c.id,
                    end,
                    t,
                    to: if end { u.start } else { u.end() },
                });
            }
        }
    }
    out
}

/// The hidden edge nearest `x` within `tol` pixels (`x_of` maps time to
/// pixels).
pub fn hidden_edge_near(
    edges: &[HiddenEdge],
    x: f32,
    tol: f32,
    x_of: impl Fn(f64) -> f32,
) -> Option<HiddenEdge> {
    edges
        .iter()
        .map(|e| ((x_of(e.t) - x).abs(), *e))
        .filter(|(d, _)| *d <= tol)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e)
}

/// The part of its file clip `c` plays, in source seconds: from its
/// offset for its length at its stretch speed (a reversed clip plays the
/// same stretch backwards). `media_len` (0 or less: unknown) bounds it.
pub fn source_range(c: &Clip, media_len: f64) -> (f64, f64) {
    let a = c.offset.max(0.0);
    let mut b = a + c.source_len();
    if media_len > 0.0 {
        b = b.min(media_len);
    }
    (a.min(b), b)
}

/// `AF2_OPEN=overlap-demo`, for screenshots: a new session with a
/// crossfade between two clips (one half bent, linked), a clip wholly
/// over another, and a clip tucked under a longer one with its end
/// hidden.
pub fn demo(st: &mut crate::state::State) {
    use af2_core::AudioBuffer;
    fn tone(secs: f64, hz: f64, wobble: f64) -> AudioBuffer {
        let rate = 48_000;
        let n = (secs * rate as f64) as usize;
        let mut b = AudioBuffer::new(rate, 2, n);
        for ch in 0..2 {
            for (i, s) in b.channels[ch].iter_mut().enumerate() {
                let t = i as f64 / rate as f64;
                let env = 0.55 + 0.35 * (t * wobble + ch as f64).sin();
                *s = (env * (std::f64::consts::TAU * hz * t).sin()) as f32 * 0.8;
            }
        }
        b
    }
    st.new_session(None);
    st.mode = crate::state::Mode::Multitrack;
    let docs: Vec<_> = [(110.0, 1.3), (220.0, 2.1), (165.0, 0.7), (330.0, 3.0)]
        .iter()
        .enumerate()
        .map(|(k, (hz, w))| st.add_doc(format!("Demo {}.wav", k + 1), None, tone(12.0, *hz, *w)))
        .collect();
    let media: Vec<_> = docs.iter().filter_map(|d| st.media_for_doc(*d)).collect();
    if media.len() < 4 {
        return;
    }
    while st.session.tracks.len() < 3 {
        st.session.add_track();
    }
    st.session.tracks.truncate(3);
    for t in &mut st.session.tracks {
        t.height = 130.0;
    }
    let tid: Vec<_> = st.session.tracks.iter().map(|t| t.id).collect();
    let put = |st: &mut crate::state::State, ti: usize, m: usize, start: f64, len: f64| {
        let id = st.session.add_clip(tid[ti], media[m], start)?;
        let (a, b) = st.session.find_clip(id)?;
        st.session.tracks[a].clips[b].length = len;
        Some(id)
    };
    // Track 1: a partial overlap, crossfaded automatically, one half bent
    // (the other bends the opposite way).
    let a = put(st, 0, 0, 0.5, 6.0);
    let b = put(st, 0, 1, 4.5, 6.0);
    if let (Some(_), Some(b)) = (a, b) {
        let s = &mut st.session;
        let (ti, bi) = s.find_clip(b).unwrap();
        let mut m = s.tracks[ti].clips.remove(bi);
        auto_crossfade(&mut m, None, true, &mut s.tracks[ti].clips);
        s.tracks[ti].clips.insert(bi, m);
        link_fade(
            &mut s.tracks[ti].clips,
            b,
            true,
            FadeLook {
                skew: 45.0,
                bell: false,
            },
            true,
        );
    }
    // Track 2: a short clip wholly over a long one, with fades of its own.
    put(st, 1, 2, 0.5, 10.0);
    if let Some(d) = put(st, 1, 3, 3.5, 3.0) {
        let (ti, ci) = st.session.find_clip(d).unwrap();
        let c = &mut st.session.tracks[ti].clips[ci];
        c.fade_in = 0.6;
        c.fade_out = 0.6;
        c.props.color = Some([230, 170, 70]);
    }
    // Track 3: a clip tucked under a longer one: its end is hidden.
    if let Some(e) = put(st, 2, 0, 1.0, 5.0) {
        let (ti, ci) = st.session.find_clip(e).unwrap();
        st.session.tracks[ti].clips[ci].props.color = Some([120, 200, 140]);
    }
    put(st, 2, 1, 4.0, 6.5);
    st.session_view = crate::state::View {
        start: 0.0,
        len: 11.5,
    };
    st.session_version += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: ClipId, start: f64, length: f64) -> Clip {
        Clip {
            id,
            media: 1,
            name: format!("c{id}"),
            start,
            offset: 0.0,
            length,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
            props: Default::default(),
            ext_id: None,
        }
    }

    fn ids(clips: &[Clip]) -> Vec<ClipId> {
        clips.iter().map(|c| c.id).collect()
    }

    #[test]
    fn the_drawn_fade_is_the_played_fade() {
        let mut c = clip(1, 2.0, 4.0);
        c.fade_in = 1.0;
        c.fade_out = 1.5;
        c.props.fade_in_skew = 60.0;
        c.props.fade_out_bell = true;
        c.gain_db = -6.0;
        let unity = af2_core::time::db_to_gain(c.gain_db) as f64;
        for k in 0..40 {
            let t = 2.0 + 4.0 * k as f64 / 40.0;
            let played = c.gain_at(t) as f64 / unity;
            assert!((fade_level(&c, t) - played).abs() < 1e-5, "at {t}");
        }
    }

    #[test]
    fn partial_overlap_is_only_for_clips_neither_inside_the_other() {
        assert_eq!(partial_overlap((0.0, 3.0), (2.0, 5.0)), Some(1.0));
        assert_eq!(partial_overlap((2.0, 5.0), (0.0, 3.0)), Some(1.0));
        assert_eq!(partial_overlap((0.0, 10.0), (2.0, 5.0)), None);
        assert_eq!(partial_overlap((0.0, 3.0), (3.0, 5.0)), None);
    }

    #[test]
    fn a_dragged_clip_lands_on_top_of_what_it_now_overlaps() {
        // 1 under 2; 3 is dragged (it was first in the list) onto both.
        let mut cl = vec![clip(3, 2.0, 2.0), clip(1, 0.0, 4.0), clip(2, 3.0, 4.0)];
        restack(&mut cl, 3, &[]);
        assert_eq!(ids(&cl), vec![1, 2, 3]);
        // A clip already on top stays where it is.
        restack(&mut cl, 3, &[]);
        assert_eq!(ids(&cl), vec![1, 2, 3]);
        // Overlapping nothing, it keeps its place.
        let mut cl = vec![clip(3, 20.0, 2.0), clip(1, 0.0, 4.0)];
        restack(&mut cl, 3, &[]);
        assert_eq!(ids(&cl), vec![3, 1]);
    }

    #[test]
    fn a_clip_tucked_under_another_before_the_drag_stays_under() {
        // 1 lies under 2 at the start of the drag.
        let mut cl = vec![clip(1, 0.0, 4.0), clip(2, 2.0, 4.0)];
        let mut tucked = tucked_under(&cl, 1);
        assert_eq!(tucked, vec![2]);
        cl[0].start = 1.0; // nudged, still under 2
        prune_tucked(&cl, 1, &mut tucked);
        restack(&mut cl, 1, &tucked);
        assert_eq!(ids(&cl), vec![1, 2]);
        // Dragged off 2, then back on: it goes over.
        cl[0].start = 10.0;
        prune_tucked(&cl, 1, &mut tucked);
        assert!(tucked.is_empty());
        cl[0].start = 1.0;
        prune_tucked(&cl, 1, &mut tucked);
        restack(&mut cl, 1, &tucked);
        assert_eq!(ids(&cl), vec![2, 1]);
    }

    #[test]
    fn tucked_clips_stay_above_when_the_clip_rises_over_others() {
        // 1 tucked under 3; dragged onto 2 too (2 lies above 3 in the list).
        let mut cl = vec![clip(1, 0.0, 4.0), clip(3, 2.0, 4.0), clip(2, 1.0, 1.0)];
        restack(&mut cl, 1, &[3]);
        assert_eq!(ids(&cl), vec![2, 1, 3]);
    }

    #[test]
    fn a_partial_overlap_becomes_a_crossfade_and_goes_with_it() {
        let mut others = vec![clip(1, 0.0, 4.0)];
        let mut m = clip(2, 10.0, 4.0);
        let prev = span(&m);
        m.start = 3.0; // overlaps 1 by a second
        auto_crossfade(&mut m, Some(prev), true, &mut others);
        assert_eq!(others[0].fade_out, 1.0);
        assert_eq!(m.fade_in, 1.0);
        // Further over: the crossfade follows the overlap.
        let prev = span(&m);
        m.start = 2.5;
        auto_crossfade(&mut m, Some(prev), true, &mut others);
        assert_eq!(others[0].fade_out, 1.5);
        assert_eq!(m.fade_in, 1.5);
        // Dragged clear: the automatic fades go.
        let prev = span(&m);
        m.start = 8.0;
        auto_crossfade(&mut m, Some(prev), true, &mut others);
        assert_eq!(others[0].fade_out, 0.0);
        assert_eq!(m.fade_in, 0.0);
    }

    #[test]
    fn a_clip_wholly_inside_gets_no_crossfade_and_loses_the_automatic_one() {
        let mut others = vec![clip(1, 0.0, 10.0)];
        let mut m = clip(2, 9.0, 2.0);
        auto_crossfade(&mut m, None, true, &mut others);
        assert_eq!((others[0].fade_out, m.fade_in), (1.0, 1.0));
        let prev = span(&m);
        m.start = 4.0; // wholly inside
        auto_crossfade(&mut m, Some(prev), true, &mut others);
        assert_eq!((others[0].fade_out, m.fade_in), (0.0, 0.0));
        // Placed inside from the start: nothing.
        let mut m = clip(3, 2.0, 2.0);
        auto_crossfade(&mut m, None, true, &mut others);
        assert_eq!(
            (others[0].fade_in, others[0].fade_out, m.fade_in, m.fade_out),
            (0.0, 0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn fades_set_by_hand_stay_when_the_overlap_goes() {
        let mut others = vec![clip(1, 0.0, 4.0)];
        others[0].fade_out = 0.7; // the user's own
        let mut m = clip(2, 3.0, 4.0);
        m.fade_in = 0.4; // the user's own
        let prev = span(&m);
        m.start = 9.0;
        auto_crossfade(&mut m, Some(prev), true, &mut others);
        assert_eq!(others[0].fade_out, 0.7);
        assert_eq!(m.fade_in, 0.4);
    }

    #[test]
    fn a_drag_works_from_the_gesture_start_and_restores_passed_fades() {
        let mut s = Session::default();
        s.tracks[0].clips = vec![clip(1, 0.0, 4.0), clip(2, 10.0, 4.0)];
        s.tracks[0].clips[0].fade_out = 0.3; // the user's own
        let before = s.clone();
        // Over clip 1's end: a crossfade replaces its fade.
        s.tracks[0].clips[1].start = 3.0;
        auto_crossfade_drag(&mut s, &before, 2);
        assert_eq!(s.tracks[0].clips[0].fade_out, 1.0);
        assert_eq!(s.tracks[0].clips[1].fade_in, 1.0);
        // Back where it was: clip 1 gets its own fade back.
        s.tracks[0].clips[1].start = 10.0;
        auto_crossfade_drag(&mut s, &before, 2);
        assert_eq!(s.tracks[0].clips[0].fade_out, 0.3);
        assert_eq!(s.tracks[0].clips[1].fade_in, 0.0);
    }

    #[test]
    fn linked_crossfades_bend_the_opposite_way() {
        let mut cl = vec![clip(1, 0.0, 4.0), clip(2, 3.0, 4.0)];
        cl[0].fade_out = 1.0;
        cl[1].fade_in = 1.0;
        assert_eq!(crossfade_partner(&cl, 2, true), Some(1));
        assert_eq!(crossfade_partner(&cl, 1, false), Some(2));
        assert_eq!(crossfade_partner(&cl, 1, true), None);
        let look = FadeLook {
            skew: 40.0,
            bell: false,
        };
        assert_eq!(link_fade(&mut cl, 2, true, look, true), Some(1));
        assert_eq!(cl[1].props.fade_in_skew, 40.0);
        assert_eq!(cl[0].props.fade_out_skew, -40.0);
        // A bell on one is a bell on both; a reset straightens both.
        link_fade(
            &mut cl,
            1,
            false,
            FadeLook {
                skew: 0.0,
                bell: true,
            },
            true,
        );
        assert!(cl[1].props.fade_in_bell && cl[0].props.fade_out_bell);
        assert_eq!(cl[1].props.fade_in_skew, 0.0);
        // Unlinked: only the one.
        link_fade(&mut cl, 2, true, look, false);
        assert_eq!(cl[0].props.fade_out_skew, 0.0);
        // Different lengths are not a pair.
        cl[0].fade_out = 0.5;
        assert_eq!(crossfade_partner(&cl, 2, true), None);
    }

    #[test]
    fn hidden_edges_are_the_ends_under_a_clip_on_top() {
        // "a" 0..3 under "b" 2..5: a's end at 3 is hidden as far as 2.
        let cl = vec![clip(1, 0.0, 3.0), clip(2, 2.0, 3.0)];
        assert_eq!(
            hidden_edges(&cl),
            vec![HiddenEdge {
                clip: 1,
                end: true,
                t: 3.0,
                to: 2.0
            }]
        );
        // Under a clip wholly over it, both of its edges are hidden.
        let cl = vec![clip(1, 2.0, 1.0), clip(2, 0.0, 10.0)];
        let e = hidden_edges(&cl);
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|e| e.clip == 1));
        // A clip on top hides nothing of its own.
        let cl = vec![clip(2, 0.0, 10.0), clip(1, 2.0, 1.0)];
        assert!(hidden_edges(&cl).is_empty());
        // Grabbed within a few pixels.
        let e = hidden_edges(&[clip(1, 0.0, 3.0), clip(2, 2.0, 3.0)]);
        let x_of = |t: f64| (t * 100.0) as f32;
        assert_eq!(
            hidden_edge_near(&e, 301.0, 5.0, x_of).map(|e| e.clip),
            Some(1)
        );
        assert!(hidden_edge_near(&e, 310.0, 5.0, x_of).is_none());
    }

    #[test]
    fn the_source_range_follows_offset_stretch_and_file_length() {
        let mut c = clip(1, 5.0, 4.0);
        c.offset = 2.0;
        assert_eq!(source_range(&c, 0.0), (2.0, 6.0));
        // Stretched to twice as long: half as much of the file.
        c.props.stretch.mode = af2_core::StretchMode::Live;
        c.props.stretch.ratio = 2.0;
        let r = source_range(&c, 0.0);
        assert!(
            (r.0 - 2.0).abs() < 1e-9 && (r.1 - 4.0).abs() < 1e-9,
            "{r:?}"
        );
        // Reversed plays the same stretch of the file.
        c.props.reversed = true;
        assert_eq!(source_range(&c, 0.0), r);
        // Bounded by the file.
        c.props.stretch.ratio = 0.5;
        assert_eq!(source_range(&c, 7.0), (2.0, 7.0));
    }
}
