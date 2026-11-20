// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Tube Master's window: a hardware-style panel with two channel rows of
//! large stepped knobs, a lit needle meter per row (input level or gain
//! reduction), toggle switches, a bottom row of smaller knobs and a
//! loudness and peak bar. Everything that never moves (the panel, tick
//! rings, labels and meter faces) is drawn once and kept; while the
//! meters move it repaints, otherwise it waits.
//!
//! Values go through the same live path as the plain controls: the
//! slot's values change, `live_slot` sends them, one undo step per
//! gesture.

use std::sync::{Arc, OnceLock};

use egui::{pos2, vec2, Color32, FontId, Galley, Pos2, Rect, Rounding, Sense, Shape, Stroke};

use af27_fx::effects::tube_master::{self as tm, ROW_KEYS};

use crate::rack::{chain_of, FILE};
use crate::state::State;
use crate::theme::{c, ca, lerp, Theme};

const W: f32 = 820.0;
const H: f32 = 486.0;
const ROW_Y: [f32; 2] = [40.0, 172.0];
const ROW_H: f32 = 126.0;
const BIG_R: f32 = 23.0;
const SMALL_R: f32 = 17.0;

/// The keys of every control, in the slot's value order.
fn keys() -> &'static [&'static str] {
    static K: OnceLock<Vec<&'static str>> = OnceLock::new();
    K.get_or_init(|| tm::info().params.iter().map(|p| p.key).collect())
}

fn defaults() -> &'static [f32] {
    static D: OnceLock<Vec<f32>> = OnceLock::new();
    D.get_or_init(|| tm::info().params.iter().map(|p| p.default as f32).collect())
}

fn at(key: &str) -> usize {
    keys().iter().position(|k| *k == key).unwrap_or(0)
}

// ------------------------------------------------------------ geometry

/// A big knob's centre in channel row `r`, column `k` (relative).
fn big_centre(r: usize, k: usize) -> Pos2 {
    pos2(84.0 + k as f32 * 90.0, ROW_Y[r] + 64.0)
}

fn row_switch(r: usize, k: usize) -> Rect {
    Rect::from_min_size(
        pos2(592.0, ROW_Y[r] + 22.0 + k as f32 * 36.0),
        vec2(36.0, 14.0),
    )
}

fn vu_rect(r: usize) -> Rect {
    Rect::from_min_size(pos2(670.0, ROW_Y[r] + 8.0), vec2(122.0, 110.0))
}

const MID_Y: f32 = 306.0;

fn mid_switch(k: usize) -> Rect {
    Rect::from_min_size(
        pos2(110.0 + k as f32 * 170.0, MID_Y + 9.0),
        vec2(48.0, 16.0),
    )
}

const BOT_Y: f32 = 350.0;

fn small_centre(k: usize) -> Pos2 {
    pos2(52.0 + k as f32 * 84.0, BOT_Y + 58.0)
}

fn sat_switch() -> Rect {
    Rect::from_min_size(pos2(452.0, BOT_Y + 40.0), vec2(48.0, 16.0))
}

fn loud_rect() -> Rect {
    Rect::from_min_size(pos2(538.0, BOT_Y + 12.0), vec2(254.0, 100.0))
}

// ------------------------------------------------------------- the knobs

/// What a big stepped knob shows: its title, the label at each step
/// (empty for a plain tick), and its readout.
struct Big {
    slot: usize,
    title: &'static str,
    steps: usize,
    label: fn(usize) -> &'static str,
    read: fn(usize) -> String,
}

fn bigs() -> [Big; 6] {
    [
        Big {
            slot: 0,
            title: "INPUT",
            steps: 5,
            label: |s| ["1", "2", "3", "4", "5"][s],
            read: |s| format!("+{:.0} dB", tm::INPUT_DB[s]),
        },
        Big {
            slot: 1,
            title: "THRESHOLD",
            steps: 11,
            label: |s| ["-30", "", "-24", "", "-18", "", "-12", "", "-6", "", "0"][s],
            read: |s| format!("{:.0} dB", tm::THRESHOLD_DB[s]),
        },
        Big {
            slot: 2,
            title: "ATTACK",
            steps: 5,
            label: |s| ["SLOW", "", "MED", "", "FAST"][s],
            read: |s| format!("{:.0} ms", tm::ATTACK_MS[s]),
        },
        Big {
            slot: 3,
            title: "RELEASE",
            steps: 5,
            label: |s| ["SLOW", "", "MED", "", "FAST"][s],
            read: |s| format!("{:.2} s", tm::RELEASE_S[s]),
        },
        Big {
            slot: 4,
            title: "RATIO",
            steps: 9,
            label: |s| ["1.5", "2", "3", "4", "6", "10", "20", "LIM", "NEG"][s],
            read: |s| {
                [
                    "1.5:1", "2:1", "3:1", "4:1", "6:1", "10:1", "20:1", "Limit", "Negative",
                ][s]
                    .to_string()
            },
        },
        Big {
            slot: 5,
            title: "OUTPUT",
            steps: 25,
            label: |s| match s {
                0 => "-12",
                6 => "-6",
                12 => "0",
                18 => "+6",
                24 => "+12",
                _ => "",
            },
            read: |s| format!("{:+} dB", s as i32 - 12),
        },
    ]
}

