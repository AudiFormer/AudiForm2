// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The host process: plug-ins of one maker (or one plug-in) run here.
//!
//! Two threads. The stdin thread reads every frame and is the plug-ins'
//! audio thread: it runs `process` itself and handles resets and new
//! control values. Everything else goes to the main thread, which loads
//! plug-ins, keeps their settings and windows, and runs the window
//! messages. A load on the main thread never holds up the audio of the
//! other plug-ins in the process.

use std::collections::HashMap;
use std::io::{self, BufWriter};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::com::Edit;
use super::editor;
use super::module::Module;
use super::plugin::{Audio, Plugin};
use crate::protocol::*;

type Out = Arc<Mutex<BufWriter<io::Stdout>>>;

fn send(out: &Out, kind: u8, id: u32, payload: &[u8]) {
    let ok = out
        .lock()
        .map(|mut w| write_frame(&mut *w, kind, id, payload).is_ok())
        .unwrap_or(false);
    if !ok {
        // The app went away: nothing left to do.
        crate::exit_now(0);
    }
}

fn reply(out: &Out, id: u32, rid: u32, m: &Message) {
    send(out, CONTROL, id, &encode_control(rid, m));
}

enum ToMain {
    Control { id: u32, rid: u32, req: Request },
    Quit,
}

pub fn serve() -> i32 {
    let out: Out = Arc::new(Mutex::new(BufWriter::new(io::stdout())));
    let (to_main, from_reader) = mpsc::channel::<ToMain>();
    let (audio_tx, audio_rx) = mpsc::channel::<(u32, Audio)>();
    let out2 = out.clone();
    let reader = std::thread::Builder::new()
        .name("plugin-audio".into())
        .spawn(move || audio_loop(out2, to_main, audio_rx));
    if reader.is_err() {
        return 1;
    }
    main_loop(out, from_reader, audio_tx);
    // Unloading plug-ins on the way out can hang or crash in them; the
    // process ends instead.
    crate::exit_now(0)
}

fn audio_loop(out: Out, to_main: mpsc::Sender<ToMain>, new_audio: mpsc::Receiver<(u32, Audio)>) {
    let mut stdin = io::stdin().lock();
    let mut audios: HashMap<u32, Audio> = HashMap::new();
    let mut a = AudioIn::default();
    let mut processed: Vec<f32> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    while let Ok(Some(frame)) = read_frame(&mut stdin) {
        while let Ok((id, au)) = new_audio.try_recv() {
            audios.insert(id, au);
        }
        let id = frame.id;
        match frame.kind {
            HELLO => send(&out, HELLO, 0, &VERSION.to_le_bytes()),
            AUDIO => {
                if decode_audio(&frame.payload, &mut a).is_none() {
                    continue;
                }
                processed.resize(a.frames * 2, 0.0);
                let ok = match audios.get_mut(&id) {
                    Some(au) => {
                        au.process(&a.samples, a.key.as_deref(), &a.changes, &mut processed)
                    }
                    None => {
                        processed.copy_from_slice(&a.samples);
                        false
                    }
                };
                encode_audio_reply(&mut bytes, a.seq, ok, &processed);
                send(&out, AUDIO, id, &bytes);
            }
            CONTROL => {
                let Some((rid, req)) = decode_control::<Request>(&frame.payload) else {
                    continue;
                };
                match req {
                    Request::Reset => {
                        if let Some(au) = audios.get_mut(&id) {
                            au.reset();
                        }
                        reply(&out, id, rid, &Message::Ok);
                    }
                    Request::SetParams { changes } => {
                        if let Some(au) = audios.get_mut(&id) {
                            au.queue(&changes);
                        }
                        let _ = to_main.send(ToMain::Control {
                            id,
                            rid,
                            req: Request::SetParams { changes },
                        });
                    }
                    Request::Unload => {
                        audios.remove(&id);
                        let _ = to_main.send(ToMain::Control { id, rid, req });
                    }
                    req => {
                        let _ = to_main.send(ToMain::Control { id, rid, req });
                    }
                }
            }
            _ => {}
        }
    }
    let _ = to_main.send(ToMain::Quit);
}

struct Loaded {
    plugin: Plugin,
    params: Vec<ParamInfo>,
}

