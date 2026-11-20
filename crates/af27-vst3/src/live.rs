// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Plug-ins as effects in a chain.
//!
//! The audio thread never waits on a plug-in. Each running plug-in (an
//! [`Instance`]) has a pump thread that talks to its host process; the
//! audio thread hands it samples through a lock-free ring and takes back
//! what came out [`Instance::pipeline`] frames earlier. That buffer is a
//! fixed delay, reported as latency with the plug-in's own, so the mix
//! can line it up. When the plug-in is late (its window opening, a
//! licence check) or stopped, the dry sound delayed by the same amount
//! plays instead, so the track stays in time.
//!
//! Offline renders (exports, Apply) start a copy of their own with no
//! buffer, and wait for every block.
//!
//! One copy runs per slot (by the id in its data), kept across chain
//! rebuilds, stops and starts, so its window and inner state carry on.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

use af27_core::plugin::PluginData;
use af27_core::EffectSlot;
use af27_dsp::live::Live;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::client::{Conn, Event, Late, Plug, HUNG, LOAD_TIMEOUT, PROCESS_TIMEOUT};
use crate::protocol::Request;
use crate::registry::{Found, Registry};

/// The live buffer when the app has not set one (frames).
pub const PIPELINE_DEFAULT: usize = 1024;
/// Samples the rings between the audio thread and a pump hold (frames).
const RING_FRAMES: usize = 1 << 15;
/// The dry delay line (frames, a power of two).
const DRY_FRAMES: usize = 1 << 16;
const DRY_MASK: usize = DRY_FRAMES - 1;
/// A pause this long between blocks (stopped, bypassed) starts the
/// buffer afresh.
const GAP: Duration = Duration::from_millis(250);
/// A control moved in the plug-in's window this recently is not sent back.
const TURNING: Duration = Duration::from_millis(1500);
/// Late for this long: the plug-in is hung, and its process is ended.
const STALL_LIMIT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy)]
struct InMeta {
    gen: u32,
    frames: u32,
    keyed: bool,
}

#[derive(Clone, Copy)]
struct OutMeta {
    gen: u32,
    frames: u32,
    /// The plug-in did not run: take the dry sound for these frames.
    dry: bool,
}

/// The audio thread's end of the buffer.
struct AudioSide {
    in_data: Producer<f32>,
    in_meta: Producer<InMeta>,
    out_data: Consumer<f32>,
    out_meta: Consumer<OutMeta>,
    cur: Option<(OutMeta, usize)>,
    gen: u32,
    /// Frames still to give as silence while the buffer fills.
    fill: usize,
    /// Frames of the current run given dry because the plug-in was late;
    /// dropped when they arrive.
    owed: usize,
    dry: Vec<f32>,
    dry_w: usize,
    last: Option<Instant>,
    pipeline: usize,
}

/// The pump's end.
struct PumpSide {
    in_data: Consumer<f32>,
    in_meta: Consumer<InMeta>,
    out_data: Producer<f32>,
    out_meta: Producer<OutMeta>,
}

fn rings(pipeline: usize) -> (AudioSide, PumpSide) {
    let (ip, ic) = RingBuffer::<f32>::new(RING_FRAMES * 4);
    let (imp, imc) = RingBuffer::<InMeta>::new(512);
    let (op, oc) = RingBuffer::<f32>::new(RING_FRAMES * 2);
    let (omp, omc) = RingBuffer::<OutMeta>::new(512);
    (
        AudioSide {
            in_data: ip,
            in_meta: imp,
            out_data: oc,
            out_meta: omc,
            cur: None,
            gen: 0,
            fill: pipeline,
            owed: 0,
            dry: vec![0.0; DRY_FRAMES * 2],
            dry_w: 0,
            last: None,
            pipeline,
        },
        PumpSide {
            in_data: ic,
            in_meta: imc,
            out_data: op,
            out_meta: omp,
        },
    )
}

fn discard(c: &mut Consumer<f32>, n: usize) {
    let n = n.min(c.slots());
    if let Ok(ch) = c.read_chunk(n) {
        ch.commit_all();
    }
}