/// A small continuous knob in the bottom row.
struct Small {
    key: &'static str,
    title: &'static str,
    min: f32,
    max: f32,
    step: f32,
    ticks: &'static [(f32, &'static str)],
    read: fn(f32) -> String,
}

fn smalls() -> [Small; 5] {
    [
        Small {
            key: "mix",
            title: "MIX",
            min: 0.0,
            max: 100.0,
            step: 1.0,
            ticks: &[(0.0, "DRY"), (50.0, "50"), (100.0, "WET")],
            read: |v| format!("{v:.0} %"),
        },
        Small {
            key: "headroom",
            title: "HEADROOM",
            min: 4.0,
            max: 24.0,
            step: 1.0,
            ticks: &[
                (4.0, "4"),
                (8.0, ""),
                (12.0, "12"),
                (16.0, ""),
                (20.0, ""),
                (24.0, "24"),
            ],
            read: |v| format!("{v:.0} dB"),
        },
        Small {
            key: "fold",
            title: "MONO FOLD",
            min: 0.0,
            max: 200.0,
            step: 5.0,
            ticks: &[
                (0.0, "OFF"),
                (50.0, ""),
                (100.0, "100"),
                (150.0, ""),
                (200.0, "200"),
            ],
            read: |v| {
                if v < 10.0 {
                    "Off".into()
                } else {
                    format!("{v:.0} Hz")
                }
            },
        },
        Small {
            key: "gain",
            title: "GAIN",
            min: -12.0,
            max: 12.0,
            step: 0.1,
            ticks: &[
                (-12.0, "-12"),
                (-6.0, ""),
                (0.0, "0"),
                (6.0, ""),
                (12.0, "+12"),
            ],
            read: |v| format!("{v:+.1} dB"),
        },
        Small {
            key: "shape",
            title: "SHAPE",
            min: 0.0,
            max: 100.0,
            step: 1.0,
            ticks: &[(0.0, "0"), (50.0, ""), (100.0, "100")],
            read: |v| format!("{v:.0} %"),
        },
    ]
}

/// The angle (radians clockwise from straight up) for a knob at `t` (0..1).
fn angle(t: f32) -> f32 {
    (t.clamp(0.0, 1.0) - 0.5) * 1.5 * std::f32::consts::PI
}

fn polar(c: Pos2, r: f32, a: f32) -> Pos2 {
    c + vec2(a.sin(), -a.cos()) * r
}

// ------------------------------------------------------------ the meter

/// The needle meter's scale: -20 to +3, a VU's spread.
fn vu_t(db: f32) -> f32 {
    let lo = 10f32.powf(-20.0 / 20.0);
    let hi = 10f32.powf(3.0 / 20.0);
    ((10f32.powf(db.clamp(-40.0, 3.0) / 20.0) - lo) / (hi - lo)).clamp(-0.03, 1.0)
}

fn vu_pivot(r: Rect) -> (Pos2, f32) {
    let face = vu_face(r);
    (pos2(face.center().x, face.bottom() + 4.0), 86.0)
}

fn vu_face(r: Rect) -> Rect {
    r.shrink(7.0)
}

fn vu_angle(t: f32) -> f32 {
    (t - 0.5) * 1.4
}

// ------------------------------------------------------------ painting

fn text(ui: &egui::Ui, s: &str, size: f32, color: Color32) -> Arc<Galley> {
    ui.fonts(|f| f.layout_no_wrap(s.to_string(), FontId::proportional(size), color))
}

fn centred(out: &mut Vec<Shape>, g: Arc<Galley>, at: Pos2) {
    let p = at - g.size() / 2.0;
    out.push(Shape::galley(p, g, Color32::WHITE));
}

fn screw(out: &mut Vec<Shape>, at: Pos2, th: &Theme) {
    out.push(Shape::circle_filled(
        at,
        5.0,
        lerp(th.panel_raised, th.text, 0.25),
    ));
    out.push(Shape::circle_stroke(at, 5.0, sk(1.0, c(th.background))));
    out.push(Shape::line_segment(
        [at + vec2(-3.0, 2.0), at + vec2(3.0, -2.0)],
        sk(1.2, c(th.background)),
    ));
}

