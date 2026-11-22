// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Caustics: the moving light on the floor of shallow water, drawn over a
//! still picture without a single animation frame. Two cellular (Voronoi)
//! patterns drift slowly in different directions; only the edges of their
//! cells are lit. The cells' points also sway a little, so the net of light
//! bends and breaks up as it moves.
//!
//! The light is heat-mapped, as on a real pool floor where refracted sun
//! focuses: each line, each junction where three cells meet, and each place
//! where the two nets cross adds to one summed intensity, and that sum is
//! coloured through a ramp built from the theme (deep blue from `accent`,
//! then `accent`, then a pale mix of `accent_alt` and the highlight, then
//! near white). A lone thin line stays dim and cool; where lines crowd or
//! cross it glows brighter and warmer.
//!
//! On wgpu the whole thing is one full-screen triangle and a small shader
//! (a paint callback): the edges come from the distance to the nearest and
//! second-nearest cell point (F2 - F1), worked out per pixel. On OpenGL a
//! cheap copy runs on the CPU at a fraction of the window's resolution,
//! built a few rows a frame (under a millisecond) and stretched over it.
//!
//! Masked as in pixel-art water: a lone line on its own is masked away,
//! so only the pools where lines cross and gather show; the light comes in
//! a few hard steps and in chunky pixels; and on wgpu it is laid over the
//! picture by multiplying (light x what is beneath, added), so it lands on
//! the bright, shallow parts of the water and fades over the dark, deep
//! ones, following the picture. The CPU copy can only add, so it skips
//! that last part.

use std::num::NonZeroU64;

use eframe::egui_wgpu::{self, wgpu};
use egui::{Color32, ColorImage, Painter, Pos2, Rect, TextureHandle, TextureOptions};

use crate::theme::{Rgb, Theme};

/// Points per cell of the two patterns (aqua, gold).
const SCALE: [f32; 2] = [85.0, 62.0];
/// How far each pattern drifts, in cells per unit of animation time.
const DRIFT: [[f32; 2]; 2] = [[0.46, 0.18], [-0.30, -0.40]];
/// How fast each pattern's points sway, and where they start.
const SWAY: [[f32; 2]; 2] = [[0.6, 0.0], [0.75, 3.0]];
/// The patterns' hash seeds.
const SEED: [u32; 2] = [1, 2];
/// The CPU patterns repeat every this many cells (4000+ points: unseen).
const PERIOD: i32 = 32;
/// The highlight where both nets cross: near-white aqua.
const HIGHLIGHT: Rgb = [214, 250, 244];

/// Uniform buffer slots: one per place that draws caustics in a frame
/// (all callbacks are prepared before any is painted, so each needs its own).
pub const SLOT_BACKDROP: u32 = 0;
pub const SLOT_SPLASH: u32 = 1;
const SLOTS: u64 = 4;
/// Bytes of uniforms, as laid out by [`pack`] and `struct U` in the shader.
const UNIFORM_BYTES: u64 = 80;

/// The fragment shader works the same sums as [`shade`]; keep them in step.
const SHADER: &str = r#"
struct U {
    origin: vec2<f32>,
    ppp: f32,
    time: f32,
    strength: f32,
    pixel: f32,
    srgb: f32,
    pad: f32,
    col_a: vec4<f32>,
    col_b: vec4<f32>,
    col_hi: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

fn hash2(cx: i32, cy: i32, seed: u32) -> vec2<f32> {
    var h = (bitcast<u32>(cx) * 1597334673u) ^ (bitcast<u32>(cy) * 3812015801u) ^ (seed * 2654435761u);
    h = h ^ (h >> 16u);
    h = h * 2246822519u;
    h = h ^ (h >> 13u);
    h = h * 3266489917u;
    h = h ^ (h >> 16u);
    var g = h * 747796405u + 2891336453u;
    g = g ^ (g >> 16u);
    return vec2<f32>(f32(h & 0xffffu), f32((g >> 8u) & 0xffffu)) / 65535.0;
}

fn point(cx: i32, cy: i32, seed: u32, phase: f32) -> vec2<f32> {
    let h = hash2(cx, cy, seed);
    return vec2<f32>(
        0.5 + 0.38 * sin(phase + 6.2831853 * h.x),
        0.5 + 0.38 * sin(phase * 0.8 + 6.2831853 * h.y),
    );
}

// (F2 - F1, F3 - F1): the first is zero on a cell's edge, growing towards
// its middle; the second is zero only where three cells meet.
fn edge(p: vec2<f32>, seed: u32, phase: f32) -> vec2<f32> {
    let ip = floor(p);
    let fp = p - ip;
    let ix = i32(ip.x);
    let iy = i32(ip.y);
    var d1 = 8.0;
    var d2 = 8.0;
    var d3 = 8.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let r = vec2<f32>(f32(i), f32(j)) + point(ix + i, iy + j, seed, phase) - fp;
            let d = dot(r, r);
            if (d < d1) {
                d3 = d2;
                d2 = d1;
                d1 = d;
            } else if (d < d2) {
                d3 = d2;
                d2 = d;
            } else if (d < d3) {
                d3 = d;
            }
        }
    }
    let s1 = sqrt(d1);
    return vec2<f32>(sqrt(d2) - s1, sqrt(d3) - s1);
}