fn main_loop(out: Out, rx: mpsc::Receiver<ToMain>, audio_tx: mpsc::Sender<(u32, Audio)>) {
    let mut modules: HashMap<PathBuf, Arc<Module>> = HashMap::new();
    let mut plugins: HashMap<u32, Loaded> = HashMap::new();
    loop {
        let wait = if plugins.values().any(|p| p.plugin.window.is_some()) {
            10
        } else if !plugins.is_empty() {
            15
        } else {
            50
        };
        let mut next = rx.recv_timeout(Duration::from_millis(wait));
        loop {
            match next {
                Ok(ToMain::Control { id, rid, req }) => {
                    handle(&out, &mut modules, &mut plugins, &audio_tx, id, rid, req)
                }
                Ok(ToMain::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
            }
            next = match rx.try_recv() {
                Ok(m) => Ok(m),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            };
        }
        for (&id, l) in plugins.iter_mut() {
            for e in l.plugin.take_edits() {
                let m = match e {
                    Edit::Param(pid, v) => {
                        if let Ok(mut q) = l.plugin.to_audio.lock() {
                            q.push((pid, v));
                        }
                        Message::ParamEdited { id: pid, value: v }
                    }
                    Edit::Begin(pid) => Message::BeginEdit { id: pid },
                    Edit::End(pid) => Message::EndEdit { id: pid },
                    Edit::Latency => Message::LatencyChanged {
                        latency: l.plugin.latency(),
                    },
                    Edit::Dirty => Message::Dirty,
                    Edit::Resize(..) => continue,
                    Edit::EditorClosed => {
                        l.plugin.window = None;
                        Message::EditorClosed
                    }
                    Edit::Bypass(b) => Message::BypassPicked { bypassed: b },
                    Edit::Space(record) => Message::SpacePressed { record },
                };
                reply(&out, id, 0, &m);
            }
        }
        if !plugins.is_empty() {
            editor::pump_messages();
        }
    }
}

fn handle(
    out: &Out,
    modules: &mut HashMap<PathBuf, Arc<Module>>,
    plugins: &mut HashMap<u32, Loaded>,
    audio_tx: &mpsc::Sender<(u32, Audio)>,
    id: u32,
    rid: u32,
    req: Request,
) {
    let err = |m: &str| Message::Error {
        message: m.to_string(),
    };
    let answer = match req {
        Request::Load {
            path,
            cid,
            rate,
            state,
            params,
        } => {
            let path = PathBuf::from(path);
            let module = match modules.get(&path) {
                Some(m) => Ok(m.clone()),
                None => Module::open(&path).map(|m| {
                    let m = Arc::new(m);
                    modules.insert(path.clone(), m.clone());
                    m
                }),
            };
            let cid = cid_from_hex(&cid).ok_or_else(|| "The plug-in id is damaged.".to_string());
            let blob = state.as_deref().and_then(base64_decode);
            match (module, cid) {
                (Ok(m), Ok(cid)) => {
                    match Plugin::load(&m, &cid, rate, blob.as_deref(), &params, &|_| {}) {
                        Ok((plugin, audio, info)) => {
                            // The audio half first: the app may send audio as
                            // soon as it hears back.
                            let _ = audio_tx.send((id, audio));
                            plugins.insert(
                                id,
                                Loaded {
                                    plugin,
                                    params: info.params.clone(),
                                },
                            );
                            Message::Loaded(info)
                        }
                        Err(e) => err(&e),
                    }
                }
                (Err(e), _) | (_, Err(e)) => err(&e),
            }
        }
        Request::Unload => {
            plugins.remove(&id);
            Message::Ok
        }
        Request::GetState => match plugins.get(&id) {
            Some(l) => match l.plugin.state() {
                Ok(b) => Message::State {
                    data: base64_encode(&b),
                },
                Err(e) => err(&e),
            },
            None => err("The plug-in is not running."),
        },
        Request::SetState { data } => match (plugins.get(&id), base64_decode(&data)) {
            (Some(l), Some(b)) => match l.plugin.set_state(&b) {
                Ok(()) => Message::Ok,
                Err(e) => err(&e),
            },
            (None, _) => err("The plug-in is not running."),
            (_, None) => err("The settings are damaged."),
        },
        Request::SetParams { changes } => {
            if let Some(l) = plugins.get(&id) {
                l.plugin.set_params(&changes);
            }
            Message::Ok
        }
        Request::OpenEditor {
            title,
            owner,
            bypassed,
        } => match plugins.get_mut(&id) {
            Some(l) => open_editor(l, &title, owner, bypassed),
            None => err("The plug-in is not running."),
        },
        Request::CloseEditor => {
            if let Some(l) = plugins.get_mut(&id) {
                l.plugin.window = None;
            }
            Message::Ok
        }
        Request::ShowBypassed { bypassed } => {
            if let Some(w) = plugins.get(&id).and_then(|l| l.plugin.window.as_ref()) {
                #[cfg(windows)]
                w.set_bypassed(bypassed);
                #[cfg(not(windows))]
                let _ = (w, bypassed);
            }
            Message::Ok
        }
        Request::Reset => Message::Ok,
    };
    let _ = plugins.get(&id).map(|l| l.params.len());
    reply(out, id, rid, &answer);
}

#[cfg(windows)]
fn open_editor(l: &mut Loaded, title: &str, owner: u64, bypassed: bool) -> Message {
    if let Some(w) = &l.plugin.window {
        w.set_bypassed(bypassed);
        w.show();
        return Message::Ok;
    }
    let Some(c) = l.plugin.controller.clone() else {
        return Message::Error {
            message: "This plug-in has no window of its own.".into(),
        };
    };
    match editor::Window::open(&c, title, owner, bypassed, l.plugin.edits.clone()) {
        Ok(w) => {
            l.plugin.window = Some(w);
            Message::Ok
        }
        Err(e) => Message::Error { message: e },
    }
}

#[cfg(not(windows))]
fn open_editor(_l: &mut Loaded, _title: &str, _owner: u64, _bypassed: bool) -> Message {
    Message::Error {
        message: "Plug-in windows open on Windows for now. The plug-in's controls are in its effect window.".into(),
    }
}
