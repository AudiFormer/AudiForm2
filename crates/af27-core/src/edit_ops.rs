// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Session edits the Multitrack menus use: track management with bus and
//! sidechain clean-up, ripple deletes, trims to the playhead, and the
//! feedback check routing pickers rely on.

use crate::session::{ClipId, Session, TrackId, TrackKind};

impl Session {
    /// Removes a track. Whatever went to it (a bus) goes to the master
    /// instead, sends to it are dropped, and effects keyed from it go
    /// back to listening to their own input.
    pub fn delete_track(&mut self, id: TrackId) -> bool {
        let Some(i) = self.track_index(id) else {
            return false;
        };
        self.tracks.remove(i);
        for t in &mut self.tracks {
            if t.output == Some(id) {
                t.output = None;
            }
            t.sends.retain(|s| s.bus != id);
            for slot in &mut t.chain {
                if slot.sidechain == Some(id) {
                    slot.sidechain = None;
                }
            }
        }
        for slot in &mut self.master_chain {
            if slot.sidechain == Some(id) {
                slot.sidechain = None;
            }
        }
        true
    }

    /// A copy of a track (clips and all) right below it. Returns its id.
    pub fn duplicate_track(&mut self, id: TrackId) -> Option<TrackId> {
        let i = self.track_index(id)?;
        let mut t = self.tracks[i].clone();
        t.id = self.alloc_id();
        t.name = format!("{} copy", t.name);
        t.ext_id = None;
        for c in &mut t.clips {
            c.id = self.alloc_id();
            c.ext_id = None;
        }
        for c in &mut t.midi {
            c.id = self.alloc_id();
        }
        for s in &mut t.sends {
            s.ext_id = None;
        }
        for s in &mut t.chain {
            s.ext_id = None;
        }
        let new = t.id;
        self.tracks.insert(i + 1, t);
        Some(new)
    }