fn line(e: f32, fw: f32) -> f32 {
    return 1.0 - smoothstep(0.5 * fw, 1.5 * fw, e) + 0.25 / (1.0 + e * e * 250.0);
}

fn junction(e3: f32) -> f32 {
    return 1.0 / (1.0 + e3 * e3 * 110.0);
}

// The summed light: each net's lines and junctions, and the crossings.
fn energy(la: f32, lb: f32, ja: f32, jb: f32) -> f32 {
    let cross = smoothstep(0.15, 0.55, la * lb);
    return 0.42 * la + 0.38 * lb + 0.7 * (ja + jb) + 0.7 * cross;
}

// The mask: lone lines (summed light under about 0.6) drop out; what
// is left comes in four hard steps, as in pixel-art water.
fn masked(e: f32) -> f32 {
    let m = max(e - 0.6, 0.0) * 1.8;
    return floor(m * 4.0 + 0.5) / 4.0;
}

// The heat ramp: off, deep blue, accent, pale warm, near white.
fn ramp(e: f32) -> vec3<f32> {
    let deep = u.col_a.rgb * vec3<f32>(0.35, 0.55, 1.0) * 0.55;
    let mid = u.col_a.rgb * 0.8;
    let warm = mix(u.col_b.rgb, u.col_hi.rgb, 0.4);
    let hot = mix(u.col_hi.rgb, vec3<f32>(1.0), 0.5) * 1.15;
    if (e < 0.35) {
        return deep * (e / 0.35);
    } else if (e < 0.75) {
        return mix(deep, mid, (e - 0.35) / 0.4);
    } else if (e < 1.2) {
        return mix(mid, warm, (e - 0.75) / 0.45);
    }
    return mix(warm, hot, min((e - 1.2) / 0.8, 1.0));
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var px = pos.xy - u.origin;
    if (u.pixel > 0.0) {
        let s = u.pixel * u.ppp;
        px = (floor(px / s) + 0.5) * s;
    }
    let q = px / u.ppp;
    let t = u.time;
    let w = q + vec2<f32>(6.0 * sin(q.y * 0.032 + t * 0.9), 6.0 * cos(q.x * 0.026 - t * 0.7));
    let pa = w / 85.0 + vec2<f32>(0.46, 0.18) * t;
    let pb = w / 62.0 + vec2<f32>(-0.30, -0.40) * t + vec2<f32>(17.0, 5.0);
    let ea = edge(pa, 1u, t * 0.6);
    let eb = edge(pb, 2u, t * 0.75 + 3.0);
    // Lines about 1.5 device pixels wide, whatever the scale. F2 - F1 grows
    // at most 2 cells per cell walked, so a pixel spans 2 / (cell px); the
    // footprint is worked out, not taken from fwidth(), which cancels out
    // where a 2x2 pixel quad straddles an edge (the lines came out dashed).
    // Drawn in chunky pixels, a line is one chunk wide.
    let px_pts = select(1.0 / u.ppp, u.pixel, u.pixel > 0.0);
    let fa = 2.0 * px_pts / 85.0;
    let fb = 2.0 * px_pts / 62.0;
    let e = masked(energy(line(ea.x, fa), line(eb.x, fb), junction(ea.y), junction(eb.y)));
    var c = ramp(e) * u.strength;
    c = min(c, vec3<f32>(1.0));
    if (u.srgb > 0.5) {
        c = pow(c, vec3<f32>(2.2));
    }
    return vec4<f32>(c, 0.0);
}
"#;

