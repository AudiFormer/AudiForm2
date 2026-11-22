// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Ragdoll: a stick figure of our own in a room, for the Visualizer. The
//! music's beats set off shockwaves (rings spreading from the floor and
//! walls) that fling him about; loud passages add more and the bass shakes
//! the room. He can be grabbed with the mouse and thrown.
//!
//! He has a mood, shown by how he stands rather than by a face. Content,
//! he stands easy, sways and bobs to the music. Grabbed, dragged, thrown
//! hard or slammed into walls (rough handling close together counts
//! more), he grows afraid: he cowers with his arms over his head and his
//! knees bent, trembles, backs away from the mouse and shies from being
//! picked up. The music's shockwaves hardly count: that is dancing, not
//! bullying. Left alone he slowly calms down; food calms him quickly. The
//! apple button in the corner drops food from the ceiling, and he walks
//! over (warily, if afraid) and eats it. Now and then a speech bubble says
//! how he feels: a smile when happy, "!" when startled, a wobbly frown
//! with a bead of sweat when nervous.
//!
//! A Verlet ragdoll: eleven points (head, neck, hips, elbows, hands,
//! knees, feet) held by sticks, a few soft limits that keep limbs from
//! folding through the body, gravity, and bouncy walls. Positions are in
//! room units (the room is 1 high), so a resized panel keeps him whole.

use super::{Analysis, Canvas, Palette, Visual};

const HEAD: usize = 0;
const NECK: usize = 1;
const HIPS: usize = 2;
const ELBOW_L: usize = 3;
const HAND_L: usize = 4;
const ELBOW_R: usize = 5;
const HAND_R: usize = 6;
const KNEE_L: usize = 7;
const FOOT_L: usize = 8;
const KNEE_R: usize = 9;
const FOOT_R: usize = 10;
const POINTS: usize = 11;
/// What a hand can catch him by first.
const ENDS: [usize; 5] = [HEAD, HAND_L, HAND_R, FOOT_L, FOOT_R];

/// Where each point sits standing at ease, relative to the feet's middle
/// (x right, y down, room units).
const STAND: [(f32, f32); POINTS] = [
    (0.0, -0.40),   // head
    (0.0, -0.31),   // neck
    (0.0, -0.17),   // hips
    (-0.07, -0.24), // elbow L
    (-0.09, -0.16), // hand L
    (0.07, -0.24),  // elbow R
    (0.09, -0.16),  // hand R
    (-0.035, -0.085),
    (-0.04, 0.0),
    (0.035, -0.085),
    (0.04, 0.0),
];

/// Cowering: crouched, knees bent wide, arms up over the head.
const COWER: [(f32, f32); POINTS] = [
    (0.0, -0.30),
    (0.0, -0.22),
    (0.0, -0.10),
    (-0.07, -0.30),
    (-0.03, -0.37),
    (0.07, -0.30),
    (0.03, -0.37),
    (-0.09, -0.07),
    (-0.06, 0.0),
    (0.09, -0.07),
    (0.06, 0.0),
];

/// Sticks: (a, b). Their lengths come from the standing pose.
const STICKS: [(usize, usize); 11] = [
    (HEAD, NECK),
    (NECK, HIPS),
    (NECK, ELBOW_L),
    (ELBOW_L, HAND_L),
    (NECK, ELBOW_R),
    (ELBOW_R, HAND_R),
    (HIPS, KNEE_L),
    (KNEE_L, FOOT_L),
    (HIPS, KNEE_R),
    (KNEE_R, FOOT_R),
    // Keeps the head on top of the body rather than folding over it.
    (HEAD, HIPS),
];

/// Soft limits: (a, b, shortest share of the standing distance).
const LIMITS: [(usize, usize, f32); 6] = [
    (HAND_L, NECK, 0.45),
    (HAND_R, NECK, 0.45),
    (FOOT_L, HIPS, 0.55),
    (FOOT_R, HIPS, 0.55),
    (FOOT_L, FOOT_R, 0.6),
    (HEAD, KNEE_L, 0.6),
];

const GRAVITY: f32 = 2.6;
/// The floor's height, room units.
const FLOOR: f32 = 0.09;
const BOUNCE: f32 = 0.55;
/// Points this near the pointer can be grabbed, room units.
const REACH: f32 = 0.07;
const SUBSTEPS: usize = 4;
/// The food button: its size and its gap from the corner, room units.
const BUTTON: f32 = 0.075;
const MARGIN: f32 = 0.025;
/// Food at once, at most.
const MAX_FOOD: usize = 2;
/// How near the pointer has to come before an afraid figure backs off.
const WARY: f32 = 0.32;

/// A shockwave: a ring spreading from a point, pushing whatever it passes.
struct Wave {
    x: f32,
    y: f32,
    r: f32,
    strength: f32,
    hue: f32,
    /// Points it has pushed already (one push each).
    hit: [bool; POINTS],
}

/// What a speech bubble says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feeling {
    Happy,
    Alert,
    Nervous,
}

/// How long a bubble stays up, seconds.
const BUBBLE: f32 = 1.8;

/// An apple, falling, lying on the floor, or being eaten.
struct Food {
    x: f32,
    y: f32,
    vy: f32,
    landed: bool,
    /// 0 untouched, to 1 eaten.
    eaten: f32,
}

pub struct Ragdoll {
    p: [(f32, f32); POINTS],
    prev: [(f32, f32); POINTS],
    len: [f32; STICKS.len()],
    limit: [f32; LIMITS.len()],
    waves: Vec<Wave>,
    food: Vec<Food>,
    /// The room's width (its height is 1).
    aspect: f32,
    placed: bool,
    grabbed: Option<usize>,
    /// Seconds since he was last flung (he gets up when it grows).
    calm: f32,
    /// 1 content, 0 terrified.
    mood: f32,
    /// Recent rough handling (it compounds, and fades over a few seconds).
    rough: f32,
    /// Seconds since a hand last let go of him.
    handled_ago: f32,
    /// How fast the held point was moving, room units a second.
    held_speed: f32,
    /// The hardest wall or floor hit this picture, room units a second.
    slam: f32,
    slam_cool: f32,
    /// Seconds to the next loud-passage wave.
    rush: f32,
    /// How hard the room shakes (bass), smoothed.
    shake: f32,
    /// A dip on each beat (he bobs to the music when content).
    bounce: f32,
    time: f32,
    walk_phase: f32,
    walking: f32,
    /// His walking speed, eased toward where he wants to go.
    walk_v: f32,
    /// The pointer was down last picture, and that press began on the
    /// food button.
    was_down: bool,
    on_button: bool,
    /// The speech bubble showing and how long it has been up, and seconds
    /// to the next unprompted one.
    bubble: Option<(Feeling, f32)>,
    chat_in: f32,
    rng: u64,
}