impl AudioSide {
    /// Starts a fresh run: what is still in flight is dropped as it comes.
    fn restart(&mut self, shared_gen: &AtomicU32) {
        self.gen = self.gen.wrapping_add(1);
        shared_gen.store(self.gen, Ordering::Release);
        self.fill = self.pipeline;
        self.owed = 0;
        self.dry.fill(0.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        key: Option<(&[f32], &[f32])>,
        latency: usize,
        offline: bool,
        pump: Option<&Thread>,
        shared_gen: &AtomicU32,
    ) {
        let n = l.len().min(r.len());
        let now = Instant::now();
        if !offline && self.last.is_some_and(|t| now.duration_since(t) > GAP) {
            self.restart(shared_gen);
        }
        self.last = Some(now);
        let base = self.dry_w;
        for i in 0..n {
            let w = (base + i) & DRY_MASK;
            self.dry[w * 2] = l[i];
            self.dry[w * 2 + 1] = r[i];
        }
        self.dry_w = base.wrapping_add(n);
        let total = (self.pipeline + latency).min(DRY_FRAMES - protocol_block() - 1);
        // Hand the block to the pump.
        let sent = if self.in_data.slots() >= n * 4 && self.in_meta.slots() >= 1 {
            if let Ok(chunk) = self.in_data.write_chunk_uninit(n * 4) {
                let (kl, kr) = key.unwrap_or((&[], &[]));
                chunk.fill_from_iter((0..n).flat_map(|i| {
                    [
                        l[i],
                        r[i],
                        kl.get(i).copied().unwrap_or(0.0),
                        kr.get(i).copied().unwrap_or(0.0),
                    ]
                }));
                let _ = self.in_meta.push(InMeta {
                    gen: self.gen,
                    frames: n as u32,
                    keyed: key.is_some(),
                });
                true
            } else {
                false
            }
        } else {
            false
        };
        if let Some(t) = pump {
            t.unpark();
        }
        if !sent {
            // The pump is far behind: start afresh, dry for now.
            self.restart(shared_gen);
            self.give_dry(l, r, 0, n, base, total);
            return;
        }
        let deadline = offline.then(|| now + PROCESS_TIMEOUT);
        let mut i = 0;
        while i < n {
            if self.fill > 0 {
                let k = self.fill.min(n - i);
                l[i..i + k].fill(0.0);
                r[i..i + k].fill(0.0);
                self.fill -= k;
                i += k;
                continue;
            }
            let (m, rem) = match self.cur {
                Some(c) => c,
                None => match self.out_meta.pop() {
                    Ok(m) => (m, m.frames as usize),
                    Err(_) => {
                        if let Some(d) = deadline {
                            if Instant::now() < d {
                                if let Some(t) = pump {
                                    t.unpark();
                                }
                                std::thread::sleep(Duration::from_micros(100));
                                continue;
                            }
                        }
                        // Late: dry, and drop these frames when they come.
                        self.give_dry(l, r, i, n, base, total);
                        self.owed += n - i;
                        return;
                    }
                },
            };
            if m.gen != self.gen {
                discard(&mut self.out_data, rem * 2);
                self.cur = None;
                continue;
            }
            if self.owed > 0 {
                let d = self.owed.min(rem);
                discard(&mut self.out_data, d * 2);
                self.owed -= d;
                self.cur = (rem > d).then_some((m, rem - d));
                continue;
            }
            let k = rem.min(n - i);
            if m.dry {
                discard(&mut self.out_data, k * 2);
                self.give_dry(l, r, i, i + k, base, total);
            } else if let Ok(ch) = self.out_data.read_chunk(k * 2) {
                let (a, b) = ch.as_slices();
                for j in 0..k {
                    let at = |x: usize| if x < a.len() { a[x] } else { b[x - a.len()] };
                    l[i + j] = at(j * 2);
                    r[i + j] = at(j * 2 + 1);
                }
                ch.commit_all();
            } else {
                self.give_dry(l, r, i, i + k, base, total);
            }
            i += k;
            self.cur = (rem > k).then_some((m, rem - k));
        }
    }

    /// Frames `from..to` of the block that started at dry frame `base`,
    /// as the dry sound `total` frames earlier.
    fn give_dry(
        &self,
        l: &mut [f32],
        r: &mut [f32],
        from: usize,
        to: usize,
        base: usize,
        total: usize,
    ) {
        for i in from..to {
            let at = base.wrapping_add(i).wrapping_sub(total) & DRY_MASK;
            l[i] = self.dry[at * 2];
            r[i] = self.dry[at * 2 + 1];
        }
    }
}

fn protocol_block() -> usize {
    crate::protocol::MAX_BLOCK
}

/// Where a running copy stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Starting,
    Running,
    /// Crashed, hung or could not start; the reason in words.
    Stopped(String),
}