// ------------------------------------------------------------ The maths

/// A small integer hash: two values in 0..1 for a cell (same as the shader).
fn hash2(cx: i32, cy: i32, seed: u32) -> [f32; 2] {
    let mut h = (cx as u32).wrapping_mul(1_597_334_673)
        ^ (cy as u32).wrapping_mul(3_812_015_801)
        ^ seed.wrapping_mul(2_654_435_761);
    h ^= h >> 16;
    h = h.wrapping_mul(2_246_822_519);
    h ^= h >> 13;
    h = h.wrapping_mul(3_266_489_917);
    h ^= h >> 16;
    let mut g = h.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    g ^= g >> 16;
    [
        (h & 0xffff) as f32 / 65535.0,
        ((g >> 8) & 0xffff) as f32 / 65535.0,
    ]
}

/// Where a cell's point sits inside it (0..1 each way) at `phase`.
fn point(cx: i32, cy: i32, seed: u32, phase: f32) -> [f32; 2] {
    let h = hash2(cx, cy, seed);
    let tau = std::f32::consts::TAU;
    [
        0.5 + 0.38 * (phase + tau * h[0]).sin(),
        0.5 + 0.38 * (phase * 0.8 + tau * h[1]).sin(),
    ]
}

/// One cellular pattern at one moment, repeating every `n` cells each way.
pub struct Layer {
    n: i32,
    pts: Vec<[f32; 2]>,
}

impl Layer {
    pub fn new(n: i32, seed: u32, phase: f32) -> Layer {
        let n = n.max(1);
        let mut pts = Vec::with_capacity((n * n) as usize);
        for cy in 0..n {
            for cx in 0..n {
                pts.push(point(cx, cy, seed, phase));
            }
        }
        Layer { n, pts }
    }

    /// F2 - F1 at (x, y) in cells: 0 on an edge, larger inside a cell.
    #[cfg(test)]
    pub fn edge(&self, x: f32, y: f32) -> f32 {
        self.edges(x, y)[0]
    }