/// Everything that does not move, relative to the panel's corner.
fn static_layer(ui: &egui::Ui, th: &Theme) -> Vec<Shape> {
    let mut out = Vec::new();
    let full = Rect::from_min_size(Pos2::ZERO, vec2(W, H));
    let dim = c(th.text_dim);
    let txt = c(th.text);
    out.push(Shape::rect_filled(
        full,
        Rounding::same(8.0),
        lerp(th.panel, th.background, 0.3),
    ));
    // Brushed bands across the face.
    for i in 0..24 {
        let y = i as f32 * (H / 24.0);
        let a = if i % 2 == 0 { 10 } else { 4 };
        out.push(Shape::rect_filled(
            Rect::from_min_size(pos2(6.0, y), vec2(W - 12.0, H / 24.0)),
            Rounding::ZERO,
            ca(th.text, a),
        ));
    }
    out.push(Shape::rect_stroke(
        full.shrink(1.0),
        Rounding::same(8.0),
        sk(2.0, c(th.border)),
    ));
    for p in [
        pos2(14.0, 14.0),
        pos2(W - 14.0, 14.0),
        pos2(14.0, H - 14.0),
        pos2(W - 14.0, H - 14.0),
    ] {
        screw(&mut out, p, th);
    }
    // The name plate.
    let g = text(ui, "TUBE  MASTER", 18.0, txt);
    out.push(Shape::galley(pos2(30.0, 10.0), g.clone(), txt));
    let sub = text(ui, "stereo valve mastering compressor", 11.0, dim);
    out.push(Shape::galley(pos2(36.0 + g.size().x, 16.0), sub, dim));
    // Channel row plates.
    for (r, &row_y) in ROW_Y.iter().enumerate() {
        let plate = Rect::from_min_size(pos2(24.0, row_y), vec2(W - 48.0, ROW_H));
        out.push(Shape::rect_filled(
            plate,
            Rounding::same(6.0),
            ca(th.panel_raised, 200),
        ));
        out.push(Shape::rect_stroke(
            plate,
            Rounding::same(6.0),
            sk(1.0, ca(th.border, 160)),
        ));
        for (k, b) in bigs().iter().enumerate() {
            let cen = big_centre(r, k);
            centred(
                &mut out,
                text(ui, b.title, 10.0, dim),
                cen + vec2(0.0, -58.0),
            );
            for s in 0..b.steps {
                let a = angle(s as f32 / (b.steps - 1) as f32);
                let lab = (b.label)(s);
                let (r0, r1) = if lab.is_empty() {
                    (BIG_R + 4.0, BIG_R + 7.0)
                } else {
                    (BIG_R + 3.0, BIG_R + 8.0)
                };
                out.push(Shape::line_segment(
                    [polar(cen, r0, a), polar(cen, r1, a)],
                    sk(if lab.is_empty() { 1.0 } else { 1.6 }, dim),
                ));
                if !lab.is_empty() {
                    let size = if lab.len() > 3 { 8.0 } else { 9.0 };
                    let rr = BIG_R + 16.0 + if lab.len() > 2 { 3.0 } else { 0.0 };
                    centred(&mut out, text(ui, lab, size, dim), polar(cen, rr, a));
                }
            }
            // The knob's skirt.
            out.push(Shape::circle_filled(
                cen + vec2(0.0, 2.0),
                BIG_R + 1.0,
                ca(th.background, 160),
            ));
            out.push(Shape::circle_filled(
                cen,
                BIG_R,
                lerp(th.panel_raised, th.background, 0.55),
            ));
            out.push(Shape::circle_stroke(
                cen,
                BIG_R,
                sk(1.5, lerp(th.border, th.text, 0.25)),
            ));
            out.push(Shape::circle_filled(
                cen,
                BIG_R - 6.0,
                lerp(th.panel_raised, th.text, 0.10),
            ));
        }
        for (k, name) in ["CHANNEL IN", "SC FILTER", "VU MODE"].iter().enumerate() {
            let s = row_switch(r, k);
            let g = text(ui, name, 8.5, dim);
            out.push(Shape::galley(
                pos2(s.left(), s.top() - g.size().y - 1.0),
                g,
                dim,
            ));
        }
        // The meter's face.
        let vr = vu_rect(r);
        out.push(Shape::rect_filled(
            vr,
            Rounding::same(5.0),
            c(th.background),
        ));
        out.push(Shape::rect_stroke(
            vr,
            Rounding::same(5.0),
            sk(1.5, lerp(th.border, th.text, 0.2)),
        ));
        let face = vu_face(vr);
        let warm = lerp(th.marker, [255, 246, 222], 0.62);
        out.push(Shape::rect_filled(face, Rounding::same(3.0), warm));
        // A soft glow from below, as if lit from behind.
        for i in 0..5 {
            let f = i as f32 / 5.0;
            let gr = Rect::from_min_max(
                pos2(
                    face.left(),
                    face.bottom() - face.height() * (0.25 + 0.15 * f),
                ),
                face.max,
            );
            out.push(Shape::rect_filled(
                gr,
                Rounding::same(3.0),
                ca(th.marker, 14),
            ));
        }
        let ink = Color32::from_rgb(40, 32, 24);
        let red = Color32::from_rgb(190, 40, 30);
        let (piv, rad) = vu_pivot(vr);
        let mut arc = Vec::new();
        let mut redarc = Vec::new();
        for i in 0..=40 {
            let db = -20.0 + 23.0 * i as f32 / 40.0;
            let p = polar(piv, rad - 22.0, vu_angle(vu_t(db)));
            if db >= 0.0 {
                redarc.push(p);
            }
            arc.push(p);
        }
        out.push(Shape::line(arc, sk(1.0, ink)));
        out.push(Shape::line(redarc, sk(3.0, red)));
        for (db, lab) in [
            (-20.0, "20"),
            (-10.0, "10"),
            (-7.0, ""),
            (-5.0, "5"),
            (-3.0, "3"),
            (-2.0, ""),
            (-1.0, ""),
            (0.0, "0"),
            (1.0, ""),
            (2.0, ""),
            (3.0, "+3"),
        ] {
            let a = vu_angle(vu_t(db));
            let col = if db > 0.0 { red } else { ink };
            out.push(Shape::line_segment(
                [polar(piv, rad - 22.0, a), polar(piv, rad - 15.0, a)],
                sk(1.2, col),
            ));
            if !lab.is_empty() {
                centred(&mut out, text(ui, lab, 9.0, col), polar(piv, rad - 8.0, a));
            }
        }
    }
    // The middle strip.
    let strip = Rect::from_min_size(pos2(24.0, MID_Y), vec2(W - 48.0, 34.0));
    out.push(Shape::rect_filled(
        strip,
        Rounding::same(6.0),
        ca(th.panel_raised, 150),
    ));
    for (k, name) in ["CTRL LINK", "L-R / M-S", "DUAL / LINK", "OVERSAMPLE"]
        .iter()
        .enumerate()
    {
        let s = mid_switch(k);
        let g = text(ui, name, 9.0, dim);
        out.push(Shape::galley(
            pos2(s.left() - g.size().x - 8.0, s.center().y - g.size().y / 2.0),
            g,
            dim,
        ));
    }
    // The bottom row.
    let bot = Rect::from_min_size(pos2(24.0, BOT_Y), vec2(W - 48.0, 124.0));
    out.push(Shape::rect_filled(
        bot,
        Rounding::same(6.0),
        ca(th.panel_raised, 200),
    ));
    out.push(Shape::rect_stroke(
        bot,
        Rounding::same(6.0),
        sk(1.0, ca(th.border, 160)),
    ));
    for (k, s) in smalls().iter().enumerate() {
        let cen = small_centre(k);
        centred(
            &mut out,
            text(ui, s.title, 9.0, dim),
            cen + vec2(0.0, -42.0),
        );
        for (v, lab) in s.ticks {
            let a = angle((v - s.min) / (s.max - s.min));
            out.push(Shape::line_segment(
                [polar(cen, SMALL_R + 3.0, a), polar(cen, SMALL_R + 6.0, a)],
                sk(1.2, dim),
            ));
            if !lab.is_empty() {
                centred(
                    &mut out,
                    text(ui, lab, 8.0, dim),
                    polar(cen, SMALL_R + 14.0, a),
                );
            }
        }
        out.push(Shape::circle_filled(
            cen + vec2(0.0, 2.0),
            SMALL_R + 1.0,
            ca(th.background, 160),
        ));
        out.push(Shape::circle_filled(
            cen,
            SMALL_R,
            lerp(th.panel_raised, th.background, 0.55),
        ));
        out.push(Shape::circle_stroke(
            cen,
            SMALL_R,
            sk(1.2, lerp(th.border, th.text, 0.25)),
        ));
    }
    centred(
        &mut out,
        text(ui, "SATURATOR", 9.0, dim),
        sat_switch().center() + vec2(0.0, -16.0),
    );
    let lr = loud_rect();
    out.push(Shape::rect_filled(
        lr,
        Rounding::same(4.0),
        c(th.background),
    ));
    out.push(Shape::rect_stroke(
        lr,
        Rounding::same(4.0),
        sk(1.0, c(th.border)),
    ));
    for (k, name) in ["LUFS M", "PEAK"].iter().enumerate() {
        let y = lr.top() + 20.0 + k as f32 * 40.0;
        out.push(Shape::galley(
            pos2(lr.left() + 8.0, y - 6.0),
            text(ui, name, 9.0, dim),
            dim,
        ));
        // Scale -40..0.
        for db in [-40i32, -30, -20, -14, -9, -6, -3, 0] {
            let x = bar_x(lr, db as f32);
            out.push(Shape::line_segment(
                [pos2(x, y + 9.0), pos2(x, y + 12.0)],
                sk(1.0, dim),
            ));
            if k == 1 || db % 10 == 0 || db == -14 {
                let g = text(ui, &format!("{}", db.abs()), 7.5, ca(th.text_dim, 200));
                centred(&mut out, g, pos2(x, y + 18.0));
            }
        }
    }
    out
}

