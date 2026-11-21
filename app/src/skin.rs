// SPDX-License-Identifier: GPL-3.0-or-later
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
    if crate::retro::on(th) {
        use crate::retro::Edge;
        let e = if sunken { Edge::Well } else { Edge::Button };
        crate::retro::panel(p, rect, None, e);
        return;
    }
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
    if crate::retro::on(th) {
        crate::retro::panel(p, rect, None, crate::retro::Edge::Raised);
        return;
    }
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
        for (d, a) in [(2.4, 20u8), (1.3, 42)] {
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
    // A hot core, paler than the glow, keeps the letters readable.
    let core = if neon.is_some() {
        let [r, g, b, _] = col.to_array();
        let w = |x: u8| (x as f32 + (255.0 - x as f32) * 0.45) as u8;
        Color32::from_rgb(w(r), w(g), w(b))
    } else {
        col
    };
    p.text(at, align, text, font, core);
}

/// The theme's own title, at the start of the menu bar: a picture or a
/// line of text. Returns false when the theme has none.
pub fn title(ui: &mut egui::Ui, th: &Theme) -> bool {
    if let Some(t) = neon_sign(ui.ctx(), &th.title_picture) {
        // A neon sign: drawn at its own size (4 pixels to the point), its
        // glow spilling a little past the room it takes in the bar.
        let size = t.size_vec2() / NEON_SCALE * NEON_ZOOM;
        let room = Vec2::new(size.x - 2.0 * NEON_SPILL_X, size.y - 2.0 * NEON_SPILL_Y);
        let (r, _) = ui.allocate_exact_size(room + Vec2::new(10.0, 0.0), egui::Sense::hover());
        let r = Rect::from_center_size(r.center(), size);
        // The glow may reach over the bar's edges.
        let p = ui.painter().with_clip_rect(ui.ctx().screen_rect());
        // The gas breathes, slowly and barely.
        let fx: Option<crate::hud::Fx> = ui.ctx().data(|d| d.get_temp(egui::Id::new("af27-fx")));
        let k = fx.map_or(1.0, |f| {
            0.96 + 0.04 * (f.t * 1.7).sin() * (f.t * 0.43).cos()
        });
        let v = (k * 255.0) as u8;
        p.image(
            t.id(),
            r,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_rgba_premultiplied(v, v, v, v),
        );
        return true;
    }
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

/// Pixels per point in the neon sign pictures, and how far their glow
/// reaches past the tubes (points) at the sides and top and bottom.
const NEON_SCALE: f32 = 4.0;
/// The sign is shown a little larger than it was drawn (still finer than
/// the screen).
const NEON_ZOOM: f32 = 1.4;
const NEON_SPILL_X: f32 = 13.0;
const NEON_SPILL_Y: f32 = 13.0;

/// The Mall Pastel '95 neon signs (`scripts/make-mall-neon.py`), decoded
/// once and kept on the GPU. Mipmapped, so the 4x picture shrinks cleanly.
pub fn neon_sign(ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
    let bytes: &'static [u8] = match name {
        "mall-day-neon" => include_bytes!("../../assets/themes/mall-day-neon.png"),
        "mall-night-neon" => include_bytes!("../../assets/themes/mall-night-neon.png"),
        // The splash's, four times finer.
        "mall-day-neon-big" => include_bytes!("../../assets/themes/mall-day-neon-big.png"),
        "mall-night-neon-big" => include_bytes!("../../assets/themes/mall-night-neon-big.png"),
        _ => return None,
    };
    let id = egui::Id::new(("af2-neon", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) {
        return t;
    }
    let tex = image::load_from_memory(bytes).ok().map(|img| {
        let img = img.to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        ctx.load_texture(
            format!("neon-{name}"),
            egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
            egui::TextureOptions {
                mipmap_mode: Some(egui::TextureFilter::Linear),
                ..egui::TextureOptions::LINEAR
            },
        )
    });
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// The theme's line at the right of the status bar.
pub fn status_line(ui: &mut egui::Ui, th: &Theme) {
    if th.status_text.is_empty() {
        return;
    }
    let font = FontId::new(13.0, egui::FontFamily::Name(JP.into()));
    let col = c(th.status_color.unwrap_or(th.text_dim));
    // Lit like the sign: the theme's neon, or a soft glow of its own colour.
    let glow = th
        .neon
        .or(th.status_color.filter(|_| !th.columns.is_empty()));
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
        glow,
    );
}

/// The family for bold captions (`retro::caption`): a real bold face where
/// the system has one, else the regular text.
pub const BOLD: &str = "af2-bold";

/// The family for Japanese lines: the system Japanese font first for every
/// character it has, so a line never switches face glyph by glyph.
pub const JP: &str = "af2-jp";

/// True when [`BOLD`] is a real bold face (no need to draw text twice).
pub fn has_bold(ctx: &egui::Context) -> bool {
    let id = egui::Id::new("af2-fonts-installed");
    ctx.data(|d| d.get_temp::<String>(id)).as_deref() == Some("ms-sans-serif")
        && system_font(&["C:\\Windows\\Fonts\\tahomabd.ttf"]).is_some()
}

/// A font file from the system, read once per path list; never shipped.
fn system_font(paths: &[&'static str]) -> Option<&'static [u8]> {
    type Read = (Vec<&'static str>, Option<&'static [u8]>);
    static CACHE: std::sync::Mutex<Vec<Read>> = std::sync::Mutex::new(Vec::new());
    let mut c = CACHE.lock().ok()?;
    if let Some((_, f)) = c.iter().find(|(k, _)| k.as_slice() == paths) {
        return *f;
    }
    let f = paths
        .iter()
        .find_map(|p| std::fs::read(p).ok())
        .map(|b| &*Box::leak(b.into_boxed_slice()));
    c.push((paths.to_vec(), f));
    f
}

/// The fonts for a theme: egui's own, the theme's font (if any) in front
/// of them for proportional text, and a system Japanese font behind as a
/// fallback for every text style. Builds them only when the font changes.
///
/// `ms-sans-serif` is Windows' own Microsoft Sans Serif (the TrueType
/// MS Sans Serif of 1995), then Tahoma, read from the system at run time.
pub fn install_fonts(ctx: &egui::Context, font: &str) {
    let id = egui::Id::new("af2-fonts-installed");
    if ctx.data(|d| d.get_temp::<String>(id)).as_deref() == Some(font) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, font.to_string()));
    let mut fonts = egui::FontDefinitions::default();
    let mut bold = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if font == "ms-sans-serif" {
        if let Some(b) = system_font(&[
            "C:\\Windows\\Fonts\\micross.ttf",
            "C:\\Windows\\Fonts\\tahoma.ttf",
        ]) {
            fonts
                .font_data
                .insert("ui95".into(), egui::FontData::from_static(b));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "ui95".into());
            bold.insert(0, "ui95".into());
        }
        if let Some(b) = system_font(&["C:\\Windows\\Fonts\\tahomabd.ttf"]) {
            fonts
                .font_data
                .insert("ui95-bold".into(), egui::FontData::from_static(b));
            bold.insert(0, "ui95-bold".into());
        }
    }
    if let Some((bytes, index)) = cjk_font() {
        let mut data = egui::FontData::from_static(bytes);
        data.index = index;
        fonts.font_data.insert("cjk".into(), data);
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("cjk".into());
        }
        bold.push("cjk".into());
    }
    // Symbol fallbacks (arrows, shapes, Ø) for systems with no Japanese
    // font that has them, as on many Linux installs. Last, so they change
    // nothing where the fonts above already have the glyph.
    for (k, paths) in SYMBOL_FONTS.iter().enumerate() {
        if let Some(b) = system_font(paths) {
            let name = format!("symbols-{k}");
            fonts
                .font_data
                .insert(name.clone(), egui::FontData::from_static(b));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(name.clone());
            }
            bold.push(name);
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name(BOLD.into()), bold);
    let mut jp = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some(i) = jp.iter().position(|f| f == "cjk") {
        let cjk = jp.remove(i);
        jp.insert(0, cjk);
    }
    fonts.families.insert(egui::FontFamily::Name(JP.into()), jp);
    ctx.set_fonts(fonts);
}