    /// (F2 - F1, F3 - F1) at (x, y) in cells: the second is 0 only where
    /// three cells meet (a junction of the net).
    pub fn edges(&self, x: f32, y: f32) -> [f32; 2] {
        let (fx0, fy0) = (x.floor(), y.floor());
        let (fx, fy) = (x - fx0, y - fy0);
        let (ix, iy) = (fx0 as i32, fy0 as i32);
        let n = self.n;
        let (mut d1, mut d2, mut d3) = (8.0_f32, 8.0_f32, 8.0_f32);
        for j in -1..=1 {
            let cy = (iy + j).rem_euclid(n);
            for i in -1..=1 {
                let cx = (ix + i).rem_euclid(n);
                let o = self.pts[(cy * n + cx) as usize];
                let rx = i as f32 + o[0] - fx;
                let ry = j as f32 + o[1] - fy;
                let d = rx * rx + ry * ry;
                if d < d1 {
                    d3 = d2;
                    d2 = d1;
                    d1 = d;
                } else if d < d2 {
                    d3 = d2;
                    d2 = d;
                } else if d < d3 {
                    d3 = d;
                }
            }
        }
        let s1 = d1.sqrt();
        [d2.sqrt() - s1, d3.sqrt() - s1]
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How brightly a net's line shows at edge distance `e`: a crisp core and
/// a soft halo.
fn line(e: f32, fw: f32) -> f32 {
    1.0 - smoothstep(0.5 * fw, 1.5 * fw, e) + 0.25 / (1.0 + e * e * 250.0)
}

/// A junction's glow at F3 - F1 = `e3`: where three cells meet, lines
/// converge and the light pools.
fn junction(e3: f32) -> f32 {
    1.0 / (1.0 + e3 * e3 * 110.0)
}

/// The summed light at one point (the heat): each net's line strength
/// `la`/`lb`, its junction glow `ja`/`jb`, and a boost where the nets cross.
/// Additive, so wherever lines crowd the sum climbs.
fn energy(la: f32, lb: f32, ja: f32, jb: f32) -> f32 {
    let cross = smoothstep(0.15, 0.55, la * lb);
    0.42 * la + 0.38 * lb + 0.7 * (ja + jb) + 0.7 * cross
}

/// The mask (as the shader's `masked`): lone lines drop out, the rest in
/// four hard steps.
fn masked(e: f32) -> f32 {
    let m = (e - 0.6).max(0.0) * 1.8;
    (m * 4.0 + 0.5).floor() / 4.0
}

fn mix3(x: [f32; 3], y: [f32; 3], t: f32) -> [f32; 3] {
    [
        x[0] + (y[0] - x[0]) * t,
        x[1] + (y[1] - x[1]) * t,
        x[2] + (y[2] - x[2]) * t,
    ]
}

/// The heat ramp from summed light `e` to colour, from the theme's two
/// colours: off, deep blue, `a`, a pale warm mix of `b` and the highlight,
/// then near white (as the shader's `ramp`).
fn ramp(e: f32, a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let h = rgbf(HIGHLIGHT);
    let deep = [a[0] * 0.35 * 0.55, a[1] * 0.55 * 0.55, a[2] * 0.55];
    let mid = [a[0] * 0.8, a[1] * 0.8, a[2] * 0.8];
    let warm = mix3(b, h, 0.4);
    let hot = mix3(h, [1.0; 3], 0.5).map(|v| v * 1.15);
    if e < 0.35 {
        deep.map(|v| v * (e / 0.35).max(0.0))
    } else if e < 0.75 {
        mix3(deep, mid, (e - 0.35) / 0.4)
    } else if e < 1.2 {
        mix3(mid, warm, (e - 0.75) / 0.45)
    } else {
        mix3(warm, hot, ((e - 1.2) / 0.8).min(1.0))
    }
}

/// The light at one point from the two nets' line strengths and junction
/// glows, 0..1 per channel.
/// `cover` (0..1) dims lines drawn wider than they are (the CPU copy's
/// coarse pixels) to the light a fine line would give; 1 on the GPU.
#[allow(clippy::too_many_arguments)]
fn shade(
    la: f32,
    lb: f32,
    ja: f32,
    jb: f32,
    a: [f32; 3],
    b: [f32; 3],
    strength: f32,
    cover: f32,
) -> [f32; 3] {
    let e = masked(energy(la, lb, ja, jb));
    // Coarse pixels widen lines, so they are dimmed to a fine line's light;
    // the pools where light gathers are broader, so they keep more of theirs.
    let k = (cover * (1.0 + 2.0 * smoothstep(0.75, 1.6, e))).min(1.0);
    ramp(e, a, b).map(|v| (v * k * strength).min(1.0))
}

fn rgbf(c: Rgb) -> [f32; 3] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    ]
}

/// The shader's uniforms in its layout (`struct U`): 80 bytes.
#[allow(clippy::too_many_arguments)]
fn pack(
    origin_px: [f32; 2],
    ppp: f32,
    time: f32,
    strength: f32,
    pixel: f32,
    srgb: bool,
    a: Rgb,
    b: Rgb,
) -> [f32; 20] {
    let (a, b, h) = (rgbf(a), rgbf(b), rgbf(HIGHLIGHT));
    [
        origin_px[0],
        origin_px[1],
        ppp,
        time,
        strength,
        pixel,
        if srgb { 1.0 } else { 0.0 },
        0.0,
        a[0],
        a[1],
        a[2],
        1.0,
        b[0],
        b[1],
        b[2],
        1.0,
        h[0],
        h[1],
        h[2],
        1.0,
    ]
}