fn bar_x(lr: Rect, db: f32) -> f32 {
    let x0 = lr.left() + 56.0;
    let x1 = lr.right() - 46.0;
    x0 + (x1 - x0) * ((db.clamp(-40.0, 0.0) + 40.0) / 40.0)
}

#[derive(Clone)]
struct Cache {
    key: (String, u32, u32),
    shapes: Arc<Vec<Shape>>,
}

fn cached(ui: &egui::Ui, th: &Theme) -> Arc<Vec<Shape>> {
    let id = egui::Id::new("tube-master-static");
    let key = (
        th.name.clone(),
        (ui.ctx().pixels_per_point() * 1000.0) as u32,
        (th.font_scale * 1000.0) as u32,
    );
    if let Some(c) = ui.ctx().data(|d| d.get_temp::<Cache>(id)) {
        if c.key == key {
            return c.shapes;
        }
    }
    let shapes = Arc::new(static_layer(ui, th));
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            id,
            Cache {
                key,
                shapes: shapes.clone(),
            },
        )
    });
    shapes
}

// ------------------------------------------------------- moving parts

/// The window's meter state: needle positions and the loudness bar,
/// with their ballistics.
#[derive(Clone, Default)]
struct Meters {
    needle: [f32; 2],
    lufs: f32,
    peak: f32,
    hold: f32,
    hold_at: f64,
    beat: u32,
    beat_at: f64,
    seen: bool,
}

