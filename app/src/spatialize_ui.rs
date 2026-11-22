// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Spatialize to Surround, in the Waveform editor: a stereo (or mono)
//! file put into a room the way a 3D Space session places a track (its
//! spot, width, Spread, Spatialize and Place Elements, its LFE send), and
//! written back as a file with one channel per speaker of the layout
//! picked. The file gains the channels it needs; Undo takes them away.
//!
//! On top of the spot are the upmixing choices of `af2_space::render::
//! Upmix`: a fold-safe Exact mode, centre divergence and dialogue focus,
//! the LFE's crossover and Copy or Split, how much room sound goes up to
//! the heights, Diffusion, and high shelves behind and above. Preview
//! plays a stretch as the original, as the upmix on headphones, or as the
//! upmix folded back to stereo, switching in place; the room view shows
//! where the sound lands, with a meter per speaker and how faithful the
//! fold is.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use af2_core::space::{Monitor, SpaceLayout, SpacePos, SpatializeMode};
use af2_core::AudioBuffer;
use af2_space::render::{self as upmix, LfeMode, Upmix};
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke};

use crate::state::State;
use crate::theme::{c, ca};
use crate::widgets::Slider;

/// How long Preview plays.
const PREVIEW_SECS: f64 = 12.0;
/// How much of the file the room view's meters listen to.
const ANALYSIS_SECS: f64 = 6.0;
/// The settings rest this long before the meters are worked out again.
const SETTLE: Duration = Duration::from_millis(250);

/// What Preview plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hear {
    /// The file as it is.
    Source,
    /// The upmix, each speaker heard around your head.
    Upmix,
    /// The upmix folded back to stereo.
    Fold,
}

/// The meters' reading of a stretch of the result.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    /// Per speaker, its level, dB.
    pub db: Vec<f32>,
    /// How far the fold is from the original, dB.
    pub fold_db: f32,
}

pub struct SpatializeUi {
    pub open: bool,
    pub layout: SpaceLayout,
    pub pos: SpacePos,
    pub up: Upmix,
    pub hear: Hear,
    /// Width, Spread, Spatialize and Ambient height move together.
    pub link: bool,
    /// Apply once a file is open (`AF2_OPEN=spatialize:<layout>`, for
    /// screenshots).
    pub apply_when_open: bool,
    /// The last preview: its settings, the stretch and its upmix, so
    /// switching what is heard needs no new render.
    preview: Option<(String, AudioBuffer, AudioBuffer)>,
    previewing: bool,
    /// The meters: the settings they were worked out for, when those
    /// settings last changed, and the reading (filled in by a thread).
    analysis_key: String,
    changed_at: Option<Instant>,
    analysis: Arc<Mutex<Option<(String, Analysis)>>>,
}

impl Default for SpatializeUi {
    fn default() -> Self {
        Self {
            open: false,
            layout: SpaceLayout::Surround51,
            pos: start_pos(),
            up: Upmix::default(),
            hear: Hear::Upmix,
            link: false,
            apply_when_open: false,
            preview: None,
            previewing: false,
            analysis_key: String::new(),
            changed_at: None,
            analysis: Arc::new(Mutex::new(None)),
        }
    }
}

/// Where a file starts: front centre, the pair as wide as the front
/// speakers, opened out into the room by Place Elements.
pub fn start_pos() -> SpacePos {
    SpacePos {
        spatialize: 0.75,
        spatialize_mode: SpatializeMode::Steer,
        ..SpacePos::default()
    }
}

/// The layouts a file can be made into (2.0 adds nothing).
pub fn layouts() -> impl Iterator<Item = SpaceLayout> {
    SpaceLayout::ALL
        .into_iter()
        .filter(|l| *l != SpaceLayout::Stereo20)
}

/// The file's channels put into the room.
pub fn render(buf: &AudioBuffer, layout: SpaceLayout, pos: &SpacePos, up: &Upmix) -> AudioBuffer {
    AudioBuffer {
        rate: buf.rate,
        channels: upmix::spatialize_with(layout, pos, up, buf.rate, &buf.channels),
    }
}