impl Default for Ragdoll {
    fn default() -> Self {
        let dist = |a: usize, b: usize| {
            let (ax, ay) = STAND[a];
            let (bx, by) = STAND[b];
            ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
        };
        let mut len = [0.0; STICKS.len()];
        for (l, &(a, b)) in len.iter_mut().zip(&STICKS) {
            *l = dist(a, b);
        }
        let mut limit = [0.0; LIMITS.len()];
        for (l, &(a, b, k)) in limit.iter_mut().zip(&LIMITS) {
            *l = dist(a, b) * k;
        }
        Ragdoll {
            p: [(0.0, 0.0); POINTS],
            prev: [(0.0, 0.0); POINTS],
            len,
            limit,
            waves: Vec::new(),
            food: Vec::new(),
            aspect: 1.0,
            placed: false,
            grabbed: None,
            calm: 10.0,
            mood: 0.85,
            rough: 0.0,
            handled_ago: 100.0,
            held_speed: 0.0,
            slam: 0.0,
            slam_cool: 0.0,
            rush: 0.0,
            shake: 0.0,
            bounce: 0.0,
            time: 0.0,
            walk_phase: 0.0,
            walking: 0.0,
            walk_v: 0.0,
            was_down: false,
            on_button: false,
            bubble: None,
            chat_in: 6.0,
            rng: 0x2545_f491_4f6c_dd1d,
        }
    }
}

impl Ragdoll {
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    fn floor_y(&self) -> f32 {
        1.0 - FLOOR
    }

    /// 0 at ease, 1 cowering.
    fn fear(&self) -> f32 {
        ((0.4 - self.mood) / 0.3).clamp(0.0, 1.0)
    }

    fn feet_down(&self) -> bool {
        let fy = self.floor_y();
        self.p[FOOT_L].1 > fy - 0.03 || self.p[FOOT_R].1 > fy - 0.03
    }

    /// The food button, in room units: (left, top, right, bottom).
    fn button(&self) -> (f32, f32, f32, f32) {
        let r = self.aspect - MARGIN;
        (r - BUTTON, MARGIN, r, MARGIN + BUTTON)
    }

    fn on_the_button(&self, x: f32, y: f32) -> bool {
        let (l, t, r, b) = self.button();
        (l..=r).contains(&x) && (t..=b).contains(&y)
    }

    /// Drops an apple from the ceiling, somewhere he can reach.
    fn drop_food(&mut self) {
        if self.food.len() >= MAX_FOOD {
            return;
        }
        let x = 0.12 + self.rand() * (self.aspect - 0.24).max(0.01);
        self.food.push(Food { x, y: 0.02, vy: 0.0, landed: false, eaten: 0.0 });
    }

    /// Shows a bubble, unless one just went up.
    fn say(&mut self, f: Feeling) {
        match self.bubble {
            Some((g, age)) if g == f || age < 0.6 => {}
            _ => self.bubble = Some((f, 0.0)),
        }
        self.chat_in = 9.0 + self.rand() * 7.0;
    }

    fn lower_mood(&mut self, by: f32) {
        self.mood = (self.mood - by.max(0.0)).clamp(0.0, 1.0);
    }

    /// Standing on the floor at `x`.
    fn place(&mut self, x: f32) {
        let fy = self.floor_y();
        for (i, &(sx, sy)) in STAND.iter().enumerate() {
            self.p[i] = (x + sx, fy + sy);
        }
        self.prev = self.p;
        self.placed = true;
    }

    /// Where point `i` would be, from the feet's middle: at ease or
    /// cowering by his fear, swaying and bobbing when content, his feet
    /// stepping when he walks.
    fn pose(&self, i: usize) -> (f32, f32) {
        let f = self.fear();
        let (sx, sy) = STAND[i];
        let (cx, cy) = COWER[i];
        let (mut x, mut y) = (sx + (cx - sx) * f, sy + (cy - sy) * f);
        let content = 1.0 - f;
        if i != FOOT_L && i != FOOT_R {
            // Upper body sways; everything above the knees dips on a beat.
            let up = (-sy / 0.4).clamp(0.0, 1.0);
            x += (self.time * 1.3).sin() * 0.018 * content * up;
            if i != KNEE_L && i != KNEE_R {
                y += self.bounce * 0.03 * content;
            }
        }
        if self.walking > 0.0 {
            let ph = self.walk_phase;
            if i == FOOT_L || i == KNEE_L {
                y -= ph.sin().max(0.0) * 0.03 * self.walking;
            } else if i == FOOT_R || i == KNEE_R {
                y -= (-ph.sin()).max(0.0) * 0.03 * self.walking;
            }
        }
        (x, y)
    }

    /// A shockwave from somewhere on the floor or a wall, `strength` 0..~2.
    fn wave(&mut self, strength: f32, hue: f32) {
        let (w, fy) = (self.aspect, self.floor_y());
        let side = self.rand();
        let (x, y) = if side < 0.6 {
            // From the floor, mostly not right under him.
            let hx = self.p[HIPS].0;
            let mut x = self.rand() * w;
            if (x - hx).abs() < 0.1 {
                x = (x + w * 0.5).rem_euclid(w);
            }
            (x, fy)
        } else if side < 0.8 {
            (0.0, 0.3 + self.rand() * (fy - 0.3))
        } else {
            (w, 0.3 + self.rand() * (fy - 0.3))
        };
        self.waves.push(Wave {
            x,
            y,
            r: 0.0,
            strength,
            hue,
            hit: [false; POINTS],
        });
        if self.waves.len() > 24 {
            self.waves.remove(0);
        }
    }

    /// Where he wants to walk, room units a second (left negative): away
    /// from a pointer that comes near when he is afraid, else to food.
    fn wish(&self, pointer: Option<(f32, f32, bool)>) -> f32 {
        let hx = self.p[HIPS].0;
        let fear = self.fear();
        let near = self
            .food
            .iter()
            .filter(|f| f.landed)
            .map(|f| f.x - hx)
            .min_by(|a, b| a.abs().total_cmp(&b.abs()));
        // How far off the mouse is, if he is scared enough to mind it.
        let mouse = pointer.filter(|_| fear > 0.35).map(|(x, y, _)| {
            let d = ((x - hx).powi(2) + (y - self.p[HIPS].1).powi(2)).sqrt();
            (if hx >= x { 1.0 } else { -1.0 }, d)
        });
        let away = |side: f32, d: f32| side * 0.26 * fear * (1.0 - d / WARY).max(0.45);
        match (near, mouse) {
            // Hungry beats scared, unless the mouse is right on top of him.
            (Some(_), Some((side, d))) if d < WARY * 0.4 => away(side, d),
            (Some(dx), _) if dx.abs() > 0.05 => dx.signum() * 0.18 * (0.45 + 0.55 * self.mood),
            (Some(_), _) => 0.0,
            (None, Some((side, d))) if d < WARY => away(side, d),
            _ => 0.0,
        }
    }

