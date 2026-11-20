// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The 3D Space parts of the mix graph.
//!
//! * Placing: each track or bus that reaches the master (or a 3D Bus) is
//!   put on the speaker bed after its chain and fader, by its spot
//!   (`af27_space::Placement`). Gains glide from the last block's to this
//!   block's across the block, so a moving source never clicks. Spread
//!   and Spatialize add decorrelated copies (main output only).
//! * Spot automation: once per block, at its middle.
//! * 3D Buses: their inputs are placed on their own bed; their effects
//!   run on every speaker; the fader goes on; the bed joins the master's.
//! * Multichannel clips: each channel straight to its speaker by name
//!   (`af27_space::channels`), on a bed of the track's own, through the
//!   track's effects on every speaker, then its fader.
//! * The low-frequency channel's band filter, and per-speaker meters.

use af27_core::space::{SpaceLane, SpaceLayout, SpacePos, SpatializeMode};
use af27_dsp::wide::{build_wide_chain, WideChain};
use af27_space::diffuse::Diffuser;
use af27_space::{Placement, MAX_SPEAKERS};

use super::{Mix, MixClip, MixTrack, BLOCK};

/// A track's or bus's place in the room, with its ramp and field state.
pub struct TrackPlace {
    layout: SpaceLayout,
    stereo: bool,
    pos: SpacePos,
    lanes: Vec<SpaceLane>,
    pub target: Placement,
    prev: Placement,
    /// No block played since it was built or reset: start where it is
    /// rather than gliding in.
    fresh: bool,
    diffuser: Box<Diffuser>,
}

impl TrackPlace {
    pub fn new(
        layout: SpaceLayout,
        pos: SpacePos,
        lanes: Vec<SpaceLane>,
        stereo: bool,
        rate: u32,
        k: usize,
    ) -> TrackPlace {
        let lanes: Vec<SpaceLane> = lanes
            .into_iter()
            .filter(|l| l.enabled && !l.points.is_empty())
            .collect();
        let target = compute(layout, &pos, stereo);
        TrackPlace {
            layout,
            stereo,
            pos,
            lanes,
            target,
            prev: target,
            fresh: true,
            diffuser: Box::new(Diffuser::new(rate, k)),
        }
    }

    /// A new spot (dragged live). No allocation.
    pub fn set_pos(&mut self, pos: SpacePos) {
        self.pos = pos;
        self.target = compute(self.layout, &pos, self.stereo);
    }

    /// Moves automated values to time `t`.
    fn automate(&mut self, t: f64) {
        if !self.lanes.is_empty() {
            let p = self.pos.at_time(&self.lanes, t);
            self.target = compute(self.layout, &p, self.stereo);
        }
        if std::mem::take(&mut self.fresh) {
            self.prev = self.target;
        }
    }

    fn reset(&mut self) {
        self.diffuser.clear();
        self.fresh = true;
    }

    /// Adds `n` frames of `l`/`r` (times `gain`) to interleaved `bed`
    /// (`ch` wide), gliding from the last block's gains. `field` adds
    /// Spread and Spatialize.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        l: &[f32],
        r: &[f32],
        n: usize,
        bed: &mut [f32],
        ch: usize,
        lfe: Option<usize>,
        field: bool,
        gain: f32,
    ) {
        let (a, b) = (self.prev, self.target);
        let moving = a != b;
        let field = field && (a.has_field() || b.has_field());
        let mut cur = b;
        let width = ch.min(MAX_SPEAKERS);
        for i in 0..n {
            let (x, y) = (l[i] * gain, r[i] * gain);
            if moving {
                lerp_into(&a, &b, (i + 1) as f32 / n as f32, &mut cur);
            }
            let frame = &mut bed[i * ch..(i + 1) * ch];
            for ((o, gl), gr) in frame[..width].iter_mut().zip(&cur.bed[0]).zip(&cur.bed[1]) {
                *o += x * gl + y * gr;
            }
            if let Some(lc) = lfe {
                frame[lc] += (x + y) * cur.lfe;
            }
            if field {
                self.diffuser.run(x, y, &cur, &mut frame[..width]);
            }
        }
    }
}

/// The placement for a spot. Place Elements is not built yet: a source
/// set to it plays from its spot as if Spatialize were off.
fn compute(layout: SpaceLayout, pos: &SpacePos, stereo: bool) -> Placement {
    let mut p = *pos;
    if p.spatialize_mode == SpatializeMode::Steer {
        p.spatialize = 0.0;
    }
    Placement::new(layout, &p, stereo)
}