/// Why the open file cannot be spatialized, if it cannot.
fn refuse(st: &State) -> Option<String> {
    let Some(d) = st.doc() else {
        return Some("Open a file in the Waveform editor first.".into());
    };
    let n = d.buf.num_channels();
    (n > 2).then(|| format!("This file already has {n} channels: Spatialize takes a mono or stereo file."))
}

/// The stretch Preview and the meters listen to: from the selection or
/// the playhead, `secs` long (moved back if the file ends sooner).
fn stretch(st: &State, secs: f64) -> Option<AudioBuffer> {
    let d = st.doc()?;
    let rate = d.buf.rate as f64;
    let from = match d.sel {
        Some((a, b)) if b - a > 1e-6 => a,
        _ => d.cursor,
    };
    let len = (secs * rate) as usize;
    let a = ((from * rate) as usize).min(d.buf.frames());
    let b = (a + len).min(d.buf.frames());
    let a = if b - a < rate as usize / 2 { b.saturating_sub(len) } else { a };
    (b > a).then(|| d.buf.slice(af2_core::Range::new(a, b)))
}

/// A key for "the same settings on the same stretch".
fn key(st: &State) -> String {
    let sp = &st.spatialize;
    let at = st.doc().map(|d| (d.buf.frames(), d.cursor, d.sel, d.buf.num_channels()));
    format!("{:?}|{:?}|{:?}|{:?}", sp.layout, sp.pos, sp.up, at)
}

pub fn show(ctx: &egui::Context, st: &mut State) {
    if st.spatialize.apply_when_open && st.doc().is_some() {
        st.spatialize.apply_when_open = false;
        apply(st);
    }
    if !st.spatialize.open {
        return;
    }
    keep_meters(ctx, st);
    let th = st.theme.clone();
    let why = refuse(st);
    let src_ch = st.doc().map_or(2, |d| d.buf.num_channels());
    let stereo = src_ch >= 2;
    let reading = st.spatialize.analysis.lock().ok().and_then(|a| a.as_ref().map(|(_, r)| r.clone()));
    let mut open = true;
    let (mut apply, mut preview) = (false, false);
    let hear_before = st.spatialize.hear;
    egui::Window::new("Spatialize to Surround")
        .open(&mut open)
        .resizable(false)
        .default_width(340.0)
        .show(ctx, |ui| {
            let sp = &mut st.spatialize;
            ui.horizontal(|ui| {
                ui.label("Layout");
                egui::ComboBox::from_id_salt("spatialize-layout")
                    .selected_text(sp.layout.label())
                    .show_ui(ui, |ui| {
                        for l in layouts() {
                            let n = af2_space::speakers(l).len();
                            ui.selectable_value(&mut sp.layout, l, format!("{}  ({n} channels)", l.label()));
                        }
                    })
                    .response
                    .on_hover_text("The speakers the file is written for: it gets one channel for each");
                ui.checkbox(&mut sp.up.exact, "Exact")
                    .on_hover_text(
                        "Folding the result back to stereo gives the original exactly: whatever the \
                         upmix changes in the fold is made up in the front pair",
                    );
            });
            ui.add_space(4.0);
            room(ui, &th, sp.layout, &mut sp.pos, stereo, reading.as_ref());
            meters(ui, &th, sp.layout, reading.as_ref(), sp.up.exact);
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                controls(ui, sp, stereo);
            });
            ui.add_space(6.0);
            let n = af2_space::speakers(sp.layout).len();
            let note = match &why {
                Some(w) => RichText::new(w.as_str()).color(c(th.meter_high)),
                None => RichText::new(format!(
                    "The whole file becomes {n} channels ({}). Undo puts it back.",
                    sp.layout.label()
                ))
                .color(c(th.text_dim)),
            };
            ui.label(note.size(11.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                preview |= ui
                    .add_enabled(why.is_none(), egui::Button::new("Preview"))
                    .on_hover_text(format!(
                        "Plays up to {PREVIEW_SECS:.0} seconds from the selection or the playhead"
                    ))
                    .clicked();
                ui.selectable_value(&mut sp.hear, Hear::Source, "Source")
                    .on_hover_text("The file as it is now");
                ui.selectable_value(&mut sp.hear, Hear::Upmix, "Upmix")
                    .on_hover_text("The result, each speaker heard around your head (headphones)");
                ui.selectable_value(&mut sp.hear, Hear::Fold, "Fold")
                    .on_hover_text("The result folded back to stereo, as a stereo player would play it");
                if ui
                    .button("Reset")
                    .on_hover_text("Front centre, Place Elements at 75%, every upmix choice off")
                    .clicked()
                {
                    sp.pos = start_pos();
                    sp.up = Upmix::default();
                }
            });
            ui.add_space(2.0);
            let go = egui::Button::new(RichText::new("Apply").strong().color(c(th.background)))
                .fill(c(th.accent));
            apply = ui.add_enabled(why.is_none(), go).clicked();
        });
    // Switching what is heard during a preview carries on from the same
    // place.
    let playing = st.engine.as_ref().is_some_and(|e| e.is_playing());
    if !playing {
        st.spatialize.previewing = false;
    }
    if preview {
        play_preview(st, 0.0);
    } else if st.spatialize.hear != hear_before && st.spatialize.previewing {
        let at = st.engine.as_ref().map_or(0.0, |e| e.position());
        play_preview(st, at);
    }
    if apply {
        self::apply(st);
        open = false;
    }
    if !open {
        st.spatialize.preview = None;
    }
    st.spatialize.open = open;
}

