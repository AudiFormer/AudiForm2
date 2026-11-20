// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The app's side of a host process: [`Conn`] is one process, [`Plug`] one
//! plug-in in it. No unsafe code; a crash or hang in a plug-in shows up
//! here as a closed pipe or a timeout, and the plug-in is marked stopped.

use std::collections::HashMap;
use std::io::BufWriter;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::protocol::*;

/// How long one block may take before it counts as late.
pub const PROCESS_TIMEOUT: Duration = Duration::from_secs(4);
/// Loading can include a licence check.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(90);
/// Settings, windows.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const HELLO_TIMEOUT: Duration = Duration::from_secs(20);

pub const CRASHED: &str = "The plug-in crashed and is bypassed.";
pub const HUNG: &str = "The plug-in stopped answering and is bypassed.";

/// Host processes started so far (for tests and the Plug-In Manager).
pub static STARTED: AtomicUsize = AtomicUsize::new(0);

/// Something a running plug-in did that the app may want to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    ParamEdited {
        id: u32,
        value: f64,
    },
    BeginEdit(u32),
    EndEdit(u32),
    LatencyChanged(u32),
    /// Settings changed that are not controls.
    Dirty,
    EditorClosed,
    BypassPicked(bool),
    /// Space in its window (true: Shift+Space, record).
    Space(bool),
    /// The plug-in stopped (crashed, hung); the reason in words.
    Stopped(String),
}

type Events = Arc<Mutex<Vec<Event>>>;

struct Route {
    audio: Sender<Vec<u8>>,
    events: Events,
}

/// One host process.
pub struct Conn {
    child: Mutex<Child>,
    stdin: Mutex<BufWriter<ChildStdin>>,
    routes: Arc<Mutex<HashMap<u32, Route>>>,
    waiting: Arc<Mutex<HashMap<u32, Sender<Message>>>>,
    next_id: AtomicU32,
    next_rid: AtomicU32,
    alive: Arc<AtomicBool>,
}

fn push(events: &Events, e: Event) {
    if let Ok(mut v) = events.lock() {
        v.push(e);
    }
}