fn lerp_into(a: &Placement, b: &Placement, f: f32, out: &mut Placement) {
    let mix = |x: f32, y: f32| x + (y - x) * f;
    for s in 0..2 {
        for c in 0..MAX_SPEAKERS {
            out.bed[s][c] = mix(a.bed[s][c], b.bed[s][c]);
        }
    }
    for c in 0..MAX_SPEAKERS {
        out.spread[c] = mix(a.spread[c], b.spread[c]);
        out.side[c] = mix(a.side[c], b.side[c]);
    }
    out.lfe = mix(a.lfe, b.lfe);
    out.center = mix(
        if a.center_channel == b.center_channel {
            a.center
        } else {
            0.0
        },
        b.center,
    );
}

/// A 3D Bus's own bed and its effects on every speaker.
pub struct RoomBed {
    pub chain: WideChain,
    bed: Vec<f32>,
}

/// A clip whose channels play from their speakers.
pub struct FeedClip {
    pub clip: MixClip,
    /// Per file channel: gain per speaker.
    pub gains: Vec<af27_space::Gains>,
}

/// A track's multichannel clips, on a bed of its own.
pub struct TrackFeed {
    pub clips: Vec<FeedClip>,
    /// The track's effects, on every speaker.
    pub chain: WideChain,
    bed: Vec<f32>,
    /// Per speaker: stereo fold, for a track that outputs to a stereo bus.
    fold: Vec<[f32; 2]>,
}

/// Everything 3D about one track of the mix.
pub struct TrackSpaceMix {
    /// None for a 3D Bus (not placed itself).
    pub place: Option<TrackPlace>,
    pub room: Option<RoomBed>,
    pub feed: Option<TrackFeed>,
}

/// Builds a track's 3D parts. `wide` are its clips of more than two
/// channels that play from their speakers.
pub fn for_track(
    layout: SpaceLayout,
    t: &af27_core::Track,
    wide: Vec<MixClip>,
    stereo: bool,
    rate: u32,
    k: usize,
) -> Box<TrackSpaceMix> {
    let names = bed_names(layout);
    let ch = names.len();
    let room = (t.kind == af27_core::TrackKind::Bus && t.spatial.room).then(|| RoomBed {
        chain: {
            let mut c = build_wide_chain(&t.chain, rate, &names, BLOCK);
            c.set_rack(&t.rack);
            c
        },
        bed: vec![0.0; BLOCK * ch],
    });
    // An Audiform session says whether the track is a pair or a point.
    let stereo = t.spatial.stereo.unwrap_or(stereo);
    let place = room
        .is_none()
        .then(|| TrackPlace::new(layout, t.pos, t.spatial.lanes.clone(), stereo, rate, k));
    let feed = (!wide.is_empty()).then(|| TrackFeed {
        clips: wide
            .into_iter()
            .map(|clip| {
                let n = clip.buf.num_channels();
                // Named by count: decoded files do not carry their WAV
                // speaker mask yet (`channels::names` reads one).
                let names = af27_space::channels::names(n, None);
                let gains = af27_space::channels::targets(layout, &names)
                    .iter()
                    .map(|t| t.gains())
                    .collect();
                FeedClip { clip, gains }
            })
            .collect(),
        chain: {
            let mut c = build_wide_chain(&t.chain, rate, &names, BLOCK);
            c.set_rack(&t.rack);
            c
        },
        bed: vec![0.0; BLOCK * ch],
        fold: af27_space::fold_bed(layout),
    });
    Box::new(TrackSpaceMix { place, room, feed })
}

pub fn bed_names(layout: SpaceLayout) -> Vec<String> {
    af27_space::speakers(layout)
        .iter()
        .map(|s| s.name.to_string())
        .collect()
}

/// Whether clip `c` of a 3D session plays each channel from its speaker.
pub fn plays_to_speakers(c: &af27_core::Clip, channels: usize) -> bool {
    channels > 2 && !c.props.fold_channels
}

/// The track's own 3D work for a block (on the engine's threads): its
/// multichannel clips onto its bed and through its effects, or a 3D
/// Bus's effects on its bed.
pub fn render_track(t: &mut MixTrack, t0: f64, dt: f64, n: usize) {
    let Some(sp) = t.sp.as_deref_mut() else {
        return;
    };
    if let Some(room) = &mut sp.room {
        let ch = room.chain.channels();
        if t.audible {
            room.chain.process(&mut room.bed[..n * ch]);
        }
    }
    if let Some(feed) = &mut sp.feed {
        let ch = feed.chain.channels();
        feed.bed[..n * ch].fill(0.0);
        if !t.audible {
            return;
        }
        for fc in &feed.clips {
            let c = &fc.clip;
            if c.start >= t0 + dt * n as f64 || c.start + c.length <= t0 {
                continue;
            }
            read_clip_bed(fc, t0, dt, n, &mut feed.bed, ch);
        }
        feed.chain.process(&mut feed.bed[..n * ch]);
    }
}