/// Starts the meters' reading again once the settings have rested.
fn keep_meters(ctx: &egui::Context, st: &mut State) {
    let k = key(st);
    let sp = &mut st.spatialize;
    if k != sp.analysis_key {
        sp.analysis_key = k.clone();
        sp.changed_at = Some(Instant::now());
    }
    let Some(t) = sp.changed_at else { return };
    if t.elapsed() < SETTLE {
        ctx.request_repaint_after(SETTLE);
        return;
    }
    sp.changed_at = None;
    if refuse(st).is_some() {
        return;
    }
    let Some(seg) = stretch(st, ANALYSIS_SECS) else { return };
    let sp = &st.spatialize;
    let (layout, pos, up, slot) = (sp.layout, sp.pos, sp.up, sp.analysis.clone());
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let bed = render(&seg, layout, &pos, &up);
        let db = bed
            .channels
            .iter()
            .map(|ch| {
                let ms = ch.iter().map(|v| v * v).sum::<f32>() / ch.len().max(1) as f32;
                (10.0 * ms.max(1e-12).log10()).max(-90.0)
            })
            .collect();
        let fold_db = upmix::fold_error_db(layout, &bed.channels, &seg.channels);
        if let Ok(mut a) = slot.lock() {
            *a = Some((k, Analysis { db, fold_db }));
        }
        ctx.request_repaint();
    });
}

/// Puts the open file into the room with the window's settings.
pub fn apply(st: &mut State) {
    if refuse(st).is_some() {
        return;
    }
    let (layout, pos, up) = (st.spatialize.layout, st.spatialize.pos, st.spatialize.up);
    if let Some(d) = st.doc_mut() {
        let out = render(&d.buf, layout, &pos, &up);
        d.set_buffer(&format!("Spatialize to {}", layout.label()), out);
        d.channel_mask = Some(af2_space::pan::channel_mask(layout));
        // Every channel of the new layout heard and edited.
        d.chans = crate::wave_channels::ChannelSet::all(d.buf.num_channels());
    }
}

