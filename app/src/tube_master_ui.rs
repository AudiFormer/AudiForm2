// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Tube Master's window: a hardware-style panel with one row of large
//! stepped knobs (the circuit every channel runs), a lit needle meter
//! beside them (input level and gain reduction of the loudest channel,
//! two needles on one face) with a per-channel strip, and below a row of
//! smaller knobs, the switches and a loudness and peak bar. The grid
//! follows the shared effect-window spacing (`fx_windows::layout`).
//! Everything that never moves (the panel, tick rings, labels and meter
//! face) is drawn once and kept; while the meters move it repaints,
//! otherwise it waits.
//!
//! Values go through the same live path as the plain controls: the
//! slot's values change, `live_slot` sends them, one undo step per
//! gesture.

use std::sync::{Arc, OnceLock};

use egui::{pos2, vec2, Color32, FontId, Galley, Pos2, Rect, Rounding, Sense, Shape, Stroke};

use af2_fx::effects::tube_master::{self as tm, TubeReading};

use crate::fx_windows::layout::{GAP, GAP_L, GAP_S, PAD};
use crate::rack::{chain_of, FILE};
use crate::state::State;
use crate::theme::{c, ca, lerp, Theme};

// ------------------------------------------------------------ geometry
//
// One balanced grid: an outer margin, then two columns (the circuit on
// the left, the meters on the right) a group gap apart, each with a main
// plate on top and a lower plate beneath, a row gap between.

/// The outer margin (room for the screws).
const M: f32 = GAP_L + PAD;
/// The circuit column: six knob cells.
const CELL: f32 = 92.0;
const LEFT_W: f32 = CELL * 6.0;
/// The meter column.
const RIGHT_W: f32 = 304.0;
const W: f32 = M * 2.0 + LEFT_W + GAP + RIGHT_W;
const HEAD_H: f32 = 40.0;
const TOP_Y: f32 = HEAD_H + 4.0;
const TOP_H: f32 = 180.0;
const BOT_Y: f32 = TOP_Y + TOP_H + GAP;
const BOT_H: f32 = 132.0;
const H: f32 = BOT_Y + BOT_H + M - 4.0;
const BIG_R: f32 = 26.0;
const SMALL_R: f32 = 17.0;
/// The lower circuit plate: five small knob cells, then the switches.
const SMALL_CELL: f32 = 72.0;

fn left_x() -> f32 {
    M
}

fn right_x() -> f32 {
    M + LEFT_W + GAP
}

fn top_left() -> Rect {
    Rect::from_min_size(pos2(left_x(), TOP_Y), vec2(LEFT_W, TOP_H))
}

fn top_right() -> Rect {
    Rect::from_min_size(pos2(right_x(), TOP_Y), vec2(RIGHT_W, TOP_H))
}

fn bot_left() -> Rect {
    Rect::from_min_size(pos2(left_x(), BOT_Y), vec2(LEFT_W, BOT_H))
}

fn bot_right() -> Rect {
    Rect::from_min_size(pos2(right_x(), BOT_Y), vec2(RIGHT_W, BOT_H))
}

/// A big knob's centre, column `k` (relative).
fn big_centre(k: usize) -> Pos2 {
    pos2(left_x() + CELL * (k as f32 + 0.5), TOP_Y + 96.0)
}

fn small_centre(k: usize) -> Pos2 {
    pos2(
        left_x() + SMALL_CELL * (k as f32 + 0.5),
        BOT_Y + BOT_H / 2.0 + 4.0,
    )
}

/// The switch column: the left edge of its labels and the switches.
fn switch_label_x() -> f32 {
    left_x() + SMALL_CELL * 5.0 + GAP
}

const SWITCHES: usize = 5;

fn switch_rect(k: usize) -> Rect {
    let step = (BOT_H - 2.0 * PAD) / SWITCHES as f32;
    let y = BOT_Y + PAD + step * (k as f32 + 0.5);
    Rect::from_center_size(pos2(switch_label_x() + 104.0, y), vec2(36.0, 14.0))
}

/// The needle meter's housing, and the per-channel strip beside it.
fn vu_rect() -> Rect {
    let r = top_right().shrink(PAD);
    Rect::from_min_size(r.min, vec2(184.0, r.height()))
}