/// Adds one multichannel clip's block to `bed`, each file channel by its
/// speaker gains.
fn read_clip_bed(fc: &FeedClip, t0: f64, dt: f64, n: usize, bed: &mut [f32], ch: usize) {
    let c = &fc.clip;
    let (buf, offset, speed, reversed) = match c.prepared.as_deref().and_then(|p| p.ready()) {
        Some(b) => (b.as_ref(), 0.0, 1.0, false),
        None => (c.buf.as_ref(), c.offset, c.speed, c.reversed),
    };
    let rate = buf.rate as f64;
    let first = if c.start > t0 {
        (((c.start - t0) / dt).ceil() as usize).min(n)
    } else {
        0
    };
    let last = (((c.start + c.length - t0) / dt).ceil().max(0.0) as usize).min(n);
    let width = ch.min(MAX_SPEAKERS);
    for i in first..last {
        let local = t0 + dt * i as f64 - c.start;
        if local < 0.0 || local >= c.length {
            continue;
        }
        let x = if reversed {
            let end = (c.length * rate).round() - 1.0;
            offset * rate + (end - local * rate) * speed
        } else {
            (offset + local * speed) * rate
        };
        let g = super::clip_gain(c, local);
        let frame = &mut bed[i * ch..(i + 1) * ch];
        for (k, gains) in fc.gains.iter().enumerate() {
            let Some(src) = buf.channels.get(k) else {
                break;
            };
            let v = super::sample(src, x) * g;
            if v == 0.0 {
                continue;
            }
            for s in 0..width {
                frame[s] += v * gains[s];
            }
        }
    }
}

/// Splits out two different tracks, both writable.
fn two_mut(tracks: &mut [MixTrack], a: usize, b: usize) -> (&mut MixTrack, &mut MixTrack) {
    debug_assert_ne!(a, b);
    if a < b {
        let (x, y) = tracks.split_at_mut(b);
        (&mut x[a], &mut y[0])
    } else {
        let (x, y) = tracks.split_at_mut(a);
        (&mut y[0], &mut x[b])
    }
}

impl Mix {
    /// Where track `ti`'s main output goes: Some(bus) or None (master).
    fn dest(&self, ti: usize) -> Option<usize> {
        let t = &self.tracks[ti];
        t.output.filter(|&bus| bus != ti && (bus > ti || !t.is_bus))
    }

    fn is_room(&self, i: usize) -> bool {
        self.tracks[i].sp.as_ref().is_some_and(|s| s.room.is_some())
    }

    /// Sends track `ti`'s main output into the room (3D sessions).
    /// Returns false when the stereo sound still has to go to a stereo
    /// bus, as in a stereo session.
    pub(super) fn route_space(&mut self, ti: usize, n: usize) -> bool {
        if self.space.is_none() {
            return false;
        }
        let dest = self.dest(ti);
        let room_dest = dest.filter(|&b| self.is_room(b));
        let g = self.tracks[ti].gain_l;
        let Mix { space, tracks, .. } = self;
        let space = space.as_mut().unwrap();
        let ch = space.channels;
        let lfe = space.lfe_channel;
        // A 3D Bus: its processed bed, through its fader, to the master.
        if let Some(room) = tracks[ti].sp.as_deref().and_then(|s| s.room.as_ref()) {
            for (o, v) in space.bed[..n * ch].iter_mut().zip(&room.bed[..n * ch]) {
                *o += v * g;
            }
            return true;
        }
        // Multichannel clips, through the fader, to where the track goes.
        if tracks[ti].sp.as_ref().is_some_and(|s| s.feed.is_some()) {
            match dest {
                None => {
                    let feed = tracks[ti].sp.as_ref().unwrap().feed.as_ref().unwrap();
                    for (o, v) in space.bed[..n * ch].iter_mut().zip(&feed.bed[..n * ch]) {
                        *o += v * g;
                    }
                }
                Some(b) => {
                    let (t, bus) = two_mut(tracks, ti, b);
                    let feed = t.sp.as_ref().unwrap().feed.as_ref().unwrap();
                    match bus.sp.as_deref_mut().and_then(|s| s.room.as_mut()) {
                        Some(room) => {
                            for (o, v) in room.bed[..n * ch].iter_mut().zip(&feed.bed[..n * ch]) {
                                *o += v * g;
                            }
                        }
                        None => {
                            for i in 0..n {
                                let frame = &feed.bed[i * ch..(i + 1) * ch];
                                let (mut l, mut r) = (0.0, 0.0);
                                for (v, f) in frame.iter().zip(&feed.fold) {
                                    l += v * f[0];
                                    r += v * f[1];
                                }
                                bus.l[i] += l * g;
                                bus.r[i] += r * g;
                            }
                        }
                    }
                }
            }
        }
        match (dest, room_dest) {
            // A stereo bus: as in a stereo session.
            (Some(_), None) => false,
            (_, Some(b)) => {
                let (t, bus) = two_mut(tracks, ti, b);
                let room = bus.sp.as_deref_mut().unwrap().room.as_mut().unwrap();
                let MixTrack { sp, l, r, .. } = t;
                if let Some(p) = sp.as_deref_mut().and_then(|s| s.place.as_mut()) {
                    p.place(l, r, n, &mut room.bed, ch, lfe, true, 1.0);
                }
                true
            }
            (None, None) => {
                let MixTrack { sp, l, r, .. } = &mut tracks[ti];
                if let Some(p) = sp.as_deref_mut().and_then(|s| s.place.as_mut()) {
                    p.place(l, r, n, &mut space.bed, ch, lfe, true, 1.0);
                }
                true
            }
        }
    }