    fn step(&mut self, dt: f32, pointer: Option<(f32, f32, bool)>) {
        let (w, fy) = (self.aspect, self.floor_y());
        // Grabbing: the nearest point within reach of a press. Afraid, he
        // shies from the hand, so it has to come closer.
        match pointer {
            Some((x, y, true)) => {
                if self.grabbed.is_none() {
                    // His head, hands and feet first; failing those, any
                    // point of him a little closer.
                    let reach = REACH * 1.6 * (1.0 - 0.6 * self.fear());
                    let dist = |i: usize| ((self.p[i].0 - x).powi(2) + (self.p[i].1 - y).powi(2)).sqrt();
                    let nearest = |list: &[usize]| {
                        list.iter().map(|&i| (i, dist(i))).min_by(|a, b| a.1.total_cmp(&b.1))
                    };
                    let all: Vec<usize> = (0..POINTS).collect();
                    let pick = match nearest(&ENDS) {
                        Some((i, d)) if d < reach => Some(i),
                        _ => nearest(&all).filter(|&(_, d)| d < reach * 0.75).map(|(i, _)| i),
                    };
                    if let Some(i) = pick {
                        self.grabbed = Some(i);
                        self.lower_mood(0.015 + 0.02 * self.rough);
                        self.rough += 0.25;
                        self.say(if self.fear() > 0.4 { Feeling::Nervous } else { Feeling::Alert });
                    }
                }
            }
            _ => {
                if self.grabbed.take().is_some() {
                    // Let go: a throw frightens him by how hard it was.
                    let v = self.held_speed;
                    if v > 2.0 {
                        self.lower_mood(((v - 2.0) * 0.03).min(0.15) * (1.0 + 0.5 * self.rough));
                        self.rough += (v * 0.15).min(1.0);
                    }
                    self.handled_ago = 0.0;
                }
            }
        }
        // Moving.
        let damp = 0.998;
        for i in 0..POINTS {
            let (x, y) = self.p[i];
            let (px, py) = self.prev[i];
            let (vx, vy) = ((x - px) * damp, (y - py) * damp);
            self.prev[i] = (x, y);
            self.p[i] = (x + vx, y + vy + GRAVITY * dt * dt);
        }
        // Getting up: once he has been left alone a moment and his feet
        // are down, the body pulls itself into his pose over them, and he
        // walks where he wants to go.
        let upright = self.grabbed.is_none() && self.calm > 0.8 && self.feet_down();
        if upright {
            let want = self.wish(pointer);
            self.walk_v += (want - self.walk_v) * (dt * 3.0).min(1.0);
            if want == 0.0 && self.walk_v.abs() < 0.005 {
                self.walk_v = 0.0;
            }
            self.walking = (self.walk_v.abs() / 0.04).min(1.0);
            if self.walk_v != 0.0 {
                self.walk_phase += dt * (5.0 + self.walk_v.abs() * 28.0);
                // All of him moves; his feet lift in turn (see `pose`).
                let step = self.walk_v * dt;
                for i in 0..POINTS {
                    self.p[i].0 += step;
                    self.prev[i].0 += step;
                }
            }
            let k = ((self.calm - 0.8) * 0.8).min(1.0) * 0.06;
            // Anchored between his feet (anchoring at the hips made the feet
            // chase the hips across the floor), kept far enough from the
            // walls that the whole pose fits in the room, so a foot caught in
            // a corner is drawn back in; the feet are pulled under him a
            // little harder.
            let mid = ((self.p[FOOT_L].0 + self.p[FOOT_R].0) / 2.0).clamp(0.11, (w - 0.11).max(0.11));
            for i in 0..POINTS {
                let (sx, sy) = self.pose(i);
                let (tx, ty) = (mid + sx, fy + sy);
                let (x, y) = self.p[i];
                let k = if i == FOOT_L || i == FOOT_R { (k * 2.5).min(1.0) } else { k };
                self.p[i] = (x + (tx - x) * k, y + (ty - y) * k);
            }
        } else {
            self.walk_v = 0.0;
            self.walking = 0.0;
        }
        // Shockwaves push each point once as their front passes.
        let speed = 1.7;
        let mut pushes = Vec::new();
        for wv in &mut self.waves {
            let r0 = wv.r;
            wv.r += speed * dt;
            for (i, &(x, y)) in self.p.iter().enumerate() {
                if wv.hit[i] {
                    continue;
                }
                let (dx, dy) = (x - wv.x, y - wv.y);
                let d = (dx * dx + dy * dy).sqrt().max(1e-4);
                if d >= r0 && d < wv.r {
                    wv.hit[i] = true;
                    // Weaker the further it has spread.
                    let k = wv.strength * 1.8 / (1.0 + d * 1.8);
                    pushes.push((i, dx / d * k * 1.3, dy / d * k * 0.85 - k * 0.22));
                }
            }
        }
        for (i, vx, vy) in pushes {
            if Some(i) == self.grabbed {
                continue;
            }
            self.prev[i].0 -= vx * dt;
            self.prev[i].1 -= vy * dt;
            self.calm = 0.0;
        }
        self.waves.retain(|wv| wv.r < 2.0 * (w + 1.0));
        // The sticks and limits, a few rounds.
        for _ in 0..3 {
            for (k, &(a, b)) in STICKS.iter().enumerate() {
                self.pull(a, b, self.len[k], false);
            }
            for (k, &(a, b, _)) in LIMITS.iter().enumerate() {
                self.pull(a, b, self.limit[k], true);
            }
            if let (Some(i), Some((x, y, _))) = (self.grabbed, pointer) {
                self.p[i] = (x.clamp(0.0, w), y.clamp(0.0, fy));
            }
            self.walls(dt);
        }
        // Held, he doesn't try to get up, and it wears on him.
        if let Some(g) = self.grabbed {
            self.calm = 0.0;
            self.handled_ago = 0.0;
            let (x, y) = self.p[g];
            let (px, py) = self.prev[g];
            self.held_speed = ((x - px).powi(2) + (y - py).powi(2)).sqrt() / dt.max(1e-5);
            self.lower_mood(dt * 0.01);
        }
    }

    /// Holds two points `len` apart (or, for a limit, at least `len`).
    fn pull(&mut self, a: usize, b: usize, len: f32, at_least: bool) {
        let (dx, dy) = (self.p[b].0 - self.p[a].0, self.p[b].1 - self.p[a].1);
        let d = (dx * dx + dy * dy).sqrt().max(1e-6);
        if at_least && d >= len {
            return;
        }
        let diff = (d - len) / d * 0.5;
        let (ox, oy) = (dx * diff, dy * diff);
        let (wa, wb) = match self.grabbed {
            Some(g) if g == a => (0.0, 2.0),
            Some(g) if g == b => (2.0, 0.0),
            _ => (1.0, 1.0),
        };
        self.p[a].0 += ox * wa;
        self.p[a].1 += oy * wa;
        self.p[b].0 -= ox * wb;
        self.p[b].1 -= oy * wb;
    }