/// Plays a stretch as the original, the upmix on headphones, or the
/// upmix folded to stereo, from `at` seconds into it.
fn play_preview(st: &mut State, at: f64) {
    let k = key(st);
    let fresh = st.spatialize.preview.as_ref().is_some_and(|(pk, _, _)| *pk == k);
    if !fresh {
        let Some(seg) = stretch(st, PREVIEW_SECS) else { return };
        let sp = &st.spatialize;
        let bed = render(&seg, sp.layout, &sp.pos, &sp.up);
        st.spatialize.preview = Some((k, seg, bed));
    }
    let Some((_, seg, bed)) = &st.spatialize.preview else { return };
    let (layout, rate) = (st.spatialize.layout, seg.rate);
    let heard = match st.spatialize.hear {
        Hear::Source => {
            if seg.num_channels() == 1 {
                AudioBuffer { rate, channels: vec![seg.channels[0].clone(), seg.channels[0].clone()] }
            } else {
                seg.clone()
            }
        }
        Hear::Fold => {
            let (l, r) = upmix::downmix(layout, &bed.channels);
            AudioBuffer { rate, channels: vec![l, r] }
        }
        Hear::Upmix => {
            let mut m = af2_space::Monitoring::new(layout, Monitor::Binaural, rate);
            let inter = bed.to_interleaved();
            let mut out = vec![0.0f32; bed.frames() * 2];
            m.process(&inter, &mut out);
            AudioBuffer::from_interleaved(rate, 2, &out)
        }
    };
    let len = heard.duration_secs();
    if let Some(eng) = &mut st.engine {
        eng.set_program(af2_engine::buffer_program(Arc::new(heard)));
        eng.play(at.clamp(0.0, len), None, false, len);
        st.spatialize.previewing = true;
    }
    // The file's own program again on the next play.
    st.session_version += 1;
}

/// The room from above, front at the top: the layout's speakers (each
/// glowing with how much it plays), and the file's spot (with its left
/// and right sides) to drag.
fn room(
    ui: &mut egui::Ui,
    th: &crate::theme::Theme,
    layout: SpaceLayout,
    pos: &mut SpacePos,
    stereo: bool,
    reading: Option<&Analysis>,
) {
    let side = ui.available_width().min(300.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side * 0.78), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    let box_ = rect.shrink(18.0);
    let to = |x: f32, y: f32| pos2(box_.center().x + x * box_.width() / 2.0, box_.center().y - y * box_.height() / 2.0);
    p.rect_filled(rect, 4.0, c(th.wave_background));
    // Where the sound lands: a glow per speaker, by its level.
    let sp = af2_space::speakers(layout);
    if let Some(r) = reading.filter(|r| r.db.len() == sp.len()) {
        let top = r.db.iter().copied().fold(-90.0f32, f32::max);
        for (s, db) in sp.iter().zip(&r.db) {
            let Some([x, y, z]) = s.at else { continue };
            let k = ((db - top + 30.0) / 30.0).clamp(0.0, 1.0);
            if k <= 0.0 {
                continue;
            }
            let at = to(x, y);
            let col = if z > 0.5 { th.accent_alt } else { th.accent };
            let reach = 10.0 + 34.0 * k;
            for (i, a) in [(1.0, 18.0), (0.66, 30.0), (0.36, 46.0)] {
                p.circle_filled(at, reach * i, ca(col, (a * k) as u8));
            }
        }
    }
    p.rect_stroke(box_, 2.0, Stroke::new(1.0_f32, ca(th.text_dim, 90)));
    p.text(pos2(rect.center().x, rect.top() + 3.0), Align2::CENTER_TOP, "Front", FontId::proportional(9.5), ca(th.text_dim, 160));
    // The listener.
    p.circle_stroke(box_.center(), 6.0, Stroke::new(1.0_f32, ca(th.text_dim, 140)));
    for s in sp {
        let Some([x, y, z]) = s.at else { continue };
        let at = to(x, y);
        let top = z > 0.5;
        let r = Rect::from_center_size(at, vec2(9.0, 9.0));
        if top {
            p.rect_stroke(r.shrink(1.0), 1.0, Stroke::new(1.2_f32, ca(th.accent_alt, 200)));
        } else {
            p.rect_filled(r, 1.0, ca(th.text, 170));
        }
        let off = if top { vec2(0.0, -11.0) } else { vec2(0.0, 11.0) };
        p.text(at + off, Align2::CENTER_CENTER, s.name, FontId::proportional(8.5), ca(th.text_dim, 200));
    }
    // Dragging moves the spot.
    if let Some(pp) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            pos.x = (((pp.x - box_.center().x) / (box_.width() / 2.0)) as f64).clamp(-1.0, 1.0);
            pos.y = (((box_.center().y - pp.y) / (box_.height() / 2.0)) as f64).clamp(-1.0, 1.0);
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    let spot = to(pos.x as f32, pos.y as f32);
    let col = c(th.accent);
    if pos.size > 0.0 {
        p.circle_filled(spot, 6.0 + pos.size as f32 * box_.width() * 0.45, ca(th.accent, 28));
    }
    if stereo {
        let [l, r] = af2_space::pan::sides(pos);
        let (lp, rp) = (to(l[0], l[1]), to(r[0], r[1]));
        p.line_segment([lp, rp], Stroke::new(1.0_f32, ca(th.accent, 140)));
        dot(&p, lp, "L", col, th);
        dot(&p, rp, "R", col, th);
    }
    let d = 6.0;
    p.add(egui::Shape::convex_polygon(
        vec![spot + vec2(0.0, -d), spot + vec2(d, 0.0), spot + vec2(0.0, d), spot + vec2(-d, 0.0)],
        col,
        Stroke::NONE,
    ));
}