/// What the app hears from running plug-ins.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    Edited {
        id: u32,
        value: f64,
    },
    BeginEdit(u32),
    EndEdit(u32),
    /// Fresh settings (base64), fetched after the plug-in changed them.
    State(String),
    Latency(u32),
    EditorClosed,
    BypassPicked(bool),
    /// Space in the plug-in's window: play or stop (true: record).
    Space(bool),
    Started,
    Stopped(String),
    /// A request (opening the window) failed.
    Error(String),
}

/// One running copy of a plug-in.
pub struct Instance {
    pub key: String,
    pub cid: String,
    pub name: String,
    pub vendor: String,
    pub rate: u32,
    pub offline: bool,
    pub sidechain: bool,
    bundle: PathBuf,
    plug: Mutex<Option<Arc<Plug>>>,
    status: Mutex<Status>,
    audio: Mutex<AudioSide>,
    gen: AtomicU32,
    pub pipeline: usize,
    latency: AtomicU32,
    keyed_latency: AtomicU32,
    pending: Mutex<Vec<(u32, f64)>>,
    /// The values the plug-in has (sent or reported).
    known: Mutex<HashMap<u32, f64>>,
    /// The settings last fetched from or given to it.
    last_state: Mutex<Option<String>>,
    /// When each control last moved in the plug-in's own window.
    turned: Mutex<HashMap<u32, Instant>>,
    notices: Mutex<Vec<Notice>>,
    pump: OnceLock<Thread>,
    quit: AtomicBool,
    /// When the plug-in last processed live (ns since the manager began).
    pub editor_open: AtomicBool,
    params: Mutex<Vec<crate::protocol::ParamInfo>>,
}

impl Instance {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn running(&self) -> bool {
        self.status() == Status::Running
    }

    pub fn plug(&self) -> Option<Arc<Plug>> {
        self.plug.lock().unwrap().clone()
    }

    /// The plug-in's own latency (keyed or not) plus the live buffer.
    pub fn total_latency(&self, keyed: bool) -> usize {
        let own = if keyed && self.sidechain {
            self.keyed_latency.load(Ordering::Relaxed)
        } else {
            self.latency.load(Ordering::Relaxed)
        };
        self.pipeline + own as usize
    }

    pub fn plugin_latency(&self) -> u32 {
        self.latency.load(Ordering::Relaxed)
    }

    /// Its controls, as it reported them when it started (the scan's
    /// until then).
    pub fn params(&self) -> Vec<crate::protocol::ParamInfo> {
        self.params.lock().unwrap().clone()
    }

    /// A new control value from the app; it goes with the next block.
    pub fn set_param(&self, id: u32, v: f64) {
        self.known.lock().unwrap().insert(id, v);
        let mut p = self.pending.lock().unwrap();
        p.retain(|(i, _)| *i != id);
        p.push((id, v));
        drop(p);
        if let Some(t) = self.pump.get() {
            t.unpark();
        }
    }

    pub fn value(&self, id: u32) -> Option<f64> {
        self.known.lock().unwrap().get(&id).copied()
    }

    /// Clears tails (called from the audio thread on a jump: never waits).
    pub fn reset_audio(&self) {
        if let Ok(mut a) = self.audio.try_lock() {
            a.restart(&self.gen);
        }
    }

    fn note(&self, n: Notice) {
        self.notices.lock().unwrap().push(n);
    }