    /// Bounces off the walls, floor and ceiling, with a little friction on
    /// the floor, noting the hardest hit.
    fn walls(&mut self, dt: f32) {
        let (w, fy) = (self.aspect, self.floor_y());
        let mut hardest = 0.0f32;
        for i in 0..POINTS {
            let (mut x, mut y) = self.p[i];
            let (mut px, mut py) = self.prev[i];
            let pad = if i == HEAD { 0.05 } else { 0.012 };
            if x < pad {
                hardest = hardest.max((px - x).max(0.0));
                x = pad;
                px = x - (x - px).abs().min(0.2) * BOUNCE;
            } else if x > w - pad {
                hardest = hardest.max((x - px).max(0.0));
                x = w - pad;
                px = x + (x - px).abs().min(0.2) * BOUNCE;
            }
            if y > fy - pad {
                hardest = hardest.max((y - py).max(0.0));
                y = fy - pad;
                let vx = x - px;
                let vy = (y - py).abs().min(0.2);
                py = y + vy * BOUNCE;
                px = x - vx * 0.85;
            } else if y < pad {
                hardest = hardest.max((py - y).max(0.0));
                y = pad;
                py = y - (py - y).abs().min(0.2) * BOUNCE;
            }
            self.p[i] = (x, y);
            self.prev[i] = (px, py);
        }
        self.slam = self.slam.max(hardest / dt.max(1e-5));
    }

    /// The mood over one picture: slams, recovery, food.
    fn feel(&mut self, dt: f32) {
        self.slam_cool = (self.slam_cool - dt).max(0.0);
        if self.slam > 1.8 && self.slam_cool <= 0.0 && self.grabbed.is_none() {
            // Hitting a wall after being thrown frightens him; after a
            // shockwave, hardly at all.
            let weight = if self.handled_ago < 2.5 { 1.0 } else { 0.1 };
            let by = ((self.slam - 1.8) * 0.03).min(0.12) * weight * (1.0 + 0.5 * self.rough);
            self.lower_mood(by);
            if weight == 1.0 {
                self.rough += 0.3;
                self.say(if self.fear() > 0.4 { Feeling::Nervous } else { Feeling::Alert });
            }
            self.slam_cool = 0.3;
        }
        self.slam = 0.0;
        self.rough *= (-dt / 4.0).exp();
        self.handled_ago += dt;
        if self.grabbed.is_none() && self.handled_ago > 3.0 {
            self.mood = (self.mood + dt * 0.012).min(1.0);
        }
        // Food: it falls, lands, and he eats what he reaches.
        let (fy, w) = (self.floor_y(), self.aspect);
        let (hx, upright) = (self.p[HIPS].0, self.grabbed.is_none() && self.feet_down());
        let mut calmed = 0.0;
        for f in &mut self.food {
            if !f.landed {
                f.vy += GRAVITY * dt;
                f.y += f.vy * dt;
                if f.y >= fy - 0.018 {
                    f.y = fy - 0.018;
                    if f.vy > 0.6 {
                        f.vy = -f.vy * 0.3;
                    } else {
                        f.landed = true;
                        f.vy = 0.0;
                    }
                }
            }
            f.x = f.x.clamp(0.03, w - 0.03);
            if f.landed && upright && (f.x - hx).abs() < 0.07 {
                let bite = dt.min(1.0 - f.eaten);
                f.eaten += bite;
                calmed += bite * 0.35;
            }
        }
        let before = self.food.len();
        self.food.retain(|f| f.eaten < 1.0);
        self.mood = (self.mood + calmed).min(1.0);
        if self.food.len() < before {
            self.say(Feeling::Happy);
        }
        // Now and then, unprompted: how he feels.
        self.chat_in -= dt;
        if self.chat_in <= 0.0 && self.grabbed.is_none() {
            let fear = self.fear();
            if fear > 0.4 {
                self.say(Feeling::Nervous);
            } else if self.mood > 0.7 && self.feet_down() {
                self.say(Feeling::Happy);
            } else {
                self.chat_in = 4.0;
            }
        }
        if let Some((_, age)) = &mut self.bubble {
            *age += dt;
            if *age >= BUBBLE {
                self.bubble = None;
            }
        }
    }
}

