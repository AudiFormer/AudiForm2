// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Theme styling beyond colours: what made the Mall Pastel '95 themes (and
//! the secret ones) themselves in Audiform, drawn for AudiForm 2.
//!
//! - marble columns down both edges of the window
//! - a lavender and mint checkerboard strip over the menu bar, a Greek key
//!   band under it
//! - bars and tab strips on marble
//! - raised bevelled panels with a hard shadow
//! - the sun and palm (or the moon) behind the editor's lanes
//! - a theme's own title in the menu bar and its line in the status bar
//!
//! Everything here is driven by fields of [`Theme`], so a theme file can
//! use any of it.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use crate::theme::{c, ca, Rgb, Theme};

/// Width of a column at the window's edge, in points.
pub const COLUMN: f32 = 26.0;
/// Height of the Greek key band.
pub const MEANDER: f32 = 14.0;

fn art(ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
    crate::pictures::texture(ctx, name, f32::MAX)
}

/// Draws picture `name` tiled across `rect`, `tile` points wide.
pub fn tile(p: &Painter, rect: Rect, name: &str, tile: Vec2, tint: Color32) {
    if let Some(t) = art(p.ctx(), name) {
        let uv = Rect::from_min_max(
            Pos2::ZERO,
            Pos2::new(
                rect.width() / tile.x.max(1.0),
                rect.height() / tile.y.max(1.0),
            ),
        );
        p.image(t.id(), rect, uv, tint);
    }
}

fn picture(p: &Painter, rect: Rect, name: &str, tint: Color32) {
    if let Some(t) = art(p.ctx(), name) {
        p.image(
            t.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    }
}

/// The marble columns at both edges, as side panels the full height of
/// the window. Call before any other panel.
pub fn columns(ctx: &egui::Context, th: &Theme) {
    if th.columns.is_empty() {
        return;
    }
    for left in [true, false] {
        let panel = if left {
            egui::SidePanel::left("skin-column-left")
        } else {
            egui::SidePanel::right("skin-column-right")
        };
        panel
            .exact_width(COLUMN)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let r = ui.max_rect();
                let p = ui.painter();
                let k = &if th.realistic {
                    format!("{}-real", th.columns)
                } else {
                    th.columns.clone()
                };
                let w = r.width();
                let cap = Rect::from_min_size(r.min, Vec2::new(w, w * 30.0 / 26.0));
                let base_h = w * 24.0 / 26.0;
                let base = Rect::from_min_max(Pos2::new(r.left(), r.bottom() - base_h), r.max);
                let shaft = Rect::from_min_max(
                    Pos2::new(r.left(), cap.bottom()),
                    Pos2::new(r.right(), base.top()),
                );
                tile(
                    p,
                    shaft,
                    &format!("{k}-shaft"),
                    Vec2::new(w, w * 12.0 / 26.0),
                    Color32::WHITE,
                );
                picture(p, cap, &format!("{k}-capital"), Color32::WHITE);
                picture(p, base, &format!("{k}-base"), Color32::WHITE);
            });
    }
}

/// Height of the checkerboard strip: real tile stands taller than the
/// flat drawing.
pub fn checker_height(th: &Theme) -> f32 {
    if th.realistic && !th.columns.is_empty() {
        20.0
    } else {
        12.0
    }
}

/// The checkerboard strip: squares of the theme's two checker colours.
pub fn checker(p: &Painter, rect: Rect, th: &Theme) {
    let Some([a, b]) = th.checker else { return };
    if th.realistic && !th.columns.is_empty() {
        // Glazed ceramic: two rows of tiles, each 10 points square.
        let name = format!("{}-tiles", th.columns);
        if crate::pictures::bytes(&name).is_some() {
            p.rect_filled(rect, 0.0, c(b));
            tile(
                p,
                rect,
                &name,
                Vec2::new(160.0, rect.height()),
                Color32::WHITE,
            );
            return;
        }
    }
    p.rect_filled(rect, 0.0, c(b));
    let s = rect.height() / 2.0;
    let mut row = 0;
    let mut y = rect.top();
    while y < rect.bottom() - 0.5 {
        let mut x = rect.left() + if row % 2 == 0 { 0.0 } else { s };
        while x < rect.right() {
            let q = Rect::from_min_size(Pos2::new(x, y), Vec2::splat(s)).intersect(rect);
            p.rect_filled(q, 0.0, c(a));
            x += 2.0 * s;
        }
        y += s;
        row += 1;
    }
}

/// The Greek key band under the menu bar, ruled above and below.
pub fn meander(p: &Painter, rect: Rect, th: &Theme) {
    if th.columns.is_empty() {
        return;
    }
    p.rect_filled(rect, 0.0, c(th.meander_bg.unwrap_or(th.panel_raised)));
    let h = rect.height();
    tile(
        p,
        rect,
        &format!("{}-meander", th.columns),
        Vec2::new(h * 24.0 / 14.0, h),
        Color32::WHITE,
    );
    let s = Stroke::new(1.0_f32, c(th.border));
    p.line_segment([rect.left_top(), rect.right_top()], s);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], s);
}

/// A bar's (or tab strip's) marble, if the theme has one.
pub fn bar(p: &Painter, rect: Rect, th: &Theme) {
    if th.bar_picture.is_empty() {
        return;
    }
    p.rect_filled(rect, 0.0, c(th.panel_raised));
    tile(p, rect, &th.bar_picture, Vec2::splat(400.0), Color32::WHITE);
}