    /// A send from `ti` into a 3D Bus `bus` at `gain`: placed by its spot
    /// (no Spread or Spatialize). Returns false for a stereo bus.
    pub(super) fn send_space(&mut self, ti: usize, bus: usize, gain: f32, n: usize) -> bool {
        if self.space.is_none() || !self.is_room(bus) {
            return false;
        }
        let ch = self.space.as_ref().map_or(2, |s| s.channels);
        let lfe = self.space.as_ref().and_then(|s| s.lfe_channel);
        let (t, b) = two_mut(&mut self.tracks, ti, bus);
        let room = b.sp.as_deref_mut().unwrap().room.as_mut().unwrap();
        let MixTrack { sp, l, r, .. } = t;
        if let Some(p) = sp.as_deref_mut().and_then(|s| s.place.as_mut()) {
            p.place(l, r, n, &mut room.bed, ch, lfe, false, gain);
        }
        true
    }

    /// Before a block: clears the 3D Buses' beds and moves automated spots.
    pub(super) fn space_begin(&mut self, t_mid: f64, n: usize) {
        let Some(space) = self.space.as_ref() else {
            return;
        };
        let ch = space.channels;
        for t in &mut self.tracks {
            if let Some(sp) = t.sp.as_deref_mut() {
                if let Some(room) = &mut sp.room {
                    room.bed[..n * ch].fill(0.0);
                }
                if let Some(p) = &mut sp.place {
                    p.automate(t_mid);
                }
            }
        }
    }

    /// After a track's outputs: this block's gains are where the next
    /// block glides from.
    pub(super) fn space_end_track(&mut self, ti: usize) {
        if let Some(p) = self.tracks[ti]
            .sp
            .as_deref_mut()
            .and_then(|s| s.place.as_mut())
        {
            p.prev = p.target;
        }
    }

    /// After every track: the LFE band filter, then the master chain on
    /// the bed, then the per-speaker meters.
    pub(super) fn space_master(&mut self, n: usize) {
        let master = self.master;
        let Some(sp) = self.space.as_mut() else {
            return;
        };
        let ch = sp.channels;
        if let Some(lc) = sp.lfe_channel {
            sp.lfe_filter.process(&mut sp.bed[..n * ch], ch, lc);
        }
        sp.chain.process(&mut sp.bed[..n * ch]);
        for f in sp.bed[..n * ch].chunks_exact(ch) {
            for (p, v) in sp.peaks.iter_mut().zip(f) {
                *p = p.max((v * master).abs());
            }
        }
    }

    /// Writes the last block's bed to `out` (after the master fader), in
    /// the export's channel order. Returns the peak (on both sides).
    pub(super) fn write_bed(&mut self, n: usize, out: &mut [f32]) -> (f32, f32) {
        let master = self.master;
        let Some(sp) = self.space.as_ref() else {
            return (0.0, 0.0);
        };
        let ch = sp.channels;
        let mut peak = 0.0f32;
        for (o, f) in out[..n * ch]
            .chunks_exact_mut(ch)
            .zip(sp.bed[..n * ch].chunks_exact(ch))
        {
            for (k, v) in o.iter_mut().enumerate() {
                let from = sp.bed_order.as_ref().map_or(k, |ord| ord[k]);
                *v = f[from] * master;
                peak = peak.max(v.abs());
            }
        }
        (peak, peak)
    }