impl Visual for Ragdoll {
    fn draw(&mut self, cv: &mut Canvas, a: &Analysis, pal: &Palette) {
        let (wpx, hpx) = (cv.w as f32, cv.h as f32);
        if wpx < 2.0 || hpx < 2.0 {
            return;
        }
        let s = hpx;
        let new_aspect = wpx / hpx;
        if !self.placed {
            self.aspect = new_aspect;
            self.place(new_aspect * 0.5);
        } else if (new_aspect - self.aspect).abs() > 1e-3 {
            // A wider or narrower room: keep him inside it.
            let k = new_aspect / self.aspect;
            for (p, q) in self.p.iter_mut().zip(self.prev.iter_mut()) {
                p.0 *= k;
                q.0 *= k;
            }
            for f in &mut self.food {
                f.x *= k;
            }
            self.aspect = new_aspect;
        }
        let dt = a.dt.clamp(0.0, 0.05);
        self.time += dt;

        // The music: a wave on each beat, more when it is loud, the room
        // shaking with the bass.
        if a.beat {
            let strength = 0.45 + a.bass * 1.5 + a.level * 0.4;
            let hue = self.rand();
            self.wave(strength, hue);
            self.bounce = 1.0;
        }
        self.bounce *= (-dt * 6.0).exp();
        self.rush -= dt;
        if a.level > 0.55 && self.rush <= 0.0 {
            let hue = self.rand();
            self.wave(0.25 + (a.level - 0.55) * 1.6 + a.treble * 0.4, hue);
            self.rush = 0.35 - a.level * 0.2;
        }
        let target = a.bass * a.bass;
        self.shake += (target - self.shake) * (dt * 10.0).min(1.0);
        if self.shake > 0.35 && self.p[FOOT_L].1 > self.floor_y() - 0.03 {
            // The floor hops under him.
            let kick = (self.shake - 0.35) * 0.9 * dt;
            for i in [FOOT_L, FOOT_R, KNEE_L, KNEE_R] {
                self.prev[i].1 += kick;
            }
        }

        // The pointer: a press on the food button drops food (and that
        // press grabs nothing).
        let mut pointer = a.pointer.map(|(x, y, down)| (x / s, y / s, down));
        let down = pointer.is_some_and(|p| p.2);
        if down && !self.was_down {
            let (x, y, _) = pointer.unwrap_or_default();
            self.on_button = self.on_the_button(x, y);
            if self.on_button {
                self.drop_food();
            }
        }
        if !down {
            self.on_button = false;
        }
        self.was_down = down;
        if self.on_button {
            pointer = pointer.map(|(x, y, _)| (x, y, false));
        }

        let sub = dt / SUBSTEPS as f32;
        for _ in 0..SUBSTEPS {
            self.step(sub, pointer);
        }
        if self.grabbed.is_none() && self.fear() > 0.35 && self.wish(pointer) != 0.0 && self.food.iter().all(|f| !f.landed) {
            if self.bubble.is_none() {
                self.say(Feeling::Nervous);
            }
        }
        self.calm += dt;
        self.feel(dt);

        // The room, shaking a little with the bass.
        let jolt = self.shake * 0.006 * s;
        let oy = if a.beat { jolt } else { 0.0 };
        let wall = [16.0, 18.0, 28.0];
        cv.fill_rect(0.0, 0.0, wpx, hpx, wall, 1.0);
        let fy = self.floor_y() * s + oy;
        let floor_col = pal.css(0.08);
        cv.fill_rect(0.0, fy, wpx, hpx - fy, [floor_col[0] * 0.18 + 20.0, floor_col[1] * 0.18 + 20.0, floor_col[2] * 0.18 + 26.0], 1.0);
        cv.fill_rect(0.0, fy, wpx, 2.0, floor_col, 0.5 + self.shake * 0.5);

        // The waves.
        for wv in &self.waves {
            let fade = (1.0 - wv.r / (self.aspect + 1.0)).clamp(0.0, 1.0);
            if fade <= 0.0 {
                continue;
            }
            let col = pal.css(wv.hue);
            let width = (0.004 + wv.strength * 0.006) * s;
            cv.ring(wv.x * s, wv.y * s + oy, wv.r * s, width, col, 0.55 * fade);
            cv.ring(wv.x * s, wv.y * s + oy, (wv.r * 0.85).max(0.0) * s, width * 0.5, col, 0.2 * fade);
        }

        // The food.
        for f in &self.food {
            let k = 1.0 - f.eaten;
            apple(cv, f.x * s, f.y * s + oy, 0.018 * s * k.sqrt(), k.max(0.2));
        }

        // His shadow on the floor, smaller the higher he is.
        let hx = self.p[HIPS].0 * s;
        let lift = (self.floor_y() - self.p[HIPS].1).max(0.0);
        let sh = (1.0 - lift * 1.2).clamp(0.15, 1.0);
        for k in 0..5 {
            let off = (k as f32 - 2.0) * 0.018 * s * sh;
            cv.disc(hx + off, fy + 0.006 * s, 0.022 * s * sh, [0.0, 0.0, 0.0], 0.12 * sh);
        }

        // Him: a stick figure, trembling when afraid. One even line, round
        // joints, a thin dark edge so he reads over the waves.
        let fear = self.fear();
        let shiver = if self.grabbed.is_none() { fear * 0.0028 * s } else { 0.0 };
        let t = self.time;
        let at = |i: usize| {
            let j = (t * 47.0 + i as f32 * 1.7).sin() * shiver;
            (self.p[i].0 * s + j, self.p[i].1 * s + oy)
        };
        let col = {
            let c = pal.css(0.22);
            [c[0] * 0.35 + 165.0, c[1] * 0.35 + 165.0, c[2] * 0.35 + 165.0]
        };
        let edge = [10.0, 10.0, 16.0];
        let line = 0.017 * s;
        let head_r = 0.05 * s;
        let (hx, hy) = at(HEAD);
        let (nx, ny) = at(NECK);
        // The neck stops at the head's circle.
        let neck_top = {
            let (dx, dy) = (nx - hx, ny - hy);
            let d = (dx * dx + dy * dy).sqrt().max(1e-4);
            (hx + dx / d * head_r, hy + dy / d * head_r)
        };
        let bones: [((f32, f32), (f32, f32)); 9] = [
            (neck_top, (nx, ny)),
            ((nx, ny), at(HIPS)),
            ((nx, ny), at(ELBOW_L)),
            (at(ELBOW_L), at(HAND_L)),
            ((nx, ny), at(ELBOW_R)),
            (at(ELBOW_R), at(HAND_R)),
            (at(HIPS), at(KNEE_L)),
            (at(KNEE_L), at(FOOT_L)),
            (at(HIPS), at(KNEE_R)),
        ];
        let last = (at(KNEE_R), at(FOOT_R));
        // Hands and feet: balls a little wider than the sticks.
        let ends = [at(HAND_L), at(HAND_R), at(FOOT_L), at(FOOT_R)];
        let ball = line * 0.72;
        for &(p, q) in bones.iter().chain([&last]) {
            cv.segment(p, q, line + 0.008 * s, true, edge, 0.7);
        }
        for &(x, y) in &ends {
            cv.disc(x, y, ball + 0.004 * s, edge, 0.7);
        }
        cv.disc(hx, hy, head_r + 0.004 * s, edge, 0.7);
        for &(p, q) in bones.iter().chain([&last]) {
            cv.segment(p, q, line, true, col, 1.0);
        }
        for &(x, y) in &ends {
            cv.disc(x, y, ball, col, 1.0);
        }
        // A solid head.
        cv.disc(hx, hy, head_r, col, 1.0);

        if let Some((f, age)) = self.bubble {
            let fade = (age / 0.15).min(1.0).min((BUBBLE - age) / 0.3).clamp(0.0, 1.0);
            bubble(cv, (hx, hy), head_r, s, wpx, f, col, fade);
        }

        // The food button, and a small mood meter beside it.
        let (l, top, r, b) = self.button();
        let hover = a.pointer.is_some_and(|(x, y, _)| self.on_the_button(x / s, y / s));
        let full = self.food.len() >= MAX_FOOD;
        let glass = if full { 0.04 } else if hover { 0.16 } else { 0.09 };
        cv.fill_rect(l * s, top * s, (r - l) * s, (b - top) * s, [255.0, 255.0, 255.0], glass);
        apple(cv, (l + r) * 0.5 * s, (top + b) * 0.5 * s + 0.004 * s, 0.019 * s, if full { 0.35 } else { 0.9 });
        let (mx0, mx1, my) = ((l - 0.012 - 0.09) * s, (l - 0.012) * s, (top + b) * 0.5 * s);
        let mh = 0.008 * s;
        cv.fill_rect(mx0, my - mh / 2.0, mx1 - mx0, mh, [255.0, 255.0, 255.0], 0.08);
        let m = self.mood;
        let mcol = [230.0 + (90.0 - 230.0) * m, 80.0 + (210.0 - 80.0) * m, 80.0 + (120.0 - 80.0) * m];
        cv.fill_rect(mx0, my - mh / 2.0, (mx1 - mx0) * m, mh, mcol, 0.7);
        heart(cv, mx0 - 0.016 * s, my, 0.006 * s, mcol, 0.7);
    }
}

/// A small apple (a red ball, a stem and a leaf) centred at `(x, y)`.
fn apple(cv: &mut Canvas, x: f32, y: f32, r: f32, a: f32) {
    if r <= 0.3 {
        return;
    }
    cv.disc(x - r * 0.3, y, r * 0.8, [205.0, 45.0, 50.0], a);
    cv.disc(x + r * 0.3, y, r * 0.8, [205.0, 45.0, 50.0], a);
    cv.disc(x - r * 0.35, y - r * 0.3, r * 0.25, [255.0, 170.0, 160.0], a * 0.6);
    cv.segment((x, y - r * 0.6), (x + r * 0.15, y - r * 1.15), r * 0.18, true, [110.0, 70.0, 40.0], a);
    cv.disc(x + r * 0.45, y - r * 1.0, r * 0.28, [80.0, 170.0, 80.0], a);
}