fn dot(p: &egui::Painter, at: Pos2, label: &str, col: Color32, th: &crate::theme::Theme) {
    p.circle_filled(at, 7.0, col);
    p.text(at, Align2::CENTER_CENTER, label, FontId::proportional(9.0), c(th.background));
}

/// A meter per speaker (its level over a few seconds from the playhead)
/// and how faithful the stereo fold is.
fn meters(ui: &mut egui::Ui, th: &crate::theme::Theme, layout: SpaceLayout, reading: Option<&Analysis>, exact: bool) {
    let sp = af2_space::speakers(layout);
    let w = ui.available_width().min(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
    let p = ui.painter_at(rect);
    let n = sp.len().max(1);
    let slot = rect.width() / n as f32;
    for (i, s) in sp.iter().enumerate() {
        let x0 = rect.left() + i as f32 * slot;
        let bar = Rect::from_min_max(pos2(x0 + slot * 0.2, rect.top()), pos2(x0 + slot * 0.8, rect.bottom() - 12.0));
        p.rect_filled(bar, 1.0, ca(th.text_dim, 40));
        if let Some(db) = reading.and_then(|r| r.db.get(i)) {
            let k = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            let lit = Rect::from_min_max(pos2(bar.left(), bar.bottom() - bar.height() * k), bar.max);
            let col = if *db > -1.0 { th.meter_high } else { th.accent };
            p.rect_filled(lit, 1.0, c(col));
        }
        p.text(pos2(x0 + slot / 2.0, rect.bottom()), Align2::CENTER_BOTTOM, s.name, FontId::proportional(8.0), ca(th.text_dim, 200));
    }
    let (text, col) = match reading {
        None => ("Working out the levels…".to_string(), th.text_dim),
        Some(r) if exact || r.fold_db < -40.0 => ("Folds back to the original stereo".to_string(), th.text_dim),
        Some(r) if r.fold_db < -12.0 => (
            format!("The stereo fold differs a little from the original ({:.0} dB)", r.fold_db),
            th.text_dim,
        ),
        Some(r) => (
            format!("The stereo fold differs from the original ({:.0} dB): Exact keeps it", r.fold_db),
            th.meter_high,
        ),
    };
    ui.label(RichText::new(text).size(10.5).color(c(col)))
        .on_hover_text("Measured over a few seconds from the selection or the playhead");
}

/// Every setting below the room.
fn controls(ui: &mut egui::Ui, sp: &mut SpatializeUi, stereo: bool) {
    let layout = sp.layout;
    let before = (sp.pos.width, sp.pos.spread, sp.pos.spatialize, sp.up.ambient_height as f64);
    egui::CollapsingHeader::new("Spot").default_open(true).show(ui, |ui| {
        spot_controls(ui, &mut sp.pos, stereo, layout);
        if stereo {
            ui.checkbox(&mut sp.link, "Link")
                .on_hover_text("Width, Spread, Spatialize and Ambient height move together, each from where it is");
        }
        if stereo && sp.pos.spatialize_mode == SpatializeMode::Steer {
            ui.add(Slider::new(&mut sp.up.diffusion, 0.0..=1.0).text("Diffusion"))
                .on_hover_text("Place Elements: 0 places each part of the sound precisely; higher is smoother and softer-edged");
        }
    });
    if upmix::center_channel(layout).is_some() {
        egui::CollapsingHeader::new("Centre").default_open(false).show(ui, |ui| {
            if stereo {
                ui.add(Slider::new(&mut sp.up.dialogue, 0.0..=1.0).text("Dialogue focus"))
                    .on_hover_text("What the recording has dead centre (a voice, most often) goes to the centre speaker on its own");
            }
            ui.add(Slider::new(&mut sp.up.divergence, 0.0..=1.0).text("Divergence"))
                .on_hover_text("0 keeps the centre speaker's sound in it; 1 moves it to the front pair as a phantom centre");
            ui.checkbox(&mut sp.up.center_lows, "Centre lows to L and R")
                .on_hover_text("The centre speaker's lows, below the crossover, play from the front pair");
        });
    }
    let has_lfe = af2_space::lfe_channel(layout).is_some();
    egui::CollapsingHeader::new("Low frequencies").default_open(false).show(ui, |ui| {
        ui.add(Slider::new(&mut sp.up.lfe_hz, 40.0..=200.0).suffix(" Hz").text("Crossover"))
            .on_hover_text("The LFE's low pass, the Split point and the centre lows' split");
        if has_lfe {
            ui.horizontal(|ui| {
                ui.label("LFE");
                for m in LfeMode::ALL {
                    ui.selectable_value(&mut sp.up.lfe_mode, m, m.label()).on_hover_text(m.tip());
                }
            });
            lfe_send(ui, &mut sp.pos, sp.up.lfe_hz);
        }
    });
    if upmix::has_heights(layout) {
        egui::CollapsingHeader::new("Height").default_open(false).show(ui, |ui| {
            let mut height = sp.pos.z * 100.0;
            if ui
                .add(Slider::new(&mut height, -100.0..=100.0).suffix("%").text("Direct height"))
                .on_hover_text("Raises the placed sound: ear level 0, ceiling 100%")
                .changed()
            {
                sp.pos.z = height / 100.0;
            }
            ui.add(Slider::new(&mut sp.up.ambient_height, 0.0..=1.0).text("Ambient height"))
                .on_hover_text("How much of the room sound (Spread, Spatialize, Place Elements' ambience) goes up to the height speakers");
            ui.checkbox(&mut sp.up.keep_center_low, "Keep the centre out of the heights")
                .on_hover_text("The centre speaker's room sound stays at ear level (the LFE never goes up)");
        });
    }
    egui::CollapsingHeader::new("Tone").default_open(false).show(ui, |ui| {
        ui.add(Slider::new(&mut sp.up.rear_hf_db, -12.0..=0.0).suffix(" dB").text("Rear highs"))
            .on_hover_text("A high shelf (above about 4 kHz) on the speakers behind: softer rears draw less attention");
        if upmix::has_heights(layout) {
            ui.add(Slider::new(&mut sp.up.top_hf_db, -12.0..=0.0).suffix(" dB").text("Height highs"))
                .on_hover_text("A high shelf (above about 4 kHz) on the height speakers");
        }
    });
    if sp.link && stereo {
        let after = (sp.pos.width, sp.pos.spread, sp.pos.spatialize, sp.up.ambient_height as f64);
        let deltas = [after.0 - before.0, after.1 - before.1, after.2 - before.2, after.3 - before.3];
        if let Some(d) = deltas.iter().copied().find(|d| d.abs() > 1e-9) {
            let moved = |v: f64, own: f64| if own.abs() > 1e-9 { v } else { (v + d).clamp(0.0, 1.0) };
            sp.pos.width = moved(after.0, deltas[0]);
            sp.pos.spread = moved(after.1, deltas[1]);
            sp.pos.spatialize = moved(after.2, deltas[2]);
            sp.up.ambient_height = moved(after.3, deltas[3]) as f32;
        }
    }
}

/// The spot's settings, as the 3D Space panel has them.
fn spot_controls(ui: &mut egui::Ui, p: &mut SpacePos, stereo: bool, layout: SpaceLayout) {
    if !upmix::has_heights(layout) {
        let mut height = p.z * 100.0;
        if ui
            .add(Slider::new(&mut height, -100.0..=100.0).suffix("%").text("Height"))
            .on_hover_text("Ear level 0, ceiling 100%, below ear level down to -100%")
            .changed()
        {
            p.z = height / 100.0;
        }
    }
    if stereo {
        ui.add(Slider::new(&mut p.width, 0.0..=1.0).text("Width"))
            .on_hover_text("How far each side sits from the spot");
        ui.add(Slider::new(&mut p.rotation, -180.0..=180.0).suffix("°").text("Rotation"))
            .on_hover_text("Turns the pair, clockwise seen from above (90 puts the left side in front)");
        ui.add(Slider::new(&mut p.spatialize, 0.0..=1.0).text("Spatialize"))
            .on_hover_text("How much of the pair opens out into the room");
        let mode = p.spatialize_mode;
        egui::ComboBox::from_label("Method")
            .selected_text(mode.label())
            .show_ui(ui, |ui| {
                for m in SpatializeMode::ALL {
                    ui.selectable_value(&mut p.spatialize_mode, m, m.label())
                        .on_hover_text(m.tip());
                }
            });
    }
    ui.add(Slider::new(&mut p.size, 0.0..=1.0).text("Size"))
        .on_hover_text("0 is a point; 1 fills the room");
    ui.add(Slider::new(&mut p.spread, 0.0..=1.0).text("Spread"))
        .on_hover_text("How much is spread through the room as a diffuse field");
}

fn lfe_send(ui: &mut egui::Ui, p: &mut SpacePos, hz: f32) {
    let mut db = if p.lfe > 0.0 { (20.0 * p.lfe.log10()).max(-60.0) } else { -60.0 };
    if ui
        .add(Slider::new(&mut db, -60.0..=0.0).suffix(" dB").text("LFE send"))
        .on_hover_text(format!("Also copies the sound into the LFE (low-passed at {hz:.0} Hz). Far left is off"))
        .changed()
    {
        p.lfe = if db <= -59.95 { 0.0 } else { 10f64.powf(db / 20.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stereo_file_gains_the_layouts_channels() {
        let n = 9600;
        let buf = AudioBuffer {
            rate: 48_000,
            channels: vec![vec![0.25; n], vec![-0.25; n]],
        };
        for l in layouts() {
            let out = render(&buf, l, &start_pos(), &Upmix::default());
            assert_eq!(out.num_channels(), af2_space::speakers(l).len(), "{}", l.label());
            assert_eq!(out.frames(), n);
            assert_eq!(out.rate, 48_000);
        }
        assert!(!layouts().any(|l| l == SpaceLayout::Stereo20));
    }

    #[test]
    fn the_new_file_names_its_channels_by_its_layout() {
        // 5.1.2 and 7.1 both have eight channels; the mask tells them apart.
        let m512 = af2_space::pan::channel_mask(SpaceLayout::Surround512);
        let names: Vec<&str> = (0..8).map(|k| crate::waveform::channel_name_in(8, Some(m512), k)).collect();
        assert_eq!(names, ["L", "R", "C", "LFE", "Ls", "Rs", "Ltf", "Rtf"]);
        let by_count: Vec<&str> = (0..8).map(|k| crate::waveform::channel_name(8, k)).collect();
        assert_eq!(by_count, ["L", "R", "C", "LFE", "Lb", "Rb", "Ls", "Rs"]);
    }
}