fn knob_pointer(p: &egui::Painter, cen: Pos2, r: f32, t: f32, th: &Theme, live: bool) {
    let a = angle(t);
    let col = if live {
        c(th.accent)
    } else {
        ca(th.text_dim, 160)
    };
    p.line_segment(
        [polar(cen, r * 0.25, a), polar(cen, r - 2.0, a)],
        sk(3.0, col),
    );
    p.circle_filled(polar(cen, r - 3.0, a), 2.2, col);
}

/// A lit toggle: the left label when off, the right label when on.
fn switch(
    p: &egui::Painter,
    ui: &egui::Ui,
    r: Rect,
    on: bool,
    labels: [&str; 2],
    hot: bool,
    th: &Theme,
) {
    let rr = Rounding::same(r.height() / 2.0);
    p.rect_filled(r, rr, c(th.background));
    p.rect_stroke(
        r,
        rr,
        sk(1.0, if hot { c(th.accent) } else { c(th.border) }),
    );
    let k = if on {
        Rect::from_min_size(
            pos2(r.right() - r.height(), r.top()),
            vec2(r.height(), r.height()),
        )
    } else {
        Rect::from_min_size(r.min, vec2(r.height(), r.height()))
    };
    p.circle_filled(
        k.center(),
        r.height() / 2.0 - 1.5,
        lerp(th.panel_raised, th.text, 0.45),
    );
    // The lamp.
    let lamp = pos2(r.right() + 9.0, r.center().y);
    if on {
        p.circle_filled(lamp, 6.0, ca(th.accent, 50));
    }
    p.circle_filled(lamp, 3.5, if on { c(th.accent) } else { ca(th.accent, 35) });
    let lab = if on { labels[1] } else { labels[0] };
    if !lab.is_empty() {
        let g = text(ui, lab, 8.5, if on { c(th.text) } else { c(th.text_dim) });
        p.galley(
            pos2(lamp.x + 8.0, r.center().y - g.size().y / 2.0),
            g,
            Color32::WHITE,
        );
    }
}

/// Where values change, and whether a gesture ended.
struct Edit {
    vals: Vec<f32>,
    changed: bool,
    released: bool,
}

impl Edit {
    fn set(&mut self, i: usize, v: f32) {
        if self.vals[i] != v {
            self.vals[i] = v;
            self.changed = true;
        }
    }
}

/// A knob's response: drag up or right to turn, scroll to step,
/// double-click for its default. Returns the new position for a stepped
/// knob (steps) or value (continuous).
fn turn(ui: &egui::Ui, resp: &egui::Response, id: egui::Id, steps: f32) -> f32 {
    let mut d = 0.0;
    if resp.dragged() {
        let dd = resp.drag_delta();
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.2
        } else {
            1.0
        };
        d += (dd.x - dd.y) * fine;
    }
    let mut out = 0.0;
    if d != 0.0 {
        // Pixels per step: stepped knobs click over every few pixels.
        let per = (160.0 / steps).clamp(6.0, 22.0);
        let acc: f32 = ui.ctx().data(|m| m.get_temp(id)).unwrap_or(0.0) + d / per;
        let whole = acc.trunc();
        ui.ctx().data_mut(|m| m.insert_temp(id, acc - whole));
        out += whole;
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|m| m.insert_temp(id, 0.0f32));
    }
    if resp.hovered() {
        let s = ui.input(|i| i.raw_scroll_delta.y);
        if s > 0.0 {
            out += 1.0;
        } else if s < 0.0 {
            out -= 1.0;
        }
    }
    out
}

/// Gives the slot a meter of its own (the hidden `meter` value): a number
/// no other Tube Master in the session uses. Not an undo step.
fn claim_meter(st: &mut State, target: u16, slot: usize) -> usize {
    let mi = at("meter");
    let mine = chain_of(st, target)
        .and_then(|c| c.get(slot))
        .and_then(|s| s.values.get(mi).copied())
        .unwrap_or(0.0)
        .round() as usize;
    let mut used = vec![0u32; tm::METERS];
    let chains =
        std::iter::once(&st.session.master_chain).chain(st.session.tracks.iter().map(|t| &t.chain));
    for ch in chains {
        for s in ch.iter().filter(|s| s.effect == tm::ID) {
            let v = s.values.get(mi).copied().unwrap_or(0.0).round() as usize;
            used[v.min(tm::METERS - 1)] += 1;
        }
    }
    if mine > 0 && mine < tm::METERS && used[mine] <= 1 {
        return mine;
    }
    let free = (1..tm::METERS).find(|k| used[*k] == 0).unwrap_or(0);
    if free == 0 {
        return 0;
    }
    if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(slot)) {
        if s.values.len() < keys().len() {
            let d = defaults();
            let n = s.values.len();
            s.values.extend_from_slice(&d[n..]);
        }
        s.values[mi] = free as f32;
    }
    if target != FILE {
        st.live_slot(target, slot);
        st.session_dirty = true;
    }
    free
}