    /// Takes in what the plug-in reported (latency, edits).
    fn absorb(self: &Arc<Self>, events: Vec<Event>) {
        for e in events {
            let n = match e {
                Event::ParamEdited { id, value } => {
                    self.known.lock().unwrap().insert(id, value);
                    self.turned.lock().unwrap().insert(id, Instant::now());
                    Notice::Edited { id, value }
                }
                Event::BeginEdit(id) => Notice::BeginEdit(id),
                Event::EndEdit(id) => Notice::EndEdit(id),
                Event::LatencyChanged(l) => {
                    self.latency.store(l, Ordering::Relaxed);
                    if self.sidechain {
                        self.keyed_latency.store(l, Ordering::Relaxed);
                    }
                    Notice::Latency(l)
                }
                Event::Dirty => {
                    self.fetch_state();
                    continue;
                }
                Event::EditorClosed => {
                    self.editor_open.store(false, Ordering::Relaxed);
                    self.fetch_state();
                    Notice::EditorClosed
                }
                Event::BypassPicked(b) => Notice::BypassPicked(b),
                Event::Space(r) => Notice::Space(r),
                Event::Stopped(why) => {
                    self.editor_open.store(false, Ordering::Relaxed);
                    *self.status.lock().unwrap() = Status::Stopped(why.clone());
                    Notice::Stopped(why)
                }
            };
            self.note(n);
        }
    }

    /// Takes what happened since last time.
    pub fn take_notices(self: &Arc<Self>) -> Vec<Notice> {
        if let Some(p) = self.plug() {
            self.absorb(p.take_events());
        }
        std::mem::take(&mut *self.notices.lock().unwrap())
    }

    /// Fetches the settings in the background; they come as a notice.
    pub fn fetch_state(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || {
            if let Some(p) = me.plug() {
                if let Ok(s) = p.state() {
                    *me.last_state.lock().unwrap() = Some(s.clone());
                    me.note(Notice::State(s));
                }
            }
        });
    }

    /// The settings now (waits for the plug-in).
    pub fn state_now(&self) -> Option<String> {
        let s = self.plug()?.state().ok()?;
        *self.last_state.lock().unwrap() = Some(s.clone());
        Some(s)
    }

    /// Gives the plug-in settings, unless they are the ones it has.
    pub fn push_state(self: &Arc<Self>, state: &str) {
        {
            let mut last = self.last_state.lock().unwrap();
            if last.as_deref() == Some(state) {
                return;
            }
            *last = Some(state.to_string());
        }
        let me = self.clone();
        let s = state.to_string();
        std::thread::spawn(move || {
            if let Some(p) = me.plug() {
                let _ = p.set_state(&s);
            }
        });
    }

    /// Brings the plug-in in line with a slot's saved values and settings
    /// (after an undo or a preset): only what differs is sent.
    pub fn sync(self: &Arc<Self>, d: &PluginData) {
        if let Some(s) = &d.state {
            self.push_state(s);
        }
        let known = self.known.lock().unwrap().clone();
        let turned = self.turned.lock().unwrap().clone();
        for (id, v) in d.pairs() {
            // A control being turned in the plug-in's window is ahead of
            // the session: sending the session's copy would make it jump.
            if turned.get(&id).is_some_and(|t| t.elapsed() < TURNING) {
                continue;
            }
            if known.get(&id).is_none_or(|k| (k - v).abs() > 1e-9) {
                self.set_param(id, v);
            }
        }
    }

    /// Opens the plug-in's own window (in the background).
    pub fn open_editor(self: &Arc<Self>, title: &str, owner: u64, bypassed: bool) {
        let me = self.clone();
        let title = title.to_string();
        std::thread::spawn(move || {
            // A plug-in still starting opens once it is up.
            let t = Instant::now();
            while me.status() == Status::Starting && t.elapsed() < LOAD_TIMEOUT {
                std::thread::sleep(Duration::from_millis(50));
            }
            let Some(p) = me.plug() else {
                me.note(Notice::Error(match me.status() {
                    Status::Stopped(why) => why,
                    _ => "The plug-in is not running.".into(),
                }));
                return;
            };
            match p.request(&Request::OpenEditor {
                title,
                owner,
                bypassed,
            }) {
                Ok(_) => me.editor_open.store(true, Ordering::Relaxed),
                Err(e) => me.note(Notice::Error(e)),
            }
        });
    }

    pub fn close_editor(self: &Arc<Self>) {
        if let Some(p) = self.plug() {
            p.tell(&Request::CloseEditor);
        }
        self.editor_open.store(false, Ordering::Relaxed);
    }

    pub fn show_bypassed(&self, b: bool) {
        if let Some(p) = self.plug() {
            p.tell(&Request::ShowBypassed { bypassed: b });
        }
    }

    fn take_pending(&self) -> Vec<(u32, f64)> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(t) = self.pump.get() {
            t.unpark();
        }
    }
}