    /// Sets the export's channel order: output channel `k` takes bed
    /// channel `order[k]`. Ignored unless it is a reordering of the bed.
    pub fn set_bed_order(&mut self, order: Option<Vec<usize>>) {
        if let Some(sp) = &mut self.space {
            let ok = order.as_ref().is_none_or(|o| {
                let mut s = o.clone();
                s.sort_unstable();
                s == (0..sp.channels).collect::<Vec<_>>()
            });
            if ok {
                sp.bed_order = order;
            }
        }
    }

    /// A new spot for a track (live).
    pub fn set_space(&mut self, track: usize, pos: SpacePos) {
        if let Some(p) = self
            .tracks
            .get_mut(track)
            .and_then(|t| t.sp.as_deref_mut())
            .and_then(|s| s.place.as_mut())
        {
            p.set_pos(pos);
        }
    }

    /// Clears the 3D state after a jump.
    pub(super) fn space_reset(&mut self) {
        for t in &mut self.tracks {
            if let Some(sp) = t.sp.as_deref_mut() {
                if let Some(p) = &mut sp.place {
                    p.reset();
                }
                if let Some(r) = &mut sp.room {
                    r.chain.reset();
                }
                if let Some(f) = &mut sp.feed {
                    f.chain.reset();
                }
            }
        }
        if let Some(sp) = &mut self.space {
            sp.lfe_filter.reset();
        }
    }

    /// Each speaker's peak since the last call (after the master chain,
    /// with the master fader), in layout order; empty outside 3D.
    pub fn take_bed_peaks(&mut self, out: &mut [f32; MAX_SPEAKERS]) -> usize {
        match &mut self.space {
            Some(sp) => {
                for (o, p) in out.iter_mut().zip(sp.peaks.iter_mut()) {
                    *o = o.max(*p);
                    *p = 0.0;
                }
                sp.channels
            }
            None => 0,
        }
    }

    /// Whether playback puts each speaker on its own device output, given
    /// a device with `outputs` channels (enough for every speaker).
    pub fn speakers_out(&self, outputs: usize) -> bool {
        self.space
            .as_ref()
            .is_some_and(|s| s.speakers && outputs >= s.channels)
    }

    /// Frame `i` of the last block's bed, after the master fader.
    pub fn bed_frame(&self, i: usize, out: &mut [f32]) {
        if let Some(sp) = &self.space {
            let ch = sp.channels;
            for (o, v) in out.iter_mut().zip(&sp.bed[i * ch..(i + 1) * ch]) {
                *o = v * self.master;
            }
        }
    }

    /// Live effect settings for a track: its multichannel clips' and 3D
    /// Bus's chains get them too.
    pub fn space_param(&mut self, track: usize, slot: usize, vals: &[f32]) {
        if let Some(sp) = self.tracks.get_mut(track).and_then(|t| t.sp.as_deref_mut()) {
            if let Some(r) = &mut sp.room {
                r.chain.set(slot, vals);
            }
            if let Some(f) = &mut sp.feed {
                f.chain.set(slot, vals);
            }
        }
    }

    pub fn space_bypass(&mut self, track: usize, slot: usize, on: bool) {
        if let Some(sp) = self.tracks.get_mut(track).and_then(|t| t.sp.as_deref_mut()) {
            if let Some(r) = &mut sp.room {
                r.chain.set_bypass(slot, on);
            }
            if let Some(f) = &mut sp.feed {
                f.chain.set_bypass(slot, on);
            }
        }
    }

    pub fn space_rack(&mut self, track: usize, rack: &af27_core::Rack) {
        if let Some(sp) = self.tracks.get_mut(track).and_then(|t| t.sp.as_deref_mut()) {
            if let Some(r) = &mut sp.room {
                r.chain.set_rack(rack);
            }
            if let Some(f) = &mut sp.feed {
                f.chain.set_rack(rack);
            }
        }
    }

    pub fn space_chain_mix(&mut self, track: usize, mix: f32) {
        if let Some(sp) = self.tracks.get_mut(track).and_then(|t| t.sp.as_deref_mut()) {
            if let Some(r) = &mut sp.room {
                r.chain.mix = mix;
            }
            if let Some(f) = &mut sp.feed {
                f.chain.mix = mix;
            }
        }
    }
}