/// Draws Tube Master's panel for a slot. False (drawing nothing) when
/// the slot holds another effect.
pub fn ui(ui: &mut egui::Ui, st: &mut State, target: u16, slot: usize) -> bool {
    let Some(effect) = chain_of(st, target)
        .and_then(|c| c.get(slot))
        .map(|s| s.effect.clone())
    else {
        return false;
    };
    if effect != tm::ID {
        return false;
    }
    let meter_idx = claim_meter(st, target, slot);
    let Some(values) = chain_of(st, target)
        .and_then(|c| c.get(slot))
        .map(|s| s.values.clone())
    else {
        return false;
    };
    let th = st.theme.clone();
    let before = st.session.clone();
    let mut vals = values;
    let d = defaults();
    if vals.len() < d.len() {
        let n = vals.len();
        vals.extend_from_slice(&d[n..]);
    }
    let mut ed = Edit {
        vals,
        changed: false,
        released: false,
    };
    let before_os = ed.vals[at("os")];

    let (rect, _) = ui.allocate_exact_size(vec2(W, H), Sense::hover());
    let painter = ui.painter_at(rect);
    let o = rect.min.to_vec2();
    let shapes = cached(ui, &th);
    painter.extend(shapes.iter().cloned().map(|mut s| {
        s.translate(o);
        s
    }));

    let base = egui::Id::new(("tube-master", target, slot));
    let link = ed.vals[at("link")] >= 0.5;
    let ms = ed.vals[at("ms")] >= 0.5;

    // ---- the channel rows
    for r in 0..2 {
        let row_live = !(link && r == 1);
        let src = if row_live { r } else { 0 };
        // The row's badge.
        let badge = Rect::from_min_size(pos2(30.0, ROW_Y[r] + 48.0) + o, vec2(20.0, 32.0));
        painter.rect_filled(badge, Rounding::same(4.0), ca(th.accent, 40));
        let name = match (ms, r) {
            (false, 0) => "L",
            (false, _) => "R",
            (true, 0) => "M",
            (true, _) => "S",
        };
        let g = text(ui, name, 16.0, c(th.accent));
        painter.galley(badge.center() - g.size() / 2.0, g, Color32::WHITE);

        for (k, b) in bigs().iter().enumerate() {
            let cen = big_centre(r, k) + o;
            let key = ROW_KEYS[src][b.slot];
            let i = at(key);
            let steps = b.steps;
            // Output is a number from -12 to +12 in whole dB.
            let is_out = b.slot == 5;
            let cur = if is_out {
                (ed.vals[i].round() + 12.0).clamp(0.0, 24.0) as usize
            } else {
                (ed.vals[i].round().max(0.0) as usize).min(steps - 1)
            };
            let hit = Rect::from_center_size(cen, vec2(BIG_R * 2.4, BIG_R * 2.4));
            let resp = ui.interact(
                hit,
                base.with(("big", r, k)),
                if row_live {
                    Sense::click_and_drag()
                } else {
                    Sense::hover()
                },
            );
            let mut pos = cur as i32;
            if row_live {
                let dv = turn(ui, &resp, base.with(("acc", r, k)), steps as f32);
                pos = (pos + dv as i32).clamp(0, steps as i32 - 1);
                if resp.double_clicked() {
                    let dv = d[i];
                    pos = if is_out {
                        (dv + 12.0) as i32
                    } else {
                        dv as i32
                    };
                }
                if pos as usize != cur {
                    ed.set(
                        i,
                        if is_out {
                            pos as f32 - 12.0
                        } else {
                            pos as f32
                        },
                    );
                }
                ed.released |= resp.drag_stopped()
                    || resp.double_clicked()
                    || (resp.hovered() && ed.changed && !resp.dragged());
            }
            let t = pos as f32 / (steps - 1) as f32;
            if resp.hovered() || resp.dragged() {
                painter.circle_stroke(cen, BIG_R + 1.5, sk(1.0, ca(th.accent, 120)));
            }
            knob_pointer(&painter, cen, BIG_R, t, &th, row_live);
            let read = (b.read)(pos as usize);
            let col = if row_live { c(th.text) } else { c(th.text_dim) };
            let g = text(ui, &read, 10.0, col);
            painter.galley(
                cen + vec2(-g.size().x / 2.0, BIG_R + 28.0),
                g,
                Color32::WHITE,
            );
            if !row_live {
                resp.on_hover_text("Ctrl Link: this row follows channel 1");
            }
        }

        // The row's switches.
        let labs: [[&str; 2]; 3] = [["OUT", "IN"], ["OFF", "HPF"], ["IN", "GR"]];
        let sw_keys = [ROW_KEYS[r][6], ROW_KEYS[src][7], ROW_KEYS[r][8]];
        for (k, key) in sw_keys.iter().enumerate() {
            let sr = row_switch(r, k).translate(o);
            let i = at(key);
            let on = ed.vals[i] >= 0.5;
            let live = k != 1 || row_live;
            let resp = ui.interact(
                sr.expand(4.0),
                base.with(("sw", r, k)),
                if live { Sense::click() } else { Sense::hover() },
            );
            let mut now = on;
            if live && resp.clicked() {
                now = !on;
                ed.set(i, if now { 1.0 } else { 0.0 });
                ed.released = true;
            }
            switch(&painter, ui, sr, now, labs[k], resp.hovered() && live, &th);
        }
    }

    // ---- the middle switches
    let mids: [(&str, [&str; 2]); 4] = [
        ("link", ["OFF", "ON"]),
        ("ms", ["L-R", "M-S"]),
        ("detect", ["DUAL", "LINK"]),
        ("os", ["2x", "4x"]),
    ];
    for (k, (key, labs)) in mids.iter().enumerate() {
        let sr = mid_switch(k).translate(o);
        let i = at(key);
        let on = ed.vals[i] >= 0.5;
        let resp = ui.interact(sr.expand(4.0), base.with(("mid", k)), Sense::click());
        let mut now = on;
        if resp.clicked() {
            now = !on;
            ed.set(i, if now { 1.0 } else { 0.0 });
            ed.released = true;
        }
        switch(&painter, ui, sr, now, *labs, resp.hovered(), &th);
    }

    // ---- the bottom knobs
    for (k, s) in smalls().iter().enumerate() {
        let cen = small_centre(k) + o;
        let i = at(s.key);
        let mut v = ed.vals[i].clamp(s.min, s.max);
        let hit = Rect::from_center_size(cen, vec2(SMALL_R * 2.6, SMALL_R * 2.6));
        let resp = ui.interact(hit, base.with(("small", k)), Sense::click_and_drag());
        let steps = ((s.max - s.min) / s.step).max(1.0);
        // Continuous knobs move 1/200 of their range per pixel; the
        // wheel moves one step.
        if resp.dragged() {
            let dd = resp.drag_delta();
            let fine = if ui.input(|i| i.modifiers.shift) {
                0.2
            } else {
                1.0
            };
            v += (dd.x - dd.y) * fine * (s.max - s.min) / 200.0;
        }
        if resp.hovered() {
            let sc = ui.input(|i| i.raw_scroll_delta.y);
            if sc != 0.0 {
                v += s.step * sc.signum() * if steps > 60.0 { 5.0 } else { 1.0 };
            }
        }
        if resp.double_clicked() {
            v = d[i];
        }
        v = ((v / s.step).round() * s.step).clamp(s.min, s.max);
        ed.set(i, v);
        ed.released |= resp.drag_stopped()
            || resp.double_clicked()
            || (resp.hovered() && !resp.dragged() && ui.input(|i| i.raw_scroll_delta.y != 0.0));
        if resp.hovered() || resp.dragged() {
            painter.circle_stroke(cen, SMALL_R + 1.5, sk(1.0, ca(th.accent, 120)));
        }
        let t = (v - s.min) / (s.max - s.min);
        knob_pointer(&painter, cen, SMALL_R, t, &th, true);
        let g = text(ui, &(s.read)(v), 9.5, c(th.text));
        painter.galley(
            cen + vec2(-g.size().x / 2.0, SMALL_R + 21.0),
            g,
            Color32::WHITE,
        );
    }
    {
        let sr = sat_switch().translate(o);
        let i = at("sat");
        let on = ed.vals[i] >= 0.5;
        let resp = ui.interact(sr.expand(4.0), base.with("sat"), Sense::click());
        let mut now = on;
        if resp.clicked() {
            now = !on;
            ed.set(i, if now { 1.0 } else { 0.0 });
            ed.released = true;
        }
        switch(&painter, ui, sr, now, ["OFF", "ON"], resp.hovered(), &th);
    }

    // ---- the meters
    let now = ui.input(|i| i.time);
    let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
    let reading = tm::meter(meter_idx).read();
    let mid = base.with("meters");
    let mut m: Meters = ui.ctx().data(|d| d.get_temp(mid)).unwrap_or_default();
    if !m.seen || reading.beat != m.beat {
        m.beat = reading.beat;
        m.beat_at = now;
    }
    // No readings for a moment: the sound has stopped.
    let running = m.seen && now - m.beat_at < 0.25;
    m.seen = true;
    let mut moving = false;
    for (r, row_keys) in ROW_KEYS.iter().enumerate() {
        let gr_mode = ed.vals[at(row_keys[8])] >= 0.5;
        let db = if !running {
            if gr_mode {
                0.0
            } else {
                -60.0
            }
        } else if gr_mode {
            -reading.gr_db[r]
        } else {
            // 0 on the meter is -18 dBFS (RMS) at the input.
            reading.input_db[r] + 18.0
        };
        let target = vu_t(db);
        let k = 1.0 - (-dt / 0.09).exp();
        let before = m.needle[r];
        m.needle[r] += (target - m.needle[r]) * k;
        if (m.needle[r] - before).abs() > 0.0005 || (target - m.needle[r]).abs() > 0.002 {
            moving = true;
        }
        let vr = vu_rect(r).translate(o);
        let face = vu_face(vr);
        let (piv, rad) = vu_pivot(vr);
        let clip = painter.with_clip_rect(face);
        let a = vu_angle(m.needle[r]);
        clip.line_segment(
            [polar(piv, 14.0, a), polar(piv, rad - 12.0, a)],
            sk(1.6, Color32::from_rgb(28, 22, 18)),
        );
        let g = text(
            ui,
            if gr_mode { "GR" } else { "VU" },
            10.0,
            Color32::from_rgb(60, 48, 36),
        );
        clip.galley(
            pos2(face.left() + 6.0, face.bottom() - 20.0),
            g,
            Color32::WHITE,
        );
        // The bezel's lower lip hides the pivot.
        painter.rect_filled(
            Rect::from_min_max(pos2(face.left(), face.bottom() - 6.0), face.max),
            Rounding::ZERO,
            ca(th.background, 90),
        );
    }
    // The loudness and peak bar.
    let (lufs_t, peak_t) = if running {
        (reading.lufs, reading.peak_db)
    } else {
        (-70.0, -70.0)
    };
    let kl = 1.0 - (-dt / 0.12).exp();
    let lb = m.lufs;
    m.lufs += (lufs_t.max(-70.0) - m.lufs) * kl;
    let pb = m.peak;
    m.peak = if peak_t > m.peak {
        peak_t
    } else {
        m.peak - 24.0 * dt
    };
    m.peak = m.peak.max(-70.0);
    if m.peak >= m.hold || now - m.hold_at > 1.5 {
        m.hold = m.peak;
        m.hold_at = now;
    }
    if (m.lufs - lb).abs() > 0.01 || (m.peak - pb).abs() > 0.01 || m.hold > -69.0 {
        moving = true;
    }
    let lr = loud_rect().translate(o);
    for (k, v) in [m.lufs, m.peak].iter().enumerate() {
        let y = lr.top() + 20.0 + k as f32 * 40.0;
        let track = Rect::from_min_max(
            pos2(bar_x(lr, -40.0), y - 4.0),
            pos2(bar_x(lr, 0.0), y + 6.0),
        );
        painter.rect_filled(track, Rounding::same(2.0), ca(th.panel_raised, 220));
        let x = bar_x(lr, *v);
        if *v > -40.0 {
            let t = (v + 40.0) / 40.0;
            let col = if *v > -3.0 {
                c(th.meter_high)
            } else if t > 0.65 {
                lerp(th.meter_low, th.meter_mid, (t - 0.65) / 0.25)
            } else {
                c(th.meter_low)
            };
            painter.rect_filled(
                Rect::from_min_max(track.min, pos2(x, track.bottom())),
                Rounding::same(2.0),
                col,
            );
        }
        if k == 1 && m.hold > -40.0 {
            let hx = bar_x(lr, m.hold);
            painter.line_segment(
                [pos2(hx, track.top()), pos2(hx, track.bottom())],
                sk(2.0, c(th.text)),
            );
        }
        let shown = if k == 0 { *v } else { m.hold };
        let s = if shown <= -69.0 {
            "-inf".to_string()
        } else {
            format!("{shown:.1}")
        };
        let g = text(ui, &s, 11.0, c(th.text));
        painter.galley(
            pos2(lr.right() - 8.0 - g.size().x, y - 6.0),
            g,
            Color32::WHITE,
        );
    }
    // Gain reduction, as numbers.
    let g = text(
        ui,
        &format!(
            "GR  {:.1}  /  {:.1} dB",
            if running { reading.gr_db[0] } else { 0.0 },
            if running { reading.gr_db[1] } else { 0.0 }
        ),
        9.0,
        c(th.text_dim),
    );
    painter.galley(pos2(lr.left() + 8.0, lr.bottom() - 18.0), g, Color32::WHITE);
    ui.ctx().data_mut(|d| d.insert_temp(mid, m));
    if moving || running {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }

    // ---- commit, as the plain controls do
    let os_changed = ed.vals[at("os")] != before_os;
    if ed.changed {
        if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(slot)) {
            s.values = ed.vals;
        }
        if target != FILE {
            st.live_slot(target, slot);
            let down = ui.input(|i| i.pointer.any_down());
            if !st.gesture {
                st.commit_live("Tube Master", before);
                st.gesture = down;
            }
        }
    }
    if ed.released {
        st.gesture = false;
        // Everything retunes while playing except the oversampling (its
        // latency changes): that rebuilds.
        if target != FILE && os_changed {
            st.session_version += 1;
        }
    }
    true
}

fn sk(w: f32, col: Color32) -> Stroke {
    Stroke::new(w, col)
}