/// The pump: runs blocks through the host process as the audio thread
/// hands them over.
fn pump(me: Weak<Instance>, mut side: PumpSide) {
    let mut input: Vec<f32> = Vec::new();
    let mut key: Vec<f32> = Vec::new();
    let mut out: Vec<f32> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    let mut last_gen = u32::MAX;
    let mut late_since: Option<Instant> = None;
    loop {
        let Some(inst) = me.upgrade() else {
            return;
        };
        if inst.quit.load(Ordering::SeqCst) {
            return;
        }
        let Ok(m) = side.in_meta.pop() else {
            // Idle: hand over control changes and take in events.
            if let Some(p) = inst.plug() {
                let changes = inst.take_pending();
                if !changes.is_empty() {
                    p.tell(&Request::SetParams { changes });
                }
                inst.absorb(p.take_events());
            }
            drop(inst);
            std::thread::park_timeout(Duration::from_millis(20));
            continue;
        };
        let n = m.frames as usize;
        input.clear();
        key.clear();
        if let Ok(ch) = side.in_data.read_chunk((n * 4).min(side.in_data.slots())) {
            let (a, b) = ch.as_slices();
            for (j, v) in a.iter().chain(b).enumerate() {
                if j % 4 < 2 {
                    input.push(*v);
                } else {
                    key.push(*v);
                }
            }
            ch.commit_all();
        }
        input.resize(n * 2, 0.0);
        let current = inst.gen.load(Ordering::Acquire);
        let mut dry = true;
        out.clear();
        // Blocks of a run the audio thread has left are not worth sending.
        if m.gen == current {
            if let Some(p) = inst.plug().filter(|p| p.alive()) {
                if m.gen != last_gen {
                    last_gen = m.gen;
                    p.tell(&Request::Reset);
                }
                let changes = inst.take_pending();
                let k = m.keyed.then_some(&key[..]);
                match p.process(&input, k, &changes, &mut out, &mut bytes, PROCESS_TIMEOUT) {
                    Ok(ok) => {
                        dry = !ok;
                        late_since = None;
                    }
                    Err(Late::Timeout) => {
                        let since = *late_since.get_or_insert_with(Instant::now);
                        if since.elapsed() > STALL_LIMIT {
                            p.kill(HUNG);
                        }
                    }
                    Err(Late::Stopped) => {}
                }
                inst.absorb(p.take_events());
            }
        }
        if dry || out.len() != n * 2 {
            out.clear();
            out.resize(n * 2, 0.0);
            dry = true;
        }
        // Data first, then its description, so the audio thread never
        // sees a description without its samples.
        let mut wait = 0;
        while side.out_data.slots() < n * 2 || side.out_meta.slots() < 1 {
            // The audio thread is not reading (stopped mid-run): drop it.
            wait += 1;
            if wait > 50 || inst.quit.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if side.out_data.slots() >= n * 2 && side.out_meta.slots() >= 1 {
            if let Ok(ch) = side.out_data.write_chunk_uninit(n * 2) {
                ch.fill_from_iter(out.iter().copied());
            }
            let _ = side.out_meta.push(OutMeta {
                gen: m.gen,
                frames: m.frames,
                dry,
            });
        }
    }
}

/// Every running plug-in, the host processes and the registry.
pub struct Manager {
    exe: Mutex<Option<PathBuf>>,
    registry: Mutex<Registry>,
    registry_path: Mutex<Option<PathBuf>>,
    instances: Mutex<HashMap<String, Arc<Instance>>>,
    conns: Mutex<HashMap<(u32, String, u32), Weak<Conn>>>,
    pipeline: AtomicUsize,
    serial: AtomicU64,
}

static MANAGER: OnceLock<Manager> = OnceLock::new();

/// The app's one plug-in manager.
pub fn manager() -> &'static Manager {
    MANAGER.get_or_init(|| Manager {
        exe: Mutex::new(None),
        registry: Mutex::new(Registry::default()),
        registry_path: Mutex::new(None),
        instances: Mutex::new(HashMap::new()),
        conns: Mutex::new(HashMap::new()),
        pipeline: AtomicUsize::new(PIPELINE_DEFAULT),
        serial: AtomicU64::new(0),
    })
}