/// The text sizes for a theme's font: egui's own, scaled. The 1995 face
/// is set small, as 8 point UI text was.
pub fn text_sizes(
    font: &str,
    scale: f32,
) -> std::collections::BTreeMap<egui::TextStyle, egui::FontId> {
    let mut sizes = egui::Style::default().text_styles;
    for (style, f) in sizes.iter_mut() {
        f.size = (f.size * scale).clamp(6.0, 40.0);
        if font == "ms-sans-serif" && f.family == egui::FontFamily::Proportional {
            f.size = match style {
                egui::TextStyle::Heading => 16.0,
                egui::TextStyle::Small => 10.0,
                _ => 11.0,
            } * scale;
        }
    }
    sizes
}

/// System fonts with arrows and other symbols, each tried in order of
/// its list (Fedora, Debian/Ubuntu, Arch, macOS).
const SYMBOL_FONTS: &[&[&str]] = &[
    &[
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    ],
    &[
        "/usr/share/fonts/google-noto-sans-symbols-2-fonts/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
        "/System/Library/Fonts/Apple Symbols.ttf",
    ],
];

/// A system Japanese font, read once.
fn cjk_font() -> Option<(&'static [u8], u32)> {
    static FONT: std::sync::OnceLock<Option<(&'static [u8], u32)>> = std::sync::OnceLock::new();
    *FONT.get_or_init(|| {
        let (bytes, index) = read_cjk_font()?;
        Some((Box::leak(bytes.into_boxed_slice()), index))
    })
}

/// Reads a system Japanese font, so themes' Japanese titles (and Japanese
/// file names) show. MS Gothic is what Audiform's Mall themes asked for
/// first.
fn read_cjk_font() -> Option<(Vec<u8>, u32)> {
    let candidates = [
        ("C:\\Windows\\Fonts\\msgothic.ttc", 0),
        ("C:\\Windows\\Fonts\\YuGothM.ttc", 0),
        ("C:\\Windows\\Fonts\\meiryo.ttc", 0),
        ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
        ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc", 0),
    ];
    candidates
        .into_iter()
        .find_map(|(path, index)| std::fs::read(path).ok().map(|b| (b, index)))
}