    /// Removes audio and MIDI tracks with nothing on them (buses stay).
    /// Returns how many went.
    pub fn delete_empty_tracks(&mut self) -> usize {
        let empty: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|t| t.kind != TrackKind::Bus && t.clips.is_empty() && t.midi.is_empty())
            .map(|t| t.id)
            .collect();
        for id in &empty {
            self.delete_track(*id);
        }
        empty.len()
    }

    /// Moves a track to position `to` (0 is the top).
    pub fn move_track(&mut self, id: TrackId, to: usize) {
        if let Some(i) = self.track_index(id) {
            let t = self.tracks.remove(i);
            let to = to.min(self.tracks.len());
            self.tracks.insert(to, t);
        }
    }

    /// True when sound from `from` reaches `to`: through outputs, sends
    /// or sidechain keys. Routing `x` into bus `b` is refused when `b`
    /// already reaches `x` (it would feed back).
    pub fn reaches(&self, from: TrackId, to: TrackId) -> bool {
        let mut seen = vec![from];
        let mut stack = vec![from];
        while let Some(cur) = stack.pop() {
            if cur == to {
                return true;
            }
            let mut next: Vec<TrackId> = Vec::new();
            if let Some(t) = self.tracks.iter().find(|t| t.id == cur) {
                next.extend(t.output);
                next.extend(t.sends.iter().map(|s| s.bus));
            }
            // A track keying an effect elsewhere feeds that track.
            for t in &self.tracks {
                if t.chain.iter().any(|s| s.sidechain == Some(cur)) {
                    next.push(t.id);
                }
            }
            for n in next {
                if !seen.contains(&n) {
                    seen.push(n);
                    stack.push(n);
                }
            }
        }
        false
    }

    /// Whether `track` may send or output to (or key an effect on) `target`
    /// without sound coming back round to itself.
    pub fn can_route(&self, track: TrackId, target: TrackId) -> bool {
        track != target && !self.reaches(target, track)
    }

    /// Every track and bus whose output (not sends) reaches `bus`,
    /// directly or through other buses.
    pub fn feeders(&self, bus: TrackId) -> Vec<TrackId> {
        let mut out: Vec<TrackId> = Vec::new();
        let mut stack = vec![bus];
        while let Some(b) = stack.pop() {
            for t in &self.tracks {
                if t.output == Some(b) && !out.contains(&t.id) && t.id != bus {
                    out.push(t.id);
                    stack.push(t.id);
                }
            }
        }
        out
    }

    /// Solos (or unsolos) a track. `exclusive` unsolos every other track
    /// and bus first (Exclusive Solo, or Ctrl+click: Solo Track Only).
    /// Soloing a bus does the same to everything whose output reaches it,
    /// so the buttons show what the engine plays.
    pub fn set_solo(&mut self, id: TrackId, on: bool, exclusive: bool) {
        if exclusive && on {
            for t in &mut self.tracks {
                t.solo = false;
            }
        }
        let is_bus = self
            .tracks
            .iter()
            .any(|t| t.id == id && t.kind == TrackKind::Bus);
        let mut ids = vec![id];
        if is_bus {
            ids.extend(self.feeders(id));
        }
        for t in &mut self.tracks {
            if ids.contains(&t.id) {
                t.solo = on;
            }
        }
    }

    /// Sets a sends list for a track, tidying each send (level -96..12
    /// dB, pan -1..1). Refused (false, nothing changed) when a send names
    /// no bus or would feed back.
    pub fn set_sends(&mut self, id: TrackId, sends: Vec<crate::session::Send>) -> bool {
        let ok = sends.iter().all(|s| {
            self.tracks
                .iter()
                .any(|t| t.id == s.bus && t.kind == TrackKind::Bus)
                && self.can_route(id, s.bus)
        });
        if !ok {
            return false;
        }
        match self.track_mut(id) {
            Some(t) => {
                t.sends = sends;
                for s in &mut t.sends {
                    s.tidy();
                }
                true
            }
            None => false,
        }
    }

    /// Sets a track's output (None: the master). Refused when the target
    /// is not a bus or would feed back.
    pub fn set_output(&mut self, id: TrackId, to: Option<TrackId>) -> bool {
        if let Some(b) = to {
            let is_bus = self
                .tracks
                .iter()
                .any(|t| t.id == b && t.kind == TrackKind::Bus);
            if !is_bus || !self.can_route(id, b) {
                return false;
            }
        }
        match self.track_mut(id) {
            Some(t) => {
                t.output = to;
                true
            }
            None => false,
        }
    }

    /// True when any routing in the session feeds back (only a damaged
    /// file can hold one; the engine still plays it).
    pub fn has_loop(&self) -> bool {
        self.tracks.iter().any(|t| {
            t.output
                .into_iter()
                .chain(t.sends.iter().map(|s| s.bus))
                .any(|b| b == t.id || self.reaches(b, t.id))
                || t.chain
                    .iter()
                    .filter_map(|s| s.sidechain)
                    .filter(|&k| self.track_index(k).is_some())
                    .any(|k| k == t.id || self.reaches(t.id, k))
        })
    }

    /// Splits every clip under time `t`. Returns how many were split.
    pub fn split_all_at(&mut self, t: f64) -> usize {
        let ids: Vec<ClipId> = self
            .tracks
            .iter()
            .flat_map(|tr| tr.clips.iter())
            .filter(|c| t > c.start + 1e-6 && t < c.end() - 1e-6)
            .map(|c| c.id)
            .collect();
        ids.iter()
            .filter(|id| self.split_clip(**id, t).is_some())
            .count()
    }

    /// Removes clips and closes the gap each leaves on its track.
    pub fn ripple_delete_clips(&mut self, ids: &[ClipId]) {
        // Latest first, so earlier removals do not move later ones twice.
        let mut found: Vec<(usize, f64, f64)> = ids
            .iter()
            .filter_map(|id| {
                let (ti, ci) = self.find_clip(*id)?;
                let c = &self.tracks[ti].clips[ci];
                Some((ti, c.start, c.length))
            })
            .collect();
        found.sort_by(|a, b| b.1.total_cmp(&a.1));
        for id in ids {
            self.remove_clip(*id);
        }
        for (ti, start, len) in found {
            for c in &mut self.tracks[ti].clips {
                if c.start >= start - 1e-9 {
                    c.start = (c.start - len).max(0.0);
                }
            }
        }
    }

    /// Cuts the time `a..b` out of the given tracks (all when None): what
    /// lies inside goes, what comes after moves back by `b - a`.
    pub fn ripple_delete_time(&mut self, a: f64, b: f64, tracks: Option<&[TrackId]>) {
        if b <= a {
            return;
        }
        let len = b - a;
        let which: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|t| tracks.is_none_or(|ts| ts.contains(&t.id)))
            .map(|t| t.id)
            .collect();
        for tid in which {
            let ids: Vec<ClipId> = self
                .track_mut(tid)
                .map(|t| t.clips.iter().map(|c| c.id).collect())
                .unwrap_or_default();
            for id in ids {
                // Split at both edges, then drop what is inside.
                let right = self.split_clip(id, a);
                let mid = right.unwrap_or(id);
                self.split_clip(mid, b);
            }
            let Some(t) = self.track_mut(tid) else {
                continue;
            };
            t.clips
                .retain(|c| !(c.start >= a - 1e-9 && c.end() <= b + 1e-9));
            for c in &mut t.clips {
                if c.start >= b - 1e-9 {
                    c.start -= len;
                }
            }
        }
    }

    /// Closes the empty gap on `track` around time `t`: the clips after it
    /// move back to meet the clip before it (or the session start).
    pub fn ripple_delete_gap(&mut self, track: TrackId, t: f64) -> bool {
        let Some(tr) = self.track_mut(track) else {
            return false;
        };
        if tr.clips.iter().any(|c| t >= c.start && t < c.end()) {
            return false;
        }
        let before = tr
            .clips
            .iter()
            .filter(|c| c.end() <= t)
            .map(|c| c.end())
            .fold(0.0, f64::max);
        let Some(after) = tr
            .clips
            .iter()
            .filter(|c| c.start > t)
            .map(|c| c.start)
            .min_by(f64::total_cmp)
        else {
            return false;
        };
        let gap = after - before;
        for c in &mut tr.clips {
            if c.start >= after - 1e-9 {
                c.start -= gap;
            }
        }
        gap > 0.0
    }

    /// Moves a clip's start to `t`, keeping its end (Trim Start to
    /// Playhead). Source seconds follow the clip's stretch.
    pub fn trim_start_to(&mut self, id: ClipId, t: f64) -> bool {
        let Some((ti, ci)) = self.find_clip(id) else {
            return false;
        };
        let c = &mut self.tracks[ti].clips[ci];
        if t <= c.start || t >= c.end() {
            return false;
        }
        let d = t - c.start;
        let ratio = c.props.stretch.applied_ratio();
        c.start = t;
        c.length -= d;
        if !c.props.reversed {
            c.offset += d / ratio;
        }
        c.fade_in = c.fade_in.min(c.length);
        c.fade_out = c.fade_out.min(c.length);
        true
    }

    /// Moves a clip's end to `t` (Trim End to Playhead).
    pub fn trim_end_to(&mut self, id: ClipId, t: f64) -> bool {
        let Some((ti, ci)) = self.find_clip(id) else {
            return false;
        };
        let c = &mut self.tracks[ti].clips[ci];
        if t <= c.start || t >= c.end() {
            return false;
        }
        let d = c.end() - t;
        let ratio = c.props.stretch.applied_ratio();
        c.length -= d;
        if c.props.reversed {
            c.offset += d / ratio;
        }
        c.fade_in = c.fade_in.min(c.length);
        c.fade_out = c.fade_out.min(c.length);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::session::{MediaItem, Send, Session, TrackKind};

    fn with_media() -> (Session, u64) {
        let mut s = Session::new("t", 48_000, 2);
        let id = s.alloc_id();
        s.media.push(MediaItem {
            id,
            name: "a".into(),
            path: None,
            duration: 10.0,
            rate: 48_000,
            channels: 2,
            ext_id: None,
        });
        (s, id)
    }

    #[test]
    fn deleting_a_bus_reroutes_what_fed_it() {
        let mut s = Session::new("t", 48_000, 2);
        let bus = s.add_track_of(TrackKind::Bus);
        s.tracks[0].output = Some(bus);
        s.tracks[1].sends.push(Send {
            bus,
            level_db: 0.0,
            pre_fader: false,
            pan: 0.0,
            enabled: true,
            ext_id: None,
        });
        s.delete_track(bus);
        assert_eq!(s.tracks.len(), 2);
        assert_eq!(s.tracks[0].output, None);
        assert!(s.tracks[1].sends.is_empty());
    }

    #[test]
    fn buses_route_send_and_solo_in_one_step() {
        let mut s = Session::new("t", 48_000, 2);
        let (a, b) = (s.tracks[0].id, s.tracks[1].id);
        let grp = s.add_track_of(TrackKind::Bus);
        let verb = s.add_track_of(TrackKind::Bus);
        assert!(s.set_output(a, Some(grp)));
        assert!(s.set_output(grp, Some(verb)));
        // A loop is refused and changes nothing.
        assert!(!s.set_output(verb, Some(grp)));
        assert_eq!(s.track_mut(verb).unwrap().output, None);
        // Sends are tidied; one that would feed back is refused.
        let mut loud = Send::new(verb);
        loud.level_db = 40.0;
        loud.pan = -3.0;
        assert!(s.set_sends(b, vec![loud]));
        let sent = &s.track_mut(b).unwrap().sends[0];
        assert_eq!((sent.level_db, sent.pan), (12.0, -1.0));
        assert!(!s.set_sends(verb, vec![Send::new(grp)]));
        assert!(!s.has_loop());
        // Soloing a bus solos what outputs into it (not what sends).
        s.set_solo(verb, true, false);
        let solos: Vec<bool> = s.tracks.iter().map(|t| t.solo).collect();
        assert_eq!(solos, vec![true, false, true, true]);
        // Exclusive: only the one.
        s.set_solo(b, true, true);
        let solos: Vec<bool> = s.tracks.iter().map(|t| t.solo).collect();
        assert_eq!(solos, vec![false, true, false, false]);
        // A damaged file's loop is seen.
        s.track_mut(verb).unwrap().output = Some(grp);
        assert!(s.has_loop());
    }

    #[test]
    fn routing_refuses_feedback() {
        let mut s = Session::new("t", 48_000, 1);
        let a = s.add_track_of(TrackKind::Bus);
        let b = s.add_track_of(TrackKind::Bus);
        let t = s.tracks[0].id;
        s.track_mut(a).unwrap().output = Some(b);
        assert!(s.can_route(t, a));
        assert!(s.can_route(a, b));
        assert!(!s.can_route(b, a), "b into a would loop through a -> b");
        assert!(!s.can_route(a, a));
    }

    #[test]
    fn ripple_delete_time_closes_the_gap() {
        let (mut s, m) = with_media();
        let t = s.tracks[0].id;
        s.add_clip(t, m, 0.0);
        s.ripple_delete_time(2.0, 5.0, None);
        let c = &s.tracks[0].clips;
        assert_eq!(c.len(), 2);
        assert!((c[0].length - 2.0).abs() < 1e-9);
        assert!((c[1].start - 2.0).abs() < 1e-9);
        assert!((c[1].offset - 5.0).abs() < 1e-9);
        assert!((s.duration() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn ripple_delete_clips_and_gaps() {
        let (mut s, m) = with_media();
        let t = s.tracks[0].id;
        let a = s.add_clip(t, m, 0.0).unwrap();
        s.add_clip(t, m, 15.0).unwrap();
        assert!(s.ripple_delete_gap(t, 12.0));
        assert!((s.tracks[0].clips[1].start - 10.0).abs() < 1e-9);
        s.ripple_delete_clips(&[a]);
        assert_eq!(s.tracks[0].clips.len(), 1);
        assert!(s.tracks[0].clips[0].start.abs() < 1e-9);
    }

    #[test]
    fn trims_and_split_all() {
        let (mut s, m) = with_media();
        let t0 = s.tracks[0].id;
        let t1 = s.tracks[1].id;
        let a = s.add_clip(t0, m, 0.0).unwrap();
        s.add_clip(t1, m, 1.0).unwrap();
        assert_eq!(s.split_all_at(4.0), 2);
        assert!(s.trim_start_to(a, 1.0));
        let c = &s.tracks[0].clips[0];
        assert!((c.start - 1.0).abs() < 1e-9 && (c.offset - 1.0).abs() < 1e-9);
        assert!((c.length - 3.0).abs() < 1e-9);
        assert!(s.trim_end_to(a, 2.0));
        assert!((s.tracks[0].clips[0].length - 1.0).abs() < 1e-9);
    }

    #[test]
    fn duplicate_and_delete_empty() {
        let (mut s, m) = with_media();
        let t0 = s.tracks[0].id;
        s.add_clip(t0, m, 0.0);
        let d = s.duplicate_track(t0).unwrap();
        assert_eq!(s.tracks[1].id, d);
        assert_ne!(s.tracks[1].clips[0].id, s.tracks[0].clips[0].id);
        assert_eq!(s.delete_empty_tracks(), 1);
        assert_eq!(s.tracks.len(), 2);
    }
}