fn bytes(u: &[f32; 20]) -> Vec<u8> {
    u.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// What the CPU copy draws: a `w` x `h` image whose pixels are `step`
/// points (x, y) at animation time `t`.
#[derive(Clone, Copy, PartialEq)]
struct Job {
    size: [usize; 2],
    step: [f32; 2],
    /// Device pixels per point.
    ppp: f32,
    t: f32,
    strength: f32,
    a: Rgb,
    b: Rgb,
}

/// Rows `rows` of the caustic light for `job`, appended to `out`: additive,
/// premultiplied. Images can be built a few rows at a time.
fn render_rows(job: &Job, rows: std::ops::Range<usize>, out: &mut Vec<Color32>) {
    let ([w, _], step, t) = (job.size, job.step, job.t);
    let layers = [
        Layer::new(PERIOD, SEED[0], t * SWAY[0][0] + SWAY[0][1]),
        Layer::new(PERIOD, SEED[1], t * SWAY[1][0] + SWAY[1][1]),
    ];
    let n = PERIOD as f32;
    let off = [
        [
            (DRIFT[0][0] * t).rem_euclid(n),
            (DRIFT[0][1] * t).rem_euclid(n),
        ],
        [
            (DRIFT[1][0] * t + 17.0).rem_euclid(n),
            (DRIFT[1][1] * t + 5.0).rem_euclid(n),
        ],
    ];
    // Lines about two of these coarse pixels wide with soft sides, so they
    // stretch smoothly instead of in steps (one pixel spans 2 * pix / SCALE)...
    let pix = step[0].max(step[1]);
    let fw = [2.8 * pix / SCALE[0], 2.8 * pix / SCALE[1]];
    // ...and as bright, overall, as the shader's 1.5 device-pixel lines.
    let cover = (1.1 / (pix * job.ppp)).clamp(0.1, 1.0);
    // The swell bends x by a function of y and y by a function of x, so
    // both are worked out once per row and column.
    let warp_y: Vec<f32> = (0..w)
        .map(|x| 6.0 * (((x as f32 + 0.5) * step[0]) * 0.026 - t * 0.7).cos())
        .collect();
    let (ca, cb) = (rgbf(job.a), rgbf(job.b));
    for y in rows {
        let qy = (y as f32 + 0.5) * step[1];
        let wx = 6.0 * (qy * 0.032 + t * 0.9).sin();
        for (x, wy) in warp_y.iter().enumerate() {
            let qx = (x as f32 + 0.5) * step[0];
            let (px, py) = (qx + wx, qy + wy);
            let ea = layers[0].edges(px / SCALE[0] + off[0][0], py / SCALE[0] + off[0][1]);
            let eb = layers[1].edges(px / SCALE[1] + off[1][0], py / SCALE[1] + off[1][1]);
            let c = shade(
                line(ea[0], fw[0]),
                line(eb[0], fw[1]),
                junction(ea[1]),
                junction(eb[1]),
                ca,
                cb,
                job.strength,
                cover,
            );
            out.push(Color32::from_rgba_premultiplied(
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
                0,
            ));
        }
    }
}

/// A whole image at once (tests).
#[cfg(test)]
fn render_cpu(job: &Job) -> Vec<Color32> {
    let mut out = Vec::new();
    render_rows(job, 0..job.size[1], &mut out);
    out
}

/// Pixels in the CPU copy's image, whatever the window's size.
const CPU_BUDGET: f32 = 64_000.0;
/// Pixels worked out per frame (about 0.8 ms in a release build): a whole
/// image takes a few frames, so the light moves at a lower rate rather than
/// in coarser pixels.
const CPU_PER_FRAME: usize = 8_000;

/// The CPU image's size for a `w` x `h` point area: at least 3 points a
/// pixel, more on big windows to stay within [`CPU_BUDGET`].
fn cpu_size(w: f32, h: f32) -> [usize; 2] {
    let step = (w * h / CPU_BUDGET).sqrt().max(3.0);
    [
        ((w / step).ceil() as usize).max(1),
        ((h / step).ceil() as usize).max(1),
    ]
}

// ------------------------------------------------------------- Drawing

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    buf: wgpu::Buffer,
    stride: u64,
    srgb: bool,
}

fn gpu_id() -> egui::Id {
    egui::Id::new("af2-caustics-gpu")
}

/// Builds the shader once, at start, when the renderer is wgpu.
pub fn install(cc: &eframe::CreationContext) {
    let ok =
        cc.wgpu_render_state
            .as_ref()
            .is_some_and(|rs| match build(&rs.device, rs.target_format) {
                Some(gpu) => {
                    rs.renderer.write().callback_resources.insert(gpu);
                    true
                }
                None => false,
            });
    cc.egui_ctx.data_mut(|d| d.insert_temp(gpu_id(), ok));
}

fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Option<Gpu> {
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("af2 caustics"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("af2 caustics"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: NonZeroU64::new(UNIFORM_BYTES),
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("af2 caustics"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("af2 caustics"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs",
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // dst + src x dst: the light takes the picture's own
                // brightness, so dark, deep water stays dark.
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Dst,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    let stride = (device.limits().min_uniform_buffer_offset_alignment as u64).max(256);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("af2 caustics"),
        size: stride * SLOTS,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("af2 caustics"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buf,
                offset: 0,
                size: NonZeroU64::new(UNIFORM_BYTES),
            }),
        }],
    });
    if let Some(e) = poll_now(device.pop_error_scope()) {
        eprintln!("caustics shader unavailable ({e}); drawing them on the CPU");
        return None;
    }
    Some(Gpu {
        pipeline,
        bind,
        buf,
        stride,
        srgb: format.is_srgb(),
    })
}

/// Polls a future once (wgpu's native error scopes are ready at once);
/// a future that is still pending counts as no error.
fn poll_now<F: std::future::Future<Output = Option<wgpu::Error>>>(f: F) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    // SAFETY: the vtable's functions do nothing and never touch the pointer.
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(e) => e,
        Poll::Pending => None,
    }
}

struct Callback {
    slot: u32,
    uniforms: [f32; 20],
}

impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(g) = resources.get::<Gpu>() {
            let mut u = self.uniforms;
            u[6] = if g.srgb { 1.0 } else { 0.0 };
            queue.write_buffer(&g.buf, self.slot as u64 * g.stride, &bytes(&u));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(g) = resources.get::<Gpu>() {
            pass.set_pipeline(&g.pipeline);
            pass.set_bind_group(0, &g.bind, &[(self.slot as u64 * g.stride) as u32]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// The CPU copy for one slot: the image on show, and the next one being
/// built a few rows a frame.
#[derive(Default)]
struct Cpu {
    tex: Option<TextureHandle>,
    shown: Option<Job>,
    job: Option<Job>,
    pending: Vec<Color32>,
    row: usize,
}

type CpuSlot = std::sync::Arc<std::sync::Mutex<Cpu>>;

/// Caustic light over `rect`, at animation time `time` (`Fx::t`), `strength`
/// 0..1. `slot` tells apart the places that draw it in one frame.
pub fn paint(p: &Painter, rect: Rect, th: &Theme, time: f32, strength: f32, slot: u32) {
    if strength <= 0.0 || rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let ctx = p.ctx();
    let gpu = ctx.data(|d| d.get_temp::<bool>(gpu_id())).unwrap_or(false);
    let slot = slot.min(SLOTS as u32 - 1);
    // Chunky pixels always: finer than the pixel themes' own.
    let chunk = if th.pixel { 4.0 } else { 3.0 };
    if gpu {
        let ppp = ctx.pixels_per_point();
        // Multiplied by the picture beneath, the light needs to be stronger
        // to read as brightly on the bright, shallow parts.
        let uniforms = pack(
            [rect.min.x * ppp, rect.min.y * ppp],
            ppp,
            time,
            (strength * 2.6).min(4.0),
            chunk,
            false,
            th.accent,
            th.accent_alt,
        );
        p.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Callback { slot, uniforms },
        ));
        return;
    }

    // OpenGL: an image at a fraction of the window's resolution, built a
    // few rows a frame and stretched over the window.
    let id = egui::Id::new(("af2-caustics-cpu", slot));
    let cell: CpuSlot = ctx.data_mut(|d| d.get_temp_mut_or_default::<CpuSlot>(id).clone());
    let Ok(mut st) = cell.lock() else {
        return;
    };
    let size = if chunk > 0.0 {
        let s = chunk.max(3.0);
        [
            ((rect.width() / s).ceil() as usize).max(1),
            ((rect.height() / s).ceil() as usize).max(1),
        ]
    } else {
        cpu_size(rect.width(), rect.height())
    };
    let want = Job {
        size,
        step: [
            rect.width() / size[0] as f32,
            rect.height() / size[1] as f32,
        ],
        ppp: ctx.pixels_per_point(),
        t: time,
        strength,
        a: th.accent,
        b: th.accent_alt,
    };
    let busy = st.job.is_some_and(|j| j.size == size);
    if !busy && st.shown != Some(want) {
        st.job = Some(want);
        st.pending.clear();
        st.row = 0;
    }
    if let Some(job) = st.job {
        let t0 = std::time::Instant::now();
        let rows = (CPU_PER_FRAME / size[0].max(1)).max(1);
        let end = (st.row + rows).min(size[1]);
        let start = st.row;
        let mut pending = std::mem::take(&mut st.pending);
        render_rows(&job, start..end, &mut pending);
        st.row = end;
        if end >= size[1] {
            let image = ColorImage {
                size,
                pixels: pending,
            };
            match &mut st.tex {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => {
                    st.tex = Some(ctx.load_texture("af2-caustics", image, TextureOptions::LINEAR))
                }
            }
            st.shown = Some(job);
            st.job = None;
            st.row = 0;
        } else {
            st.pending = pending;
            // Finish the image even if nothing else is moving.
            ctx.request_repaint();
        }
        if std::env::var_os("AF2_TRACE").is_some() {
            eprintln!(
                "caustics cpu {}x{} rows {start}..{end} {:.2} ms",
                size[0],
                size[1],
                t0.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    if let Some(t) = &st.tex {
        p.image(
            t.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_tiles_with_the_period() {
        let l = Layer::new(PERIOD, 7, 1.3);
        let n = PERIOD as f32;
        for k in 0..200 {
            let x = (k as f32 * 0.37) % n;
            let y = (k as f32 * 0.61) % n;
            let e = l.edge(x, y);
            for (dx, dy) in [(n, 0.0), (0.0, n), (-n, n), (2.0 * n, -n)] {
                let f = l.edge(x + dx, y + dy);
                assert!((e - f).abs() < 1e-3, "{x},{y}: {e} vs {f}");
            }
        }
    }

    #[test]
    fn edge_is_zero_between_points_and_positive_inside() {
        let l = Layer::new(8, 3, 0.0);
        let mut lo = f32::MAX;
        let mut hi = 0.0_f32;
        for k in 0..4000 {
            let e = l.edge(k as f32 * 0.0137, k as f32 * 0.0071);
            assert!(e >= 0.0);
            lo = lo.min(e);
            hi = hi.max(e);
        }
        // A line through the pattern crosses edges and cell middles.
        assert!(lo < 0.02, "{lo}");
        assert!(hi > 0.3, "{hi}");
    }

    #[test]
    fn points_stay_inside_their_cells() {
        for c in 0..500 {
            for ph in [0.0, 1.0, 2.5, 100.0] {
                let p = point(c * 3 - 700, c * 7 + 11, 2, ph);
                assert!(p.iter().all(|v| (0.1..=0.9).contains(v)), "{p:?}");
            }
        }
    }

    #[test]
    fn overlaps_glow_hotter_and_warmer() {
        let (a, b) = ([0.3, 0.82, 0.89], [0.85, 0.91, 0.29]);
        let (on, off) = (line(0.0, 0.01), line(0.5, 0.01));
        let (j_on, j_off) = (junction(0.0), junction(1.0));
        let s = |la, lb, ja, jb| shade(la, lb, ja, jb, a, b, 1.0, 1.0);
        let none = s(off, off, j_off, j_off);
        let one = s(on, off, j_off, j_off);
        let meet = s(on, off, j_on, j_off);
        let cross = s(on, on, j_off, j_off);
        let hottest = s(on, on, j_on, j_on);
        let sum = |c: [f32; 3]| c.iter().sum::<f32>();
        // Nothing and a lone line are masked away (pixel-art water)...
        assert!(none.iter().all(|v| *v < 0.03), "{none:?}");
        assert!(one.iter().all(|v| *v < 0.03), "{one:?}");
        // ...where lines converge it shows, and crossings are warm.
        assert!(sum(meet) > 0.3, "{meet:?}");
        assert!(sum(cross) > sum(meet), "{cross:?} {meet:?}");
        assert!(cross[0] > 0.6 && cross[0] > one[0] * 3.0, "{cross:?}");
        assert!(hottest.iter().all(|v| *v > 0.9), "{hottest:?}");
        // Junctions glow only near where three cells meet.
        assert!(j_off < 0.02);
        assert_eq!(shade(on, on, j_on, j_on, a, b, 0.0, 1.0), [0.0; 3]);
        // The ramp only climbs.
        let mut last = 0.0;
        for k in 0..=40 {
            let v = sum(ramp(k as f32 * 0.06, a, b));
            assert!(v >= last - 1e-4, "{k}");
            last = v;
        }
    }

    #[test]
    fn junctions_are_where_three_cells_meet() {
        let l = Layer::new(8, 3, 0.4);
        let mut seen = 0;
        for k in 0..20000 {
            let (x, y) = (k as f32 * 0.00377 % 8.0, k as f32 * 0.00611 % 8.0);
            let [e2, e3] = l.edges(x, y);
            assert!(e3 >= e2 - 1e-5, "{x},{y}");
            if e3 < 0.03 {
                seen += 1;
            }
        }
        assert!(seen > 0);
    }

    #[test]
    fn uniforms_match_the_shader_layout() {
        let u = pack(
            [10.0, 20.0],
            1.5,
            3.0,
            0.5,
            4.0,
            true,
            [255, 0, 0],
            [0, 0, 255],
        );
        assert_eq!(bytes(&u).len() as u64, UNIFORM_BYTES);
        assert_eq!(&u[..8], &[10.0, 20.0, 1.5, 3.0, 0.5, 4.0, 1.0, 0.0]);
        // col_a, col_b and col_hi start on 16-byte boundaries.
        assert_eq!(&u[8..12], &[1.0, 0.0, 0.0, 1.0]);
        assert_eq!(&u[12..16], &[0.0, 0.0, 1.0, 1.0]);
        assert_eq!(u[19], 1.0);
        assert_eq!(bytes(&u)[..4], 10.0_f32.to_le_bytes());
    }

    #[test]
    fn shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("validates");
    }

    fn job(t: f32) -> Job {
        Job {
            size: [40, 30],
            step: [6.0, 6.0],
            ppp: 1.0,
            t,
            strength: 0.55,
            a: [76, 210, 226],
            b: [216, 232, 74],
        }
    }

    #[test]
    fn cpu_size_stays_within_budget() {
        for (w, h) in [
            (800.0, 500.0),
            (1600.0, 900.0),
            (3840.0, 2160.0),
            (7680.0, 4320.0),
        ] {
            let [x, y] = cpu_size(w, h);
            assert!((x * y) as f32 <= CPU_BUDGET * 1.05, "{w}x{h}: {x}x{y}");
            assert!(w / x as f32 >= 2.9);
        }
        let img = render_cpu(&job(2.0));
        assert_eq!(img.len(), 1200);
        assert!(img.iter().all(|c| c.a() == 0), "light is additive");
        assert!(img.iter().any(|c| c.g() > 40), "some light shows");
    }

    #[test]
    fn cpu_image_built_in_slices_matches_whole() {
        let whole = render_cpu(&job(1.0));
        let mut parts = Vec::new();
        for r in [0..7, 7..8, 8..30] {
            render_rows(&job(1.0), r, &mut parts);
        }
        assert_eq!(whole, parts);
    }

    #[test]
    fn cpu_pattern_moves_with_time_and_rests_without() {
        let a = render_cpu(&job(2.0));
        let b = render_cpu(&job(2.0));
        let c = render_cpu(&job(2.3));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn only_undercurrent_has_caustics() {
        let th = Theme::undercurrent();
        assert!(th.caustics > 0.0);
        assert!(crate::hud::Fx::default().animating(&th));
        for t in crate::theme::Theme::builtins() {
            if t.name != "Undercurrent" {
                assert_eq!(t.caustics, 0.0, "{}", t.name);
            }
        }
        // Theme files without the field read as off, and off is not written.
        let json = serde_json::to_string(&Theme::default()).unwrap();
        assert!(!json.contains("caustics"));
        let back: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(back.caustics, 0.0);
    }
}

#[cfg(test)]
mod mask_tests {
    use super::*;

    #[test]
    fn the_mask_drops_lone_lines_and_steps_the_rest() {
        // A lone line's light (about 0.42 plus its halo) is gone.
        assert_eq!(masked(0.45), 0.0);
        assert_eq!(masked(0.6), 0.0);
        // Above it, only four steps a unit.
        for k in 0..60 {
            let m = masked(0.6 + k as f32 * 0.05);
            assert!((m * 4.0 - (m * 4.0).round()).abs() < 1e-5, "{m}");
        }
        assert!(masked(1.6) > masked(1.0));
    }
}