/// A two-tone bevel: light above and left, dark below and right (sunken
/// swaps them), 2 points wide, as on mid-90s desktops.
pub fn bevel(p: &Painter, rect: Rect, th: &Theme, sunken: bool) {
    let Some([hi, lo, _]) = th.bevel else { return };
    let (tl, br) = if sunken { (lo, hi) } else { (hi, lo) };
    let r = rect.shrink(1.0);
    let s1 = Stroke::new(2.0_f32, c(tl));
    let s2 = Stroke::new(2.0_f32, c(br));
    p.add(Shape::line(
        vec![r.left_bottom(), r.left_top(), r.right_top()],
        s1,
    ));
    p.add(Shape::line(
        vec![r.right_top(), r.right_bottom(), r.left_bottom()],
        s2,
    ));
}

/// A panel's raised bevel and its hard one-point shadow.
pub fn panel(p: &Painter, rect: Rect, th: &Theme) {
    let Some([_, _, shadow]) = th.bevel else {
        return;
    };
    let s = Stroke::new(1.0_f32, c(shadow));
    p.line_segment(
        [
            rect.right_top() + Vec2::new(1.0, 1.0),
            rect.right_bottom() + Vec2::new(1.0, 1.0),
        ],
        s,
    );
    p.line_segment(
        [
            rect.left_bottom() + Vec2::new(1.0, 1.0),
            rect.right_bottom() + Vec2::new(1.0, 1.0),
        ],
        s,
    );
    bevel(p, rect, th, false);
}

/// The sun and palm (or moon) behind the editor's lanes: top right, faint.
pub fn lane_art(p: &Painter, editor: Rect, th: &Theme) {
    if th.lane_art.is_empty() || editor.width() < 420.0 || editor.height() < 200.0 {
        return;
    }
    let size = Vec2::new(260.0, 220.0);
    let r = Rect::from_min_size(
        Pos2::new(editor.right() - 40.0 - size.x, editor.top() + 16.0 + 26.0),
        size,
    );
    picture(p, r, &th.lane_art, Color32::from_white_alpha(110));
}

/// Text with a soft neon halo when the theme has one.
fn neon_text(
    p: &Painter,
    at: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    col: Color32,
    neon: Option<Rgb>,
) {
    if let Some(n) = neon {
        for (d, a) in [(2.5, 40u8), (1.2, 80)] {
            for v in [
                Vec2::new(d, 0.0),
                Vec2::new(-d, 0.0),
                Vec2::new(0.0, d),
                Vec2::new(0.0, -d),
            ] {
                p.text(at + v, align, text, font.clone(), ca(n, a));
            }
        }
    }
    p.text(at, align, text, font, col);
}

/// The theme's own title, at the start of the menu bar: a picture or a
/// line of text. Returns false when the theme has none.
pub fn title(ui: &mut egui::Ui, th: &Theme) -> bool {
    if !th.title_picture.is_empty() {
        if let Some(t) = art(ui.ctx(), &th.title_picture) {
            let [w, h] = t.size_vec2().into();
            let height = 20.0;
            let size = Vec2::new(height * w / h.max(1.0), height);
            let (r, _) = ui.allocate_exact_size(size + Vec2::new(16.0, 0.0), egui::Sense::hover());
            let r = Rect::from_center_size(r.center(), size);
            ui.painter().image(
                t.id(),
                r,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            return true;
        }
    }
    if th.title_text.is_empty() {
        return false;
    }
    let font = FontId::proportional(14.0);
    let col = c(th.title_color.unwrap_or(th.accent));
    let galley = ui
        .painter()
        .layout_no_wrap(th.title_text.clone(), font.clone(), col);
    let (r, _) = ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 0.0), egui::Sense::hover());
    let p = ui.painter();
    let at = r.left_center() + Vec2::new(10.0, 0.0);
    // Heavy: the title is drawn twice, a hair apart.
    neon_text(
        p,
        at,
        Align2::LEFT_CENTER,
        &th.title_text,
        font.clone(),
        col,
        th.neon,
    );
    p.text(
        at + Vec2::new(0.6, 0.0),
        Align2::LEFT_CENTER,
        &th.title_text,
        font,
        col,
    );
    true
}

/// The theme's line at the right of the status bar.
pub fn status_line(ui: &mut egui::Ui, th: &Theme) {
    if th.status_text.is_empty() {
        return;
    }
    let font = FontId::proportional(12.0);
    let col = c(th.status_color.unwrap_or(th.text_dim));
    let galley = ui
        .painter()
        .layout_no_wrap(th.status_text.clone(), font.clone(), col);
    let (r, _) = ui.allocate_exact_size(galley.size() + Vec2::new(12.0, 0.0), egui::Sense::hover());
    neon_text(
        ui.painter(),
        r.right_center() - Vec2::new(4.0, 0.0),
        Align2::RIGHT_CENTER,
        &th.status_text,
        font,
        col,
        th.neon,
    );
}

/// Adds a system Japanese font as a fallback for every text style, so
/// themes' Japanese titles (and Japanese file names) show. MS Gothic is
/// what Audiform's Mall themes asked for first.
pub fn install_fallback_fonts(ctx: &egui::Context) {
    let candidates = [
        ("C:\\Windows\\Fonts\\msgothic.ttc", 0),
        ("C:\\Windows\\Fonts\\YuGothM.ttc", 0),
        ("C:\\Windows\\Fonts\\meiryo.ttc", 0),
        ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
        ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
    ];
    for (path, index) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            let mut data = egui::FontData::from_owned(bytes);
            data.index = index;
            fonts.font_data.insert("cjk".into(), data);
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push("cjk".into());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}