impl Conn {
    /// Starts a host process (`exe --plugin-host`) and checks it speaks
    /// this build's protocol.
    pub fn spawn(exe: &Path) -> Result<Arc<Conn>, String> {
        let mut cmd = Command::new(exe);
        cmd.arg("--plugin-host")
            .arg("--parent")
            .arg(std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("The plug-in host could not start: {e}"))?;
        crate::bind_to_app(&child);
        STARTED.fetch_add(1, Ordering::SeqCst);
        let stdin = child.stdin.take().ok_or("No pipe to the plug-in host.")?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or("No pipe from the plug-in host.")?;
        let routes: Arc<Mutex<HashMap<u32, Route>>> = Arc::default();
        let waiting: Arc<Mutex<HashMap<u32, Sender<Message>>>> = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (hello_tx, hello_rx) = mpsc::channel::<u32>();
        {
            let routes = routes.clone();
            let waiting = waiting.clone();
            let alive = alive.clone();
            std::thread::Builder::new()
                .name("plugin-pipe".into())
                .spawn(move || {
                    while let Ok(Some(f)) = read_frame(&mut stdout) {
                        match f.kind {
                            HELLO => {
                                let v = f
                                    .payload
                                    .get(..4)
                                    .and_then(|b| b.try_into().ok())
                                    .map_or(0, u32::from_le_bytes);
                                let _ = hello_tx.send(v);
                            }
                            AUDIO => {
                                if let Some(r) = routes.lock().unwrap().get(&f.id) {
                                    let _ = r.audio.send(f.payload);
                                }
                            }
                            CONTROL => {
                                let Some((rid, m)) = decode_control::<Message>(&f.payload) else {
                                    continue;
                                };
                                if rid != 0 {
                                    if let Some(tx) = waiting.lock().unwrap().remove(&rid) {
                                        let _ = tx.send(m);
                                    }
                                    continue;
                                }
                                let e = match m {
                                    Message::ParamEdited { id, value } => {
                                        Event::ParamEdited { id, value }
                                    }
                                    Message::BeginEdit { id } => Event::BeginEdit(id),
                                    Message::EndEdit { id } => Event::EndEdit(id),
                                    Message::LatencyChanged { latency } => {
                                        Event::LatencyChanged(latency)
                                    }
                                    Message::Dirty => Event::Dirty,
                                    Message::EditorClosed => Event::EditorClosed,
                                    Message::BypassPicked { bypassed } => {
                                        Event::BypassPicked(bypassed)
                                    }
                                    Message::SpacePressed { record } => Event::Space(record),
                                    _ => continue,
                                };
                                if let Some(r) = routes.lock().unwrap().get(&f.id) {
                                    push(&r.events, e);
                                }
                            }
                            _ => {}
                        }
                    }
                    // The pipe closed: the process crashed or ended.
                    let was = alive.swap(false, Ordering::SeqCst);
                    let mut rs = routes.lock().unwrap();
                    if was {
                        for r in rs.values() {
                            push(&r.events, Event::Stopped(CRASHED.into()));
                        }
                    }
                    // Anyone waiting hears at once.
                    rs.clear();
                    waiting.lock().unwrap().clear();
                })
                .map_err(|e| e.to_string())?;
        }
        let conn = Arc::new(Conn {
            child: Mutex::new(child),
            stdin: Mutex::new(BufWriter::new(stdin)),
            routes,
            waiting,
            next_id: AtomicU32::new(0),
            next_rid: AtomicU32::new(1),
            alive,
        });
        conn.write(HELLO, 0, &VERSION.to_le_bytes())?;
        match hello_rx.recv_timeout(HELLO_TIMEOUT) {
            Ok(VERSION) => Ok(conn),
            Ok(v) => {
                conn.kill(HUNG);
                Err(format!(
                    "The plug-in host is from another version (protocol {v}, wanted {VERSION})."
                ))
            }
            Err(_) => {
                conn.kill(HUNG);
                Err("The plug-in host did not start.".into())
            }
        }
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn write(&self, kind: u8, id: u32, payload: &[u8]) -> Result<(), String> {
        let ok = self
            .stdin
            .lock()
            .map(|mut w| write_frame(&mut *w, kind, id, payload).is_ok())
            .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            self.kill(CRASHED);
            Err(CRASHED.into())
        }
    }

    /// Ends the process; every plug-in in it is marked stopped with `why`.
    pub fn kill(&self, why: &str) {
        if self.alive.swap(false, Ordering::SeqCst) {
            for r in self.routes.lock().unwrap().values() {
                push(&r.events, Event::Stopped(why.into()));
            }
        }
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }

    fn request(&self, id: u32, req: &Request, timeout: Duration) -> Result<Message, String> {
        if !self.alive() {
            return Err(CRASHED.into());
        }
        let rid = self.next_rid.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.waiting.lock().unwrap().insert(rid, tx);
        self.write(CONTROL, id, &encode_control(rid, req))?;
        match rx.recv_timeout(timeout) {
            Ok(Message::Error { message }) => Err(message),
            Ok(m) => Ok(m),
            Err(RecvTimeoutError::Timeout) => {
                self.waiting.lock().unwrap().remove(&rid);
                self.kill(HUNG);
                Err(HUNG.into())
            }
            Err(RecvTimeoutError::Disconnected) => Err(CRASHED.into()),
        }
    }

    /// Sends a request nobody waits for.
    fn tell(&self, id: u32, req: &Request) {
        if self.alive() {
            let rid = self.next_rid.fetch_add(1, Ordering::SeqCst);
            let _ = self.write(CONTROL, id, &encode_control(rid, req));
        }
    }