/// Scope of shared processes: playback and offline renders never share.
const LIVE: u32 = 0;
const OFFLINE: u32 = 1;

impl Manager {
    /// The program hosts are started from (`--plugin-host`): the app
    /// itself unless `AF2_PLUGIN_HOST` names another.
    pub fn host_exe(&self) -> PathBuf {
        if let Some(e) = self.exe.lock().unwrap().clone() {
            return e;
        }
        std::env::var_os("AF2_PLUGIN_HOST")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_default()
    }

    pub fn set_host_exe(&self, exe: PathBuf) {
        *self.exe.lock().unwrap() = Some(exe);
    }

    /// Loads the registry from `path` (the app's `plugins.json`).
    pub fn open_registry(&self, path: PathBuf) {
        *self.registry.lock().unwrap() = Registry::load(&path);
        *self.registry_path.lock().unwrap() = Some(path);
    }

    pub fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap()
    }

    pub fn registry_mutex(&self) -> &Mutex<Registry> {
        &self.registry
    }

    pub fn save_registry(&self) -> Result<(), String> {
        let Some(p) = self.registry_path.lock().unwrap().clone() else {
            return Ok(());
        };
        self.registry.lock().unwrap().save(&p)
    }

    /// The live buffer in frames: at least twice the audio device's
    /// buffer, so a plug-in has a whole callback to answer.
    pub fn set_device_buffer(&self, frames: Option<u32>) {
        let want = frames.map_or(PIPELINE_DEFAULT, |f| (f as usize * 2).next_power_of_two());
        self.pipeline
            .store(want.clamp(PIPELINE_DEFAULT, 8192), Ordering::SeqCst);
    }

    pub fn pipeline(&self) -> usize {
        self.pipeline.load(Ordering::SeqCst)
    }

    pub fn find(&self, cid: &str) -> Option<Found> {
        self.registry.lock().unwrap().find(cid).cloned()
    }

    /// The process a plug-in of `maker` runs in.
    fn conn_for(&self, maker: &str, rate: u32, scope: u32) -> Result<Arc<Conn>, String> {
        let exe = self.host_exe();
        if self.registry.lock().unwrap().runs_separately(maker) {
            return Conn::spawn(&exe);
        }
        let key = (scope, maker.trim().to_lowercase(), rate);
        let mut conns = self.conns.lock().unwrap();
        if let Some(c) = conns.get(&key).and_then(Weak::upgrade) {
            if c.alive() {
                return Ok(c);
            }
        }
        let c = Conn::spawn(&exe)?;
        conns.insert(key, Arc::downgrade(&c));
        conns.retain(|_, w| w.strong_count() > 0);
        Ok(c)
    }

    fn new_instance(
        &self,
        key: &str,
        found: &Found,
        d: &PluginData,
        rate: u32,
        offline: bool,
    ) -> Arc<Instance> {
        let pipeline = if offline { 0 } else { self.pipeline() };
        let (audio, pump_side) = rings(pipeline);
        let inst = Arc::new(Instance {
            key: key.to_string(),
            cid: found.info.cid.clone(),
            name: found.info.name.clone(),
            vendor: found.maker(),
            rate,
            offline,
            sidechain: found.info.sidechain,
            bundle: found.bundle.clone(),
            plug: Mutex::new(None),
            status: Mutex::new(Status::Starting),
            audio: Mutex::new(audio),
            gen: AtomicU32::new(0),
            pipeline,
            latency: AtomicU32::new(found.info.latency),
            keyed_latency: AtomicU32::new(found.info.latency),
            pending: Mutex::new(Vec::new()),
            known: Mutex::new(d.pairs().into_iter().collect()),
            last_state: Mutex::new(d.state.clone()),
            turned: Mutex::new(HashMap::new()),
            notices: Mutex::new(Vec::new()),
            pump: OnceLock::new(),
            quit: AtomicBool::new(false),
            editor_open: AtomicBool::new(false),
            params: Mutex::new(found.info.params.clone()),
        });
        let weak = Arc::downgrade(&inst);
        if let Ok(h) = std::thread::Builder::new()
            .name("plugin-pump".into())
            .spawn(move || pump(weak, pump_side))
        {
            let _ = inst.pump.set(h.thread().clone());
        }
        inst
    }

    /// Starts `inst` (waits for it).
    fn start(&self, inst: &Arc<Instance>, d: &PluginData) {
        let scope = if inst.offline { OFFLINE } else { LIVE };
        let r = self.conn_for(&inst.vendor, inst.rate, scope).and_then(|c| {
            Plug::load(
                &c,
                &inst.bundle,
                &inst.cid,
                inst.rate,
                d.state.clone(),
                d.pairs(),
            )
        });
        match r {
            Ok(p) => {
                inst.latency.store(p.info.latency, Ordering::Relaxed);
                inst.keyed_latency.store(
                    p.info.keyed_latency.unwrap_or(p.info.latency),
                    Ordering::Relaxed,
                );
                if !p.info.params.is_empty() {
                    *inst.params.lock().unwrap() = p.info.params.clone();
                }
                {
                    let mut k = inst.known.lock().unwrap();
                    for &(id, v) in &p.info.values {
                        k.insert(id, v);
                    }
                }
                *inst.plug.lock().unwrap() = Some(Arc::new(p));
                *inst.status.lock().unwrap() = Status::Running;
                inst.note(Notice::Started);
            }
            Err(e) => {
                *inst.status.lock().unwrap() = Status::Stopped(e.clone());
                inst.note(Notice::Stopped(e));
            }
        }
    }

    /// The live copy for a slot, started in the background if it is not
    /// running. None for a plug-in that is not installed (the slot is
    /// skipped).
    pub fn live(&self, slot: &EffectSlot, rate: u32) -> Option<Arc<Instance>> {
        let cid = crate::cid_of(&slot.effect)?;
        let found = self.find(cid)?;
        let d = slot.plugin_data();
        let key = if d.inst.is_empty() {
            format!("{}#{}", slot.effect, slot.ext_id.as_deref().unwrap_or(""))
        } else {
            d.inst.clone()
        };
        let mut map = self.instances.lock().unwrap();
        let mut pooled = true;
        if let Some(i) = map.get(&key) {
            if i.rate == rate && i.cid.eq_ignore_ascii_case(&found.info.cid) {
                return Some(i.clone());
            }
            // Playing at another rate (an export at its own rate): a copy
            // of its own, so playback's copy never restarts for it.
            pooled = Arc::strong_count(i) == 1;
        }
        let inst = self.new_instance(&key, &found, &d, rate, false);
        if pooled {
            map.insert(key, inst.clone());
        }
        drop(map);
        let me = inst.clone();
        std::thread::spawn(move || manager().start(&me, &d));
        Some(inst)
    }

    /// A copy of a slot's plug-in for one offline render, started now.
    pub fn offline(&self, slot: &EffectSlot, rate: u32) -> Option<Arc<Instance>> {
        let cid = crate::cid_of(&slot.effect)?;
        let found = self.find(cid)?;
        let mut d = slot.plugin_data();
        // The live copy's settings are newer than the slot's, if it runs.
        if let Some(live) = self.get(&d.inst) {
            if live.running() {
                if let Some(s) = live.state_now() {
                    d.state = Some(s);
                }
                let known = live.known.lock().unwrap().clone();
                for (id, v) in known {
                    d.set_param(id, v);
                }
            }
        }
        let n = self.serial.fetch_add(1, Ordering::SeqCst);
        let inst = self.new_instance(&format!("offline-{n}"), &found, &d, rate, true);
        self.start(&inst, &d);
        Some(inst)
    }

    pub fn get(&self, key: &str) -> Option<Arc<Instance>> {
        self.instances.lock().unwrap().get(key).cloned()
    }

    /// Every live copy.
    pub fn instances(&self) -> Vec<Arc<Instance>> {
        self.instances.lock().unwrap().values().cloned().collect()
    }

    /// Stops live copies no slot uses any more (by key), except ones
    /// with their window open.
    pub fn retain(&self, keys: &HashSet<String>) {
        let mut map = self.instances.lock().unwrap();
        map.retain(|k, i| keys.contains(k) || i.editor_open.load(Ordering::Relaxed));
    }

    /// Forgets a stopped copy so it starts again with the next build.
    pub fn restart(&self, key: &str) {
        let mut map = self.instances.lock().unwrap();
        if map.get(key).is_some_and(|i| !i.running()) {
            map.remove(key);
        }
    }

    /// Stops every live copy of a maker's plug-ins (after its sharing
    /// choice changed), so they start the new way.
    pub fn drop_maker(&self, maker: &str) {
        let m = maker.trim().to_lowercase();
        self.instances
            .lock()
            .unwrap()
            .retain(|_, i| i.vendor.trim().to_lowercase() != m);
    }

    /// What every live copy reported since last time: (slot key, notice).
    pub fn take_notices(&self) -> Vec<(String, Notice)> {
        let mut out = Vec::new();
        for i in self.instances() {
            for n in i.take_notices() {
                out.push((i.key.clone(), n));
            }
        }
        out
    }
}