fn strip_rect() -> Rect {
    let r = top_right().shrink(PAD);
    let v = vu_rect();
    Rect::from_min_max(pos2(v.right() + GAP_S, r.top()), r.max)
}

fn loud_rect() -> Rect {
    bot_right().shrink(PAD)
}

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

// ------------------------------------------------------------- the knobs

/// What a big stepped knob shows: its value, title, the label at each
/// step (empty for a plain tick), and its readout.
struct Big {
    key: &'static str,
    title: &'static str,
    steps: usize,
    label: fn(usize) -> &'static str,
    read: fn(usize) -> String,
}

fn bigs() -> [Big; 6] {
    [
        Big {
            key: "in",
            title: "INPUT",
            steps: 5,
            label: |s| ["1", "2", "3", "4", "5"][s],
            read: |s| format!("+{:.0} dB", tm::INPUT_DB[s]),
        },
        Big {
            key: "thr",
            title: "THRESHOLD",
            steps: 11,
            label: |s| ["-30", "", "-24", "", "-18", "", "-12", "", "-6", "", "0"][s],
            read: |s| format!("{:.0} dB", tm::THRESHOLD_DB[s]),
        },
        Big {
            key: "att",
            title: "ATTACK",
            steps: 5,
            label: |s| ["SLOW", "", "MED", "", "FAST"][s],
            read: |s| format!("{:.0} ms", tm::ATTACK_MS[s]),
        },
        Big {
            key: "rel",
            title: "RELEASE",
            steps: 5,
            label: |s| ["SLOW", "", "MED", "", "FAST"][s],
            read: |s| format!("{:.2} s", tm::RELEASE_S[s]),
        },
        Big {
            key: "ratio",
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
            key: "out",
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

/// A small continuous knob in the lower row.
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

/// The switch column: key, label, the off and on words.
const SWITCH_ROWS: [(&str, &str, [&str; 2]); SWITCHES] = [
    ("sc", "SC FILTER", ["OFF", "HPF"]),
    ("sat", "SATURATOR", ["OFF", "ON"]),
    ("detect", "DETECTION", ["IND", "LINK"]),
    ("ms", "STEREO", ["L-R", "M-S"]),
    ("os", "OVERSAMPLE", ["2x", "4x"]),
];

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

fn vu_face(r: Rect) -> Rect {
    r.shrink(7.0)
}

fn vu_pivot(r: Rect) -> (Pos2, f32) {
    let face = vu_face(r);
    (
        pos2(face.center().x, face.bottom() + 6.0),
        face.height() * 0.86,
    )
}

fn vu_angle(t: f32) -> f32 {
    (t - 0.5) * 1.4
}

/// What a layout of `n` channels is called.
fn layout_name(n: usize) -> String {
    match n {
        1 => "MONO".into(),
        2 => "STEREO".into(),
        3 => "3.0".into(),
        4 => "QUAD".into(),
        5 => "5.0".into(),
        6 => "5.1".into(),
        7 => "6.1".into(),
        8 => "7.1".into(),
        10 => "7.1.2".into(),
        12 => "7.1.4".into(),
        n => format!("{n} CH"),
    }
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

fn plate(out: &mut Vec<Shape>, r: Rect, th: &Theme) {
    out.push(Shape::rect_filled(
        r,
        Rounding::same(6.0),
        ca(th.panel_raised, 200),
    ));
    out.push(Shape::rect_stroke(
        r,
        Rounding::same(6.0),
        sk(1.0, ca(th.border, 160)),
    ));
}

fn knob_body(out: &mut Vec<Shape>, cen: Pos2, r: f32, th: &Theme) {
    out.push(Shape::circle_filled(
        cen + vec2(0.0, 2.0),
        r + 1.0,
        ca(th.background, 160),
    ));
    out.push(Shape::circle_filled(
        cen,
        r,
        lerp(th.panel_raised, th.background, 0.55),
    ));
    out.push(Shape::circle_stroke(
        cen,
        r,
        sk(1.5, lerp(th.border, th.text, 0.25)),
    ));
    out.push(Shape::circle_filled(
        cen,
        r - r * 0.24,
        lerp(th.panel_raised, th.text, 0.10),
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
    let bands = 20;
    for i in 0..bands {
        let bh = H / bands as f32;
        let a = if i % 2 == 0 { 10 } else { 4 };
        out.push(Shape::rect_filled(
            Rect::from_min_size(pos2(6.0, i as f32 * bh), vec2(W - 12.0, bh)),
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
        pos2(12.0, 12.0),
        pos2(W - 12.0, 12.0),
        pos2(12.0, H - 12.0),
        pos2(W - 12.0, H - 12.0),
    ] {
        screw(&mut out, p, th);
    }
    // The name plate.
    let g = text(ui, "TUBE  MASTER", 18.0, txt);
    let gy = (HEAD_H - g.size().y) / 2.0 + 2.0;
    out.push(Shape::galley(pos2(M, gy), g.clone(), txt));
    let sub = text(ui, "multichannel valve mastering compressor", 11.0, dim);
    out.push(Shape::galley(
        pos2(
            M + g.size().x + GAP,
            gy + (g.size().y - sub.size().y) / 2.0 + 1.0,
        ),
        sub,
        dim,
    ));
    for r in [top_left(), top_right(), bot_left(), bot_right()] {
        plate(&mut out, r, th);
    }
    // The circuit: six big knobs.
    for (k, b) in bigs().iter().enumerate() {
        let cen = big_centre(k);
        centred(
            &mut out,
            text(ui, b.title, 10.0, dim),
            cen + vec2(0.0, -BIG_R - 40.0),
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
        knob_body(&mut out, cen, BIG_R, th);
    }
    // The lower circuit plate: small knobs and switches.
    for (k, s) in smalls().iter().enumerate() {
        let cen = small_centre(k);
        centred(
            &mut out,
            text(ui, s.title, 9.0, dim),
            cen + vec2(0.0, -SMALL_R - 26.0),
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
        knob_body(&mut out, cen, SMALL_R, th);
    }
    // A rule between the knobs and the switches.
    let rx = switch_label_x() - GAP / 2.0;
    out.push(Shape::line_segment(
        [
            pos2(rx, BOT_Y + PAD + 4.0),
            pos2(rx, BOT_Y + BOT_H - PAD - 4.0),
        ],
        sk(1.0, ca(th.border, 140)),
    ));
    for (k, (_, name, _)) in SWITCH_ROWS.iter().enumerate() {
        let s = switch_rect(k);
        let g = text(ui, name, 8.5, dim);
        out.push(Shape::galley(
            pos2(switch_label_x() + 2.0, s.center().y - g.size().y / 2.0),
            g,
            dim,
        ));
    }
    // The needle meter's face.
    let vr = vu_rect();
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
    let arc_r = rad - 30.0;
    let mut arc = Vec::new();
    let mut redarc = Vec::new();
    for i in 0..=48 {
        let db = -20.0 + 23.0 * i as f32 / 48.0;
        let p = polar(piv, arc_r, vu_angle(vu_t(db)));
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
        (-7.0, "7"),
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
            [polar(piv, arc_r, a), polar(piv, arc_r + 8.0, a)],
            sk(1.2, col),
        ));
        if !lab.is_empty() {
            centred(
                &mut out,
                text(ui, lab, 9.5, col),
                polar(piv, arc_r + 17.0, a),
            );
        }
    }
    // The legend: which needle is which.
    let g = text(ui, "IN", 10.0, ink);
    out.push(Shape::galley(
        pos2(face.left() + 8.0, face.top() + 6.0),
        g,
        ink,
    ));
    let g = text(ui, "GR", 10.0, red);
    out.push(Shape::galley(
        pos2(face.right() - 8.0 - g.size().x, face.top() + 6.0),
        g,
        red,
    ));
    // The strip's well and the loudness box.
    for r in [strip_rect(), loud_rect()] {
        out.push(Shape::rect_filled(r, Rounding::same(4.0), c(th.background)));
        out.push(Shape::rect_stroke(
            r,
            Rounding::same(4.0),
            sk(1.0, c(th.border)),
        ));
    }
    let lr = loud_rect();
    for (k, name) in ["LUFS M", "PEAK"].iter().enumerate() {
        let y = loud_row_y(lr, k);
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

fn loud_row_y(lr: Rect, k: usize) -> f32 {
    lr.top() + 18.0 + k as f32 * 38.0
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
    let id = egui::Id::new("tube-master-static-v2");
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

/// The window's meter state: needle positions, each channel's strip
/// bar, and the loudness bar, with their ballistics.
#[derive(Clone, Default)]
struct Meters {
    /// The input needle and the gain-reduction needle.
    needle: [f32; 2],
    bars: [f32; tm::MAX_CH],
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

/// A lit toggle: the left label when off, the right label when on;
/// greyed out when it does nothing here.
#[allow(clippy::too_many_arguments)]
fn switch(
    p: &egui::Painter,
    ui: &egui::Ui,
    r: Rect,
    on: bool,
    labels: [&str; 2],
    hot: bool,
    live: bool,
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
        if live {
            lerp(th.panel_raised, th.text, 0.45)
        } else {
            lerp(th.panel_raised, th.text, 0.15)
        },
    );
    // The lamp.
    let lit = on && live;
    let lamp = pos2(r.right() + 9.0, r.center().y);
    if lit {
        p.circle_filled(lamp, 6.0, ca(th.accent, 50));
    }
    p.circle_filled(
        lamp,
        3.5,
        if lit { c(th.accent) } else { ca(th.accent, 35) },
    );
    let lab = if !live {
        "n/a"
    } else if on {
        labels[1]
    } else {
        labels[0]
    };
    let g = text(ui, lab, 8.5, if lit { c(th.text) } else { c(th.text_dim) });
    p.galley(
        pos2(lamp.x + 8.0, r.center().y - g.size().y / 2.0),
        g,
        Color32::WHITE,
    );
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
/// double-click for its default. Returns how many steps it moved.
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

/// The names the strip shows for `n` channels (mid and side when a
/// stereo pair runs in Mid-Side).
fn channel_names(r: &TubeReading, n: usize) -> Vec<String> {
    let n = n.max(1);
    if r.ms && n == 2 {
        return vec!["M".into(), "S".into()];
    }
    af2_dsp::wide::labels(n)
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
    let reading = tm::meter(meter_idx).read();
    // Not built yet: a stereo track is the likeliest.
    let channels = if reading.channels == 0 {
        2
    } else {
        reading.channels
    };
    let linked = ed.vals[at("detect")] >= 0.5;

    // ---- the header badge: what it runs on
    {
        let worked = (0..channels)
            .filter(|c| *c >= 32 || reading.passed & (1 << c) == 0)
            .count();
        let mut s = layout_name(channels);
        if channels > 2 {
            s = format!("{s}  ·  {worked} CIRCUITS");
        } else if channels == 2 {
            s = format!("{s}  ·  2 CIRCUITS");
        } else {
            s = format!("{s}  ·  1 CIRCUIT");
        }
        s = format!("{s}  ·  {}", if linked { "LINKED" } else { "INDEPENDENT" });
        let g = text(ui, &s, 9.5, c(th.accent));
        let pad = vec2(8.0, 4.0);
        let r = Rect::from_min_size(
            pos2(
                W - M - g.size().x - pad.x * 2.0,
                (HEAD_H - g.size().y) / 2.0 - 2.0,
            ) + o,
            g.size() + pad * 2.0,
        );
        painter.rect_filled(r, Rounding::same(4.0), ca(th.accent, 30));
        painter.rect_stroke(r, Rounding::same(4.0), sk(1.0, ca(th.accent, 90)));
        painter.galley(r.min + pad, g, Color32::WHITE);
    }

    // ---- the big knobs
    for (k, b) in bigs().iter().enumerate() {
        let cen = big_centre(k) + o;
        let i = at(b.key);
        let steps = b.steps;
        // Output is a number from -12 to +12 in whole dB.
        let is_out = b.key == "out";
        let cur = if is_out {
            (ed.vals[i].round() + 12.0).clamp(0.0, 24.0) as usize
        } else {
            (ed.vals[i].round().max(0.0) as usize).min(steps - 1)
        };
        let hit = Rect::from_center_size(cen, vec2(BIG_R * 2.4, BIG_R * 2.4));
        let resp = ui.interact(hit, base.with(("big", k)), Sense::click_and_drag());
        let mut pos = cur as i32;
        let dv = turn(ui, &resp, base.with(("acc", k)), steps as f32);
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
        let t = pos as f32 / (steps - 1) as f32;
        if resp.hovered() || resp.dragged() {
            painter.circle_stroke(cen, BIG_R + 1.5, sk(1.0, ca(th.accent, 120)));
        }
        knob_pointer(&painter, cen, BIG_R, t, &th, true);
        let g = text(ui, &(b.read)(pos as usize), 10.0, c(th.text));
        painter.galley(
            cen + vec2(-g.size().x / 2.0, BIG_R + 26.0),
            g,
            Color32::WHITE,
        );
    }

    // ---- the small knobs
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
            cen + vec2(-g.size().x / 2.0, SMALL_R + 18.0),
            g,
            Color32::WHITE,
        );
    }

    // ---- the switches
    for (k, (key, _, labs)) in SWITCH_ROWS.iter().enumerate() {
        let sr = switch_rect(k).translate(o);
        let i = at(key);
        let on = ed.vals[i] >= 0.5;
        // Mid-Side needs a stereo pair.
        let live = *key != "ms" || channels == 2;
        let resp = ui.interact(
            sr.expand(4.0),
            base.with(("sw", k)),
            if live { Sense::click() } else { Sense::hover() },
        );
        let mut now = on;
        if live && resp.clicked() {
            now = !on;
            ed.set(i, if now { 1.0 } else { 0.0 });
            ed.released = true;
        }
        switch(
            &painter,
            ui,
            sr,
            now,
            *labs,
            resp.hovered() && live,
            live,
            &th,
        );
        let tip = match *key {
            "sc" => "Sidechain filter: the detector ignores the deep bass, so it does not pump",
            "sat" => "An extra soft clipper after the gain cell (Shape sets its drive)",
            "detect" => "Independent: each channel compresses on its own.\nLinked: every channel follows the loudest, so the image holds still",
            "ms" if live => "Run the two circuits on the mid and side of the stereo pair",
            "ms" => "Mid-Side works on a stereo pair only",
            _ => "Oversampling for the valve and saturator (changes the latency)",
        };
        resp.on_hover_text(tip);
    }

    // ---- the meters
    let now = ui.input(|i| i.time);
    let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
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
    let k_needle = 1.0 - (-dt / 0.09).exp();
    // 0 on the meter is -18 dBFS (RMS) at the input; gain reduction
    // rests at 0 and swings down the scale.
    let targets = if running {
        [reading.loudest_input() + 18.0, -reading.most_gr()]
    } else {
        [-60.0, 0.0]
    };
    for (n, db) in targets.iter().enumerate() {
        let target = vu_t(*db);
        let before = m.needle[n];
        m.needle[n] += (target - m.needle[n]) * k_needle;
        if (m.needle[n] - before).abs() > 0.0005 || (target - m.needle[n]).abs() > 0.002 {
            moving = true;
        }
    }
    {
        let vr = vu_rect().translate(o);
        let face = vu_face(vr);
        let (piv, rad) = vu_pivot(vr);
        let clip = painter.with_clip_rect(face);
        let cols = [
            Color32::from_rgb(28, 22, 18),
            Color32::from_rgb(178, 34, 26),
        ];
        // GR first, so the input needle sits on top.
        for n in [1, 0] {
            let a = vu_angle(m.needle[n]);
            clip.line_segment(
                [polar(piv, 18.0, a), polar(piv, rad - 12.0, a)],
                sk(if n == 0 { 1.8 } else { 1.5 }, cols[n]),
            );
        }
        // The bezel's lower lip hides the pivot.
        painter.rect_filled(
            Rect::from_min_max(pos2(face.left(), face.bottom() - 8.0), face.max),
            Rounding::ZERO,
            ca(th.background, 110),
        );
    }

    // ---- the per-channel strip
    {
        let sr = strip_rect().translate(o);
        let inner = sr.shrink(6.0);
        let names = channel_names(&reading, channels);
        let n = channels.min(tm::MAX_CH);
        let k_bar = 1.0 - (-dt / 0.07).exp();
        for c_ in 0..n {
            let t = if running { reading.gr_db[c_] } else { 0.0 };
            let b = m.bars[c_];
            m.bars[c_] += (t - b) * k_bar;
            if (m.bars[c_] - b).abs() > 0.01 {
                moving = true;
            }
        }
        if n <= 2 {
            // One or two channels: readouts, big enough to read across
            // the room.
            let head = text(ui, "IN  /  GR", 8.5, c(th.text_dim));
            painter.galley(inner.min, head, Color32::WHITE);
            let rows = n.max(1) as f32;
            let rh = (inner.height() - 14.0) / rows;
            for c_ in 0..n.max(1) {
                let y = inner.top() + 14.0 + rh * c_ as f32;
                let name = names.get(c_).map(String::as_str).unwrap_or("M");
                let g = text(ui, name, 15.0, c(th.accent));
                painter.galley(
                    pos2(inner.left(), y + (rh - g.size().y) / 2.0),
                    g,
                    Color32::WHITE,
                );
                let input = if running {
                    format!("{:.1}", reading.input_db[c_].max(-99.0))
                } else {
                    "--".into()
                };
                let gin = text(ui, &input, 11.0, c(th.text));
                painter.galley(
                    pos2(
                        inner.right() - gin.size().x,
                        y + rh / 2.0 - gin.size().y - 1.0,
                    ),
                    gin,
                    Color32::WHITE,
                );
                let gr = if m.bars[c_] < 0.05 {
                    "0.0".to_string()
                } else {
                    format!("-{:.1}", m.bars[c_])
                };
                let ggr = text(ui, &gr, 11.0, Color32::from_rgb(214, 70, 52));
                painter.galley(
                    pos2(inner.right() - ggr.size().x, y + rh / 2.0 + 1.0),
                    ggr,
                    Color32::WHITE,
                );
            }
        } else {
            // A bed: one gain-reduction bar per channel, 0 to 20 dB.
            let head = text(ui, "GR  dB", 8.5, c(th.text_dim));
            painter.galley(inner.min, head, Color32::WHITE);
            let top = inner.top() + 14.0;
            let rh = ((inner.bottom() - top) / n as f32).min(18.0);
            let lab_w = 26.0;
            for c_ in 0..n {
                let y = top + rh * c_ as f32;
                let name = names.get(c_).map(String::as_str).unwrap_or("?");
                let g = text(ui, name, (rh * 0.62).clamp(7.0, 9.5), c(th.text_dim));
                painter.galley(
                    pos2(inner.left(), y + (rh - g.size().y) / 2.0),
                    g,
                    Color32::WHITE,
                );
                let track = Rect::from_min_max(
                    pos2(inner.left() + lab_w, y + rh * 0.22),
                    pos2(inner.right(), y + rh * 0.78),
                );
                let passed = c_ < 32 && reading.passed & (1 << c_) != 0;
                painter.rect_filled(track, Rounding::same(1.5), ca(th.panel_raised, 220));
                if passed {
                    let g = text(
                        ui,
                        "thru",
                        (rh * 0.55).clamp(6.5, 8.5),
                        ca(th.text_dim, 160),
                    );
                    painter.galley(
                        pos2(track.left() + 3.0, track.center().y - g.size().y / 2.0),
                        g,
                        Color32::WHITE,
                    );
                    continue;
                }
                let f = (m.bars[c_] / 20.0).clamp(0.0, 1.0);
                if f > 0.002 {
                    painter.rect_filled(
                        Rect::from_min_max(
                            track.min,
                            pos2(track.left() + track.width() * f, track.bottom()),
                        ),
                        Rounding::same(1.5),
                        lerp(th.meter_mid, th.meter_high, f),
                    );
                }
            }
        }
    }

    // ---- the loudness and peak bar
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
        let y = loud_row_y(lr, k);
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
    // The most gain reduction, as a number.
    let g = text(
        ui,
        &format!(
            "MAX GR  {:.1} dB",
            if running { reading.most_gr() } else { 0.0 }
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