    /// Plug-ins currently in this process.
    pub fn count(&self) -> usize {
        self.routes.lock().map_or(0, |r| r.len())
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Why a block did not come back.
#[derive(Debug, Clone, PartialEq)]
pub enum Late {
    /// Not back in time; it may still come (the next call drops it).
    Timeout,
    /// The process is gone.
    Stopped,
}

/// One plug-in in a host process.
pub struct Plug {
    conn: Arc<Conn>,
    id: u32,
    audio: Mutex<Receiver<Vec<u8>>>,
    events: Events,
    pub info: Loaded,
    seq: AtomicU32,
}

impl Plug {
    /// Starts plug-in `cid` of bundle `path` in `conn`.
    pub fn load(
        conn: &Arc<Conn>,
        path: &Path,
        cid: &str,
        rate: u32,
        state: Option<String>,
        params: Vec<(u32, f64)>,
    ) -> Result<Plug, String> {
        let id = conn.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        let events: Events = Arc::default();
        conn.routes.lock().unwrap().insert(
            id,
            Route {
                audio: tx,
                events: events.clone(),
            },
        );
        let req = Request::Load {
            path: path.to_string_lossy().into_owned(),
            cid: cid.to_string(),
            rate: rate as f64,
            state,
            params,
        };
        let info = match conn.request(id, &req, LOAD_TIMEOUT) {
            Ok(Message::Loaded(info)) => info,
            Ok(_) => Err("The plug-in host gave an odd answer.".to_string())?,
            Err(e) => {
                conn.routes.lock().unwrap().remove(&id);
                return Err(e);
            }
        };
        Ok(Plug {
            conn: conn.clone(),
            id,
            audio: Mutex::new(rx),
            events,
            info,
            seq: AtomicU32::new(1),
        })
    }

    pub fn alive(&self) -> bool {
        self.conn.alive()
    }

    pub fn conn(&self) -> &Arc<Conn> {
        &self.conn
    }

    pub fn request(&self, req: &Request) -> Result<Message, String> {
        self.conn.request(self.id, req, REQUEST_TIMEOUT)
    }

    pub fn tell(&self, req: &Request) {
        self.conn.tell(self.id, req)
    }

    /// The settings blob, base64.
    pub fn state(&self) -> Result<String, String> {
        match self.request(&Request::GetState)? {
            Message::State { data } => Ok(data),
            _ => Err("The plug-in host gave an odd answer.".into()),
        }
    }

    pub fn set_state(&self, data: &str) -> Result<(), String> {
        self.request(&Request::SetState {
            data: data.to_string(),
        })
        .map(|_| ())
    }

    pub fn take_events(&self) -> Vec<Event> {
        self.events
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }

    /// Runs one block (interleaved stereo) through the plug-in; `out`
    /// gets the processed samples. Ok(false): the host passed it through
    /// (the plug-in did not run).
    pub fn process(
        &self,
        input: &[f32],
        key: Option<&[f32]>,
        changes: &[(u32, f64)],
        out: &mut Vec<f32>,
        bytes: &mut Vec<u8>,
        timeout: Duration,
    ) -> Result<bool, Late> {
        if !self.alive() {
            return Err(Late::Stopped);
        }
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        encode_audio(bytes, seq, input, key, changes);
        if self.conn.write(AUDIO, self.id, bytes).is_err() {
            return Err(Late::Stopped);
        }
        let rx = self.audio.lock().unwrap();
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(payload) => match decode_audio_reply(&payload, out) {
                    // An answer to an earlier, late block: dropped.
                    Some((s, _)) if s != seq => continue,
                    Some((_, ok)) if out.len() == input.len() => return Ok(ok),
                    _ => return Err(Late::Stopped),
                },
                Err(RecvTimeoutError::Timeout) => return Err(Late::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(Late::Stopped),
            }
        }
    }

    /// Ends the process this plug-in is in (it hung).
    pub fn kill(&self, why: &str) {
        self.conn.kill(why);
    }
}

impl Drop for Plug {
    fn drop(&mut self) {
        self.conn.tell(self.id, &Request::Unload);
        self.conn.routes.lock().unwrap().remove(&self.id);
    }
}