/// A plug-in slot in a chain.
pub struct VstEffect {
    inst: Option<Arc<Instance>>,
    slot: EffectSlot,
    rate: u32,
    offline: bool,
    keyed: bool,
}

impl VstEffect {
    pub fn instance(&self) -> Option<&Arc<Instance>> {
        self.inst.as_ref()
    }

    fn run(&mut self, l: &mut [f32], r: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let Some(inst) = &self.inst else {
            return;
        };
        let lat = inst.total_latency(self.keyed) - inst.pipeline;
        let pump = inst.pump.get();
        if let Ok(mut side) = inst.audio.try_lock() {
            side.process(l, r, key, lat, self.offline, pump, &inst.gen);
        }
    }
}

impl Live for VstEffect {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.run(l, r, None);
    }

    fn process_sidechain(&mut self, l: &mut [f32], r: &mut [f32], kl: &[f32], kr: &[f32]) {
        self.run(l, r, Some((kl, kr)));
    }

    fn takes_key(&self) -> bool {
        self.inst.as_ref().is_some_and(|i| i.sidechain)
    }

    fn set(&mut self, _vals: &[f32]) {}

    fn reset(&mut self) {
        if let Some(i) = &self.inst {
            i.reset_audio();
        }
    }

    fn latency(&self) -> usize {
        self.inst
            .as_ref()
            .map_or(0, |i| i.total_latency(self.keyed))
    }

    fn vary(&mut self, k: usize) {
        // One copy per slot: a speaker bed's other pairs pass through.
        if k > 0 {
            self.inst = None;
        }
    }

    fn set_offline(&mut self, offline: bool) {
        if offline == self.offline {
            return;
        }
        self.offline = offline;
        self.inst = if offline {
            manager().offline(&self.slot, self.rate)
        } else {
            manager().live(&self.slot, self.rate)
        };
    }
}

impl VstEffect {
    /// A slot's plug-in, live. None when it is not installed.
    pub fn new(slot: &EffectSlot, rate: u32) -> Option<VstEffect> {
        let inst = manager().live(slot, rate)?;
        Some(VstEffect {
            keyed: slot.sidechain.is_some(),
            inst: Some(inst),
            slot: slot.clone(),
            rate,
            offline: false,
        })
    }
}

/// The live processor for a plug-in slot (registered with the chain
/// builder by the app). None when the plug-in is not installed: the slot
/// stays in the session and is skipped.
pub fn build_live(slot: &EffectSlot, rate: u32) -> Option<Box<dyn Live>> {
    VstEffect::new(slot, rate).map(|v| Box::new(v) as Box<dyn Live>)
}