/// A speech bubble up and to the side of his head (whichever side has
/// room), with its tail pointing at him and a line-drawn sign in it.
#[allow(clippy::too_many_arguments)]
fn bubble(cv: &mut Canvas, head: (f32, f32), head_r: f32, s: f32, wpx: f32, f: Feeling, col: [f32; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let (bw, bh) = (0.17 * s, 0.12 * s);
    let right = head.0 + head_r + bw + 0.02 * s < wpx;
    let cx = if right { head.0 + head_r + bw * 0.5 + 0.01 * s } else { head.0 - head_r - bw * 0.5 - 0.01 * s };
    let cy = (head.1 - head_r - bh * 0.6).max(bh * 0.5 + 2.0);
    let cx = cx.clamp(bw * 0.5 + 2.0, (wpx - bw * 0.5 - 2.0).max(bw * 0.5 + 2.0));
    let (l, t) = (cx - bw / 2.0, cy - bh / 2.0);
    let r = bh * 0.35;
    let fill = [col[0] * 0.25 + 24.0, col[1] * 0.25 + 26.0, col[2] * 0.25 + 36.0];
    // The tail, then the rounded body over it.
    let tail_from = (cx + if right { -bw * 0.25 } else { bw * 0.25 }, cy + bh * 0.35);
    let tail_to = (head.0 + if right { head_r * 0.7 } else { -head_r * 0.7 }, head.1 - head_r * 0.7);
    cv.segment(tail_from, tail_to, 0.012 * s, true, fill, a * 0.95);
    cv.fill_rect(l + r, t, bw - 2.0 * r, bh, fill, a * 0.95);
    cv.fill_rect(l, t + r, bw, bh - 2.0 * r, fill, a * 0.95);
    for (x, y) in [(l + r, t + r), (l + bw - r, t + r), (l + r, t + bh - r), (l + bw - r, t + bh - r)] {
        cv.disc(x, y, r, fill, a * 0.95);
    }
    // The sign.
    let ink = col;
    let w = 0.009 * s;
    let u = bh * 0.32;
    match f {
        Feeling::Happy => {
            cv.disc(cx - u * 0.5, cy - u * 0.45, w * 0.8, ink, a);
            cv.disc(cx + u * 0.5, cy - u * 0.45, w * 0.8, ink, a);
            // A smile: the lower part of a circle (screen y runs down).
            arc(cv, (cx, cy - u * 0.15), u * 0.75, 0.08, 0.42, w, ink, a);
        }
        Feeling::Alert => {
            cv.segment((cx, cy - u * 0.95), (cx, cy + u * 0.3), w * 1.3, true, ink, a);
            cv.disc(cx, cy + u * 0.85, w * 0.9, ink, a);
        }
        Feeling::Nervous => {
            cv.disc(cx - u * 0.5, cy - u * 0.4, w * 0.8, ink, a);
            cv.disc(cx + u * 0.5, cy - u * 0.4, w * 0.8, ink, a);
            // A wobbly frown.
            let pts: Vec<(f32, f32)> = (0..=8)
                .map(|k| {
                    let t = k as f32 / 8.0;
                    let x = cx - u * 0.6 + t * u * 1.2;
                    let y = cy + u * 0.55 + (t * std::f32::consts::TAU * 1.5).sin() * u * 0.12
                        - (t * std::f32::consts::PI).sin() * u * 0.25;
                    (x, y)
                })
                .collect();
            for p in pts.windows(2) {
                cv.segment(p[0], p[1], w, true, ink, a);
            }
            // A bead of sweat.
            let (dx, dy) = (cx + u * 1.05, cy - u * 0.55);
            cv.disc(dx, dy + u * 0.12, u * 0.17, [120.0, 190.0, 255.0], a);
            cv.segment((dx, dy - u * 0.15), (dx, dy + u * 0.05), u * 0.16, true, [120.0, 190.0, 255.0], a);
        }
    }
}

/// Part of a circle: from `from` to `to` turns (0 is to the right, going
/// clockwise on screen, so 0 to 0.5 is the lower half).
#[allow(clippy::too_many_arguments)]
fn arc(cv: &mut Canvas, c: (f32, f32), r: f32, from: f32, to: f32, w: f32, col: [f32; 3], a: f32) {
    let n = 10;
    let at = |k: usize| {
        let t = (from + (to - from) * k as f32 / n as f32) * std::f32::consts::TAU;
        (c.0 + r * t.cos(), c.1 + r * t.sin())
    };
    for k in 0..n {
        cv.segment(at(k), at(k + 1), w, true, col, a);
    }
}

/// A tiny heart centred at `(x, y)`.
fn heart(cv: &mut Canvas, x: f32, y: f32, r: f32, col: [f32; 3], a: f32) {
    cv.disc(x - r * 0.55, y - r * 0.3, r * 0.65, col, a);
    cv.disc(x + r * 0.55, y - r * 0.3, r * 0.65, col, a);
    cv.segment((x - r * 0.95, y), (x, y + r * 1.1), r * 0.6, true, col, a);
    cv.segment((x + r * 0.95, y), (x, y + r * 1.1), r * 0.6, true, col, a);
}

#[cfg(test)]
impl Ragdoll {
    /// Whether a point of him is within grabbing reach of `(x, y)` in
    /// canvas pixels on a canvas `h` high.
    fn near(&self, x: f32, y: f32, h: f32) -> bool {
        let (x, y) = (x / h, y / h);
        self.p
            .iter()
            .any(|p| ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt() < REACH * 1.6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Analyser, Scheme, BANDS};

    fn run(r: &mut Ragdoll, cv: &mut Canvas, a: &Analysis, n: usize) {
        let pal = Palette::new(Scheme::Theme, None, None);
        for _ in 0..n {
            r.draw(cv, a, &pal);
        }
    }

    fn quiet() -> Analysis {
        Analyser::default().feed(&[], 1.0, 1.0 / 30.0, 1.0)
    }

    /// Grabs his head and flings it `dx`, `dy` pixels a picture for ten
    /// pictures, then lets go and lets him land.
    fn fling(r: &mut Ragdoll, cv: &mut Canvas, dx: f32, dy: f32) {
        let mut q = quiet();
        let (hx, hy) = (r.p[HEAD].0 * 200.0, r.p[HEAD].1 * 200.0);
        for k in 0..10 {
            let (x, y) = (hx + dx * k as f32, hy + dy * k as f32);
            q.pointer = Some((x.clamp(1.0, 399.0), y.clamp(1.0, 175.0), true));
            run(r, cv, &q, 1);
        }
        q.pointer = None;
        run(r, cv, &q, 45);
    }

    #[test]
    fn left_alone_he_stands_on_the_floor() {
        let mut cv = Canvas::default();
        cv.resize(320, 200);
        let mut r = Ragdoll::default();
        run(&mut r, &mut cv, &quiet(), 200);
        let fy = r.floor_y();
        assert!(r.p[FOOT_L].1 > fy - 0.03 && r.p[FOOT_R].1 > fy - 0.03, "feet on the floor");
        assert!(r.p[HEAD].1 < r.p[HIPS].1 - 0.1, "head over hips");
        assert!(r.waves.is_empty());
        assert!(r.fear() == 0.0, "content");
    }

    #[test]
    fn beats_fling_him_and_he_stays_in_the_room() {
        let mut cv = Canvas::default();
        cv.resize(320, 200);
        let pal = Palette::new(Scheme::Theme, None, None);
        let mut r = Ragdoll::default();
        let mood = r.mood;
        let mut an = Analyser::default();
        let quiet = [-80.0f32; BANDS];
        let mut loud = [-80.0f32; BANDS];
        loud[..10].fill(-3.0);
        let mut highest = 1.0f32;
        for k in 0..600 {
            let a = an.feed(if k % 15 < 3 { &loud } else { &quiet }, 1.0, 1.0 / 30.0, 1.0);
            r.draw(&mut cv, &a, &pal);
            highest = highest.min(r.p[HIPS].1);
            for &(x, y) in &r.p {
                assert!(x.is_finite() && y.is_finite());
                assert!((-0.01..=r.aspect + 0.01).contains(&x), "x {x}");
                assert!((-0.01..=1.0).contains(&y), "y {y}");
            }
        }
        assert!(highest < r.floor_y() - 0.3, "the waves threw him up ({highest})");
        // Dancing to the music is not being bullied.
        assert!(r.mood > mood - 0.25, "the music frightened him: {} from {mood}", r.mood);
    }

    #[test]
    fn he_can_be_grabbed_and_thrown() {
        let mut cv = Canvas::default();
        cv.resize(400, 200);
        let mut r = Ragdoll::default();
        let mut q = quiet();
        run(&mut r, &mut cv, &q, 30);
        let (hx, hy) = (r.p[HEAD].0 * 200.0, r.p[HEAD].1 * 200.0);
        assert!(r.near(hx, hy, 200.0));
        // Grab the head and sweep it up and to the left, then let go.
        for k in 0..10 {
            q.pointer = Some((hx - k as f32 * 12.0, hy - k as f32 * 8.0, true));
            run(&mut r, &mut cv, &q, 1);
        }
        assert_eq!(r.grabbed, Some(HEAD));
        q.pointer = Some((hx - 120.0, hy - 80.0, false));
        run(&mut r, &mut cv, &q, 1);
        assert_eq!(r.grabbed, None);
        let vx = r.p[HEAD].0 - r.prev[HEAD].0;
        assert!(vx < 0.0, "still flying left");
    }

    #[test]
    fn rough_throws_frighten_him_and_he_backs_away_from_the_mouse() {
        let mut cv = Canvas::default();
        cv.resize(400, 200);
        let mut r = Ragdoll::default();
        run(&mut r, &mut cv, &quiet(), 40);
        let start = r.mood;
        // A couple of gentle pick-ups don't frighten him.
        for _ in 0..2 {
            fling(&mut r, &mut cv, 1.0, -1.0);
        }
        assert_eq!(r.fear(), 0.0, "mood {} after gentle handling", r.mood);
        // Being thrown hard, again and again, does.
        for k in 0..10 {
            let dir = if k % 2 == 0 { -1.0 } else { 1.0 };
            fling(&mut r, &mut cv, dir * 30.0, -12.0);
        }
        assert!(r.mood < start - 0.3, "mood {} from {start}", r.mood);
        assert!(r.fear() > 0.3, "afraid: {}", r.fear());
        // Let him get up, then bring the mouse near him from the wall side
        // (so there is room to back away into).
        r.mood = 0.15;
        let mut q = quiet();
        run(&mut r, &mut cv, &q, 60);
        let before = r.p[HIPS].0;
        let side = if before < r.aspect / 2.0 { -1.0 } else { 1.0 };
        let px = (before + side * 0.15).max(0.0) * 200.0;
        q.pointer = Some((px, r.p[HIPS].1 * 200.0, false));
        // His feet, a second and a half later: a walk, not a skid.
        let feet = |r: &Ragdoll| (r.p[FOOT_L].0 + r.p[FOOT_R].0) / 2.0;
        let feet_before = feet(&r);
        run(&mut r, &mut cv, &q, 45);
        let moved = (r.p[HIPS].0 - before) * -side;
        assert!(moved > 0.03, "backed off {moved} from {before}");
        let pace = (feet(&r) - feet_before).abs() / 1.5;
        assert!(pace < 0.3, "walked at {pace} a second");
        // Afraid, he is harder to catch: a press just out of easy reach misses.
        let edge = r.p.iter().copied().max_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        q.pointer = Some(((edge.0 + REACH * 1.3) * 200.0, edge.1 * 200.0, true));
        run(&mut r, &mut cv, &q, 1);
        assert_eq!(r.grabbed, None);
    }

    #[test]
    fn food_calms_him_quickly() {
        let mut cv = Canvas::default();
        cv.resize(400, 200);
        let mut r = Ragdoll::default();
        let mut q = quiet();
        run(&mut r, &mut cv, &q, 30);
        r.mood = 0.2;
        // Press the apple button.
        let (l, t, rr, b) = r.button();
        q.pointer = Some(((l + rr) * 100.0, (t + b) * 100.0, true));
        run(&mut r, &mut cv, &q, 1);
        assert_eq!(r.food.len(), 1, "an apple dropped");
        assert_eq!(r.grabbed, None, "the button grabs nothing");
        q.pointer = Some((5.0, 5.0, false));
        run(&mut r, &mut cv, &q, 1);
        q.pointer = None;
        // It lands, he walks over (slowly: he's scared) and eats it.
        run(&mut r, &mut cv, &q, 750);
        assert!(r.food.is_empty(), "eaten");
        assert!(r.mood > 0.5, "mood {}", r.mood);
    }

    #[test]
    fn he_is_caught_by_his_head_hands_and_feet() {
        let mut cv = Canvas::default();
        cv.resize(400, 200);
        let q = quiet();
        for end in ENDS {
            let mut r = Ragdoll::default();
            run(&mut r, &mut cv, &q, 30);
            let mut q = q;
            q.pointer = Some((r.p[end].0 * 200.0, r.p[end].1 * 200.0, true));
            run(&mut r, &mut cv, &q, 1);
            assert_eq!(r.grabbed, Some(end));
        }
        // Halfway between his hand and his elbow, the hand is the one caught.
        let mut r = Ragdoll::default();
        run(&mut r, &mut cv, &q, 30);
        let mut q = q;
        let (h, e) = (r.p[HAND_L], r.p[ELBOW_L]);
        q.pointer = Some(((h.0 * 0.55 + e.0 * 0.45) * 200.0, (h.1 * 0.55 + e.1 * 0.45) * 200.0, true));
        run(&mut r, &mut cv, &q, 1);
        assert_eq!(r.grabbed, Some(HAND_L));
    }

    #[test]
    fn speech_bubbles_say_how_he_feels() {
        let mut cv = Canvas::default();
        cv.resize(400, 200);
        let mut r = Ragdoll::default();
        let mut q = quiet();
        run(&mut r, &mut cv, &q, 30);
        assert!(r.bubble.is_none());
        // Picked up while calm: startled.
        q.pointer = Some((r.p[HEAD].0 * 200.0, r.p[HEAD].1 * 200.0, true));
        run(&mut r, &mut cv, &q, 1);
        assert_eq!(r.bubble.map(|b| b.0), Some(Feeling::Alert));
        q.pointer = None;
        let mut gone = false;
        for _ in 0..240 {
            run(&mut r, &mut cv, &q, 1);
            gone |= r.bubble.is_none();
        }
        assert!(gone, "it goes away");
        // Afraid and left alone: nervous, before long.
        r.mood = 0.1;
        let mut said = None;
        for _ in 0..600 {
            run(&mut r, &mut cv, &q, 1);
            if let Some((f, _)) = r.bubble {
                said = Some(f);
                break;
            }
        }
        assert_eq!(said, Some(Feeling::Nervous));
        // Fed: happy.
        let (l, t, rr, b) = r.button();
        q.pointer = Some(((l + rr) * 100.0, (t + b) * 100.0, true));
        run(&mut r, &mut cv, &q, 1);
        q.pointer = None;
        let mut happy = false;
        // Afraid, he edges over slowly: give him half a minute.
        for _ in 0..900 {
            run(&mut r, &mut cv, &q, 1);
            happy |= r.bubble.is_some_and(|b| b.0 == Feeling::Happy);
        }
        assert!(r.food.is_empty(), "eaten");
        assert!(happy, "a smile after eating");
    }

    #[test]
    fn afraid_he_still_goes_for_food_with_the_mouse_about() {
        let mut cv = Canvas::default();
        cv.resize(400, 200);
        let mut r = Ragdoll::default();
        let mut q = quiet();
        run(&mut r, &mut cv, &q, 30);
        r.mood = 0.05;
        let (l, t, rr, b) = r.button();
        q.pointer = Some(((l + rr) * 100.0, (t + b) * 100.0, true));
        run(&mut r, &mut cv, &q, 1);
        for _ in 0..900 {
            // The mouse lingers a little above him.
            q.pointer = Some((r.p[HEAD].0 * 200.0 + 30.0, (r.p[HEAD].1 - 0.15).max(0.02) * 200.0, false));
            run(&mut r, &mut cv, &q, 1);
            if r.food.is_empty() {
                break;
            }
        }
        assert!(r.food.is_empty(), "he ate it");
    }

    #[test]
    fn left_alone_he_calms_down_slowly() {
        let mut cv = Canvas::default();
        cv.resize(400, 200);
        let mut r = Ragdoll::default();
        let q = quiet();
        run(&mut r, &mut cv, &q, 30);
        r.mood = 0.2;
        run(&mut r, &mut cv, &q, 300);
        assert!(r.mood > 0.25, "recovering: {}", r.mood);
        assert!(r.mood < 0.45, "but slowly: {}", r.mood);
    }
}

/// `AF2_RAGDOLL_SHOT=<file.ppm> cargo test -p audiform2 ragdoll_shot -- --ignored --nocapture`
/// writes a picture of him mid-frenzy under a hard, steady beat, after
/// `AF2_RAGDOLL_FRAMES` pictures (262 by default), and prints how often
/// his hips were near the ceiling and how often well off the floor (for
/// tuning how hard the shockwaves push). `AF2_RAGDOLL_SCENE=afraid`
/// instead shows him frightened, with the mouse near and an apple falling.
#[cfg(test)]
#[test]
#[ignore]
fn ragdoll_shot() {
    use super::{Analyser, Scheme, BANDS};
    let Ok(path) = std::env::var("AF2_RAGDOLL_SHOT") else { return };
    let mut cv = Canvas::default();
    cv.resize(640, 360);
    let pal = Palette::new(Scheme::Fire, None, None);
    let mut r = Ragdoll::default();
    let mut an = Analyser::default();
    let quiet = [-60.0f32; BANDS];
    let mut loud = [-20.0f32; BANDS];
    loud[..10].fill(-2.0);
    let frames: usize = std::env::var("AF2_RAGDOLL_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(262);
    let scene = std::env::var("AF2_RAGDOLL_SCENE").unwrap_or_default();
    if let Some(f) = match scene.as_str() {
        "happy" => Some(Feeling::Happy),
        "alert" => Some(Feeling::Alert),
        "nervous" => Some(Feeling::Nervous),
        _ => None,
    } {
        // Standing in silence, saying one thing.
        let mut q = an.feed(&[], 1.0, 1.0 / 30.0, 1.0);
        q.beat = false;
        if f == Feeling::Nervous {
            r.mood = 0.1;
        }
        for _ in 0..90 {
            r.draw(&mut cv, &q, &pal);
        }
        r.bubble = Some((f, 0.5));
        r.chat_in = 100.0;
        r.draw(&mut cv, &q, &pal);
    } else if scene == "afraid" {
        // Silence, reused every picture: no beat in it.
        let mut q = an.feed(&[], 1.0, 1.0 / 30.0, 1.0);
        q.beat = false;
        for _ in 0..30 {
            r.draw(&mut cv, &q, &pal);
        }
        r.mood = 0.08;
        for _ in 0..frames.min(90) {
            r.draw(&mut cv, &q, &pal);
        }
        let (l, t, rr, b) = r.button();
        q.pointer = Some(((l + rr) * 180.0, (t + b) * 180.0, true));
        r.draw(&mut cv, &q, &pal);
        let near = ((r.p[HIPS].0 + 0.2) * 360.0, r.p[NECK].1 * 360.0);
        q.pointer = Some((near.0, near.1, false));
        for _ in 0..12 {
            r.draw(&mut cv, &q, &pal);
        }
    } else {
        let mut ceil = 0;
        let mut air = 0;
        for k in 0..frames {
            let a = an.feed(if k % 14 < 3 { &loud } else { &quiet }, 1.0, 1.0 / 30.0, 1.0);
            r.draw(&mut cv, &a, &pal);
            if r.p[HIPS].1 < 0.2 {
                ceil += 1;
            }
            if r.p[HIPS].1 < r.floor_y() - 0.3 {
                air += 1;
            }
        }
        eprintln!("near the ceiling {ceil}, in the air {air}, of {frames} pictures");
    }
    let img = cv.image();
    let mut out = format!("P6\n{} {}\n255\n", img.size[0], img.size[1]).into_bytes();
    for p in &img.pixels {
        out.extend_from_slice(&[p.r(), p.g(), p.b()]);
    }
    std::fs::write(path, out).unwrap();
}
