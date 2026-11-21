// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! One running plug-in, in two halves: [`Plugin`] lives on the host's main
//! thread (controller, settings, window) and [`Audio`] on its audio thread
//! (the processor).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, BusInfo, IAudioProcessor, IAudioProcessorTrait,
    IComponent, IComponentTrait, IConnectionPoint, IConnectionPointTrait, IEditController,
    IEditControllerTrait, IParameterChanges, ParameterInfo, ProcessContext, ProcessData,
    ProcessSetup, SpeakerArrangement,
};
use vst3::Steinberg::{kResultOk, kResultTrue, IBStream, IPluginBaseTrait, IPluginFactoryTrait};
use vst3::{ComPtr, ComWrapper, Interface};

use super::com::{self, Edit, EventList, Edits, MemStream, ParamChanges, Sendable};
use super::module::{wstr, Module};
use crate::protocol::{join_state, split_state, Loaded, NoteEvent, ParamInfo, Timed, MAX_BLOCK};

/// MIDI controllers a mapping can name: 0..127, aftertouch, pitch bend.
const CONTROLLERS: usize = 130;
/// No control for this controller.
const UNMAPPED: u32 = u32::MAX;

const STEREO: SpeakerArrangement = 3;
const MONO: SpeakerArrangement = 1 << 19; // kSpeakerM

const K_AUDIO: i32 = 0;
const K_EVENT: i32 = 1;
const K_IN: i32 = 0;
const K_OUT: i32 = 1;

/// How long a key may be missing before the sidechain input is switched
/// off again.
const KEY_HOLD: Duration = Duration::from_secs(1);

/// Logs a step of loading, so a crash in a scan can be placed.
pub type Stage<'a> = &'a dyn Fn(&str);

unsafe fn create<I: Interface>(
    factory: &ComPtr<vst3::Steinberg::IPluginFactory>,
    cid: &[u8; 16],
) -> Option<ComPtr<I>> {
    let mut obj = std::ptr::null_mut();
    let r = factory.createInstance(cid.as_ptr().cast(), I::IID.as_ptr().cast(), &mut obj);
    if r != kResultOk || obj.is_null() {
        return None;
    }
    ComPtr::from_raw(obj as *mut I)
}

fn stream_ptr(s: &ComWrapper<MemStream>) -> *mut IBStream {
    s.to_com_ptr::<IBStream>()
        .map_or(std::ptr::null_mut(), |p| {
            let raw = p.as_ptr();
            // The wrapper keeps the object alive while this call runs.
            drop(p);
            raw
        })
}

/// The main-thread half.
pub struct Plugin {
    pub window: Option<super::editor::Window>,
    pub controller: Option<ComPtr<IEditController>>,
    separate: bool,
    points: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    pub component: ComPtr<IComponent>,
    pub processor: ComPtr<IAudioProcessor>,
    pub edits: Edits,
    /// Edits from the window that the audio half picks up.
    pub to_audio: Arc<Mutex<Vec<(u32, f64)>>>,
    pub name: String,
    _host: ComPtr<vst3::Steinberg::Vst::IHostApplication>,
    _module: Arc<Module>,
}

/// The audio-thread half.
pub struct Audio {
    processor: ComPtr<IAudioProcessor>,
    component: ComPtr<IComponent>,
    in_data: Vec<Vec<Vec<f32>>>,
    out_data: Vec<Vec<Vec<f32>>>,
    in_ptrs: Vec<Vec<*mut f32>>,
    out_ptrs: Vec<Vec<*mut f32>>,
    in_buses: Vec<AudioBusBuffers>,
    out_buses: Vec<AudioBusBuffers>,
    changes: ComWrapper<ParamChanges>,
    changes_ptr: ComPtr<IParameterChanges>,
    /// Note events for the block, and whether it has an event input.
    events: ComWrapper<EventList>,
    events_ptr: ComPtr<vst3::Steinberg::Vst::IEventList>,
    takes_events: bool,
    /// Per channel and controller, the control its MIDI mapping names.
    midi_map: Vec<u32>,
    ctx: Box<ProcessContext>,
    rate: f64,
    time: i64,
    pub sidechain: bool,
    key_on: bool,
    last_key: Option<Instant>,
    instrument: bool,
    pub from_editor: Arc<Mutex<Vec<(u32, f64)>>>,
    pending: Vec<(u32, f64)>,
}

unsafe impl Send for Audio {}

fn speaker_count(a: SpeakerArrangement) -> usize {
    a.count_ones() as usize
}

impl Plugin {
    /// Creates, sets up and switches on plug-in `cid` from `module`.
    pub fn load(
        module: &Arc<Module>,
        cid: &[u8; 16],
        rate: f64,
        state: Option<&[u8]>,
        params: &[(u32, f64)],
        stage: Stage,
    ) -> Result<(Plugin, Audio, Loaded), String> {
        unsafe { Self::load_inner(module, cid, rate, state, params, stage) }
    }

    unsafe fn load_inner(
        module: &Arc<Module>,
        cid: &[u8; 16],
        rate: f64,
        state: Option<&[u8]>,
        params: &[(u32, f64)],
        stage: Stage,
    ) -> Result<(Plugin, Audio, Loaded), String> {
        let name = module
            .audio_classes()
            .into_iter()
            .find(|c| &c.cid == cid)
            .map(|c| c.name)
            .unwrap_or_default();
        stage("creating it");
        let component: ComPtr<IComponent> =
            create(&module.factory, cid).ok_or("It could not be created.")?;
        let host = com::host_app().ok_or("No host object.")?;
        stage("starting it");
        if component.initialize(host.as_ptr().cast()) != kResultOk {
            return Err("It refused to start.".into());
        }
        let processor = component
            .cast::<IAudioProcessor>()
            .ok_or("It has no audio processor.")?;
        stage("starting its controls");
        let mut separate = false;
        let controller = match component.cast::<IEditController>() {
            Some(c) => Some(c),
            None => {
                let mut ccid = [0 as std::ffi::c_char; 16];
                if component.getControllerClassId(&mut ccid) == kResultOk {
                    let ccid = ccid.map(|b| b as u8);
                    match create::<IEditController>(&module.factory, &ccid) {
                        Some(c) if c.initialize(host.as_ptr().cast()) == kResultOk => {
                            separate = true;
                            Some(c)
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
        };
        let mut points = None;
        if separate {
            if let (Some(a), Some(b)) = (
                component.cast::<IConnectionPoint>(),
                controller
                    .as_ref()
                    .and_then(|c| c.cast::<IConnectionPoint>()),
            ) {
                a.connect(b.as_ptr());
                b.connect(a.as_ptr());
                points = Some((a, b));
            }
        }
        stage("reading its settings");
        if let Some(blob) = state {
            let (comp, ctrl) = split_state(blob).unwrap_or((blob, &[]));
            let s = MemStream::with(comp);
            component.setState(stream_ptr(&s));
            if let (Some(c), true) = (&controller, separate) {
                let s = MemStream::with(comp);
                c.setComponentState(stream_ptr(&s));
                if !ctrl.is_empty() {
                    let s = MemStream::with(ctrl);
                    c.setState(stream_ptr(&s));
                }
            }
        } else if let (Some(c), true) = (&controller, separate) {
            let s = MemStream::empty();
            if component.getState(stream_ptr(&s)) == kResultOk {
                let bytes = s.data.borrow().clone();
                let s = MemStream::with(&bytes);
                c.setComponentState(stream_ptr(&s));
            }
        }
        let edits: Edits = Arc::new(Mutex::new(Vec::new()));
        if let Some(c) = &controller {
            if let Some(h) = com::handler(&edits) {
                c.setComponentHandler(h.as_ptr());
            }
        }
        stage("setting up its audio");
        let n_in = component.getBusCount(K_AUDIO, K_IN).max(0);
        let n_out = component.getBusCount(K_AUDIO, K_OUT).max(0);
        let n_ev = component.getBusCount(K_EVENT, K_IN).max(0);
        if n_ev > 0 {
            component.activateBus(K_EVENT, K_IN, 0, 1);
        }
        let mut in_arr = Vec::new();
        for i in 0..n_in {
            let mut a: SpeakerArrangement = 0;
            processor.getBusArrangement(K_IN, i, &mut a);
            in_arr.push(if i == 0 { STEREO } else { a });
        }
        let mut out_arr = Vec::new();
        for i in 0..n_out {
            let mut a: SpeakerArrangement = 0;
            processor.getBusArrangement(K_OUT, i, &mut a);
            out_arr.push(if i == 0 { STEREO } else { a });
        }
        if processor.setBusArrangements(in_arr.as_mut_ptr(), n_in, out_arr.as_mut_ptr(), n_out)
            != kResultTrue
        {
            // It keeps a layout of its own (often mono); routed below.
        }
        let mut ins = Vec::new();
        for i in 0..n_in {
            let mut a: SpeakerArrangement = 0;
            processor.getBusArrangement(K_IN, i, &mut a);
            let mut info: BusInfo = std::mem::zeroed();
            component.getBusInfo(K_AUDIO, K_IN, i, &mut info);
            let n = if a != 0 {
                speaker_count(a)
            } else {
                info.channelCount.max(0) as usize
            };
            ins.push(n);
            // Only the main input; a key input stays off until a key comes.
            component.activateBus(K_AUDIO, K_IN, i, (i == 0) as u8);
        }
        let mut outs = Vec::new();
        for i in 0..n_out {
            let mut a: SpeakerArrangement = 0;
            processor.getBusArrangement(K_OUT, i, &mut a);
            let mut info: BusInfo = std::mem::zeroed();
            component.getBusInfo(K_AUDIO, K_OUT, i, &mut info);
            let n = if a != 0 {
                speaker_count(a)
            } else {
                info.channelCount.max(0) as usize
            };
            outs.push(n);
            component.activateBus(K_AUDIO, K_OUT, i, 1);
        }
        let _ = MONO;
        if processor.canProcessSampleSize(0) != kResultOk {
            return Err("It cannot work in 32-bit floating point.".into());
        }
        let mut setup = ProcessSetup {
            processMode: 0,
            symbolicSampleSize: 0,
            maxSamplesPerBlock: MAX_BLOCK as i32,
            sampleRate: rate,
        };
        if processor.setupProcessing(&mut setup) != kResultOk {
            // Some return kNotImplemented and work anyway.
        }
        stage("switching it on");
        if component.setActive(1) != kResultOk {
            return Err("It refused to switch on.".into());
        }
        processor.setProcessing(1);
        let latency = processor.getLatencySamples();
        let sidechain = n_in > 1 && ins.get(1).copied().unwrap_or(0) > 0;
        let mut keyed_latency = None;
        if sidechain {
            stage("measuring its delay with the sidechain on");
            processor.setProcessing(0);
            component.setActive(0);
            component.activateBus(K_AUDIO, K_IN, 1, 1);
            component.setActive(1);
            keyed_latency = Some(processor.getLatencySamples());
            component.setActive(0);
            component.activateBus(K_AUDIO, K_IN, 1, 0);
            component.setActive(1);
            processor.setProcessing(1);
        }
        let tail = processor.getTailSamples();
        stage("reading its controls");
        let mut infos = Vec::new();
        if let Some(c) = &controller {
            for i in 0..c.getParameterCount().max(0) {
                let mut p: ParameterInfo = std::mem::zeroed();
                if c.getParameterInfo(i, &mut p) != kResultOk {
                    continue;
                }
                use vst3::Steinberg::Vst::ParameterInfo_::ParameterFlags_::*;
                let hidden_mask = (kIsReadOnly | kIsHidden | kIsProgramChange) as i32;
                infos.push(ParamInfo {
                    id: p.id,
                    title: wstr(&p.title),
                    units: wstr(&p.units),
                    steps: p.stepCount,
                    default: p.defaultNormalizedValue,
                    hidden: p.flags & hidden_mask != 0,
                });
            }
        }
        let instrument = n_ev > 0 && n_in == 0;
        let changes = ParamChanges::new();
        let changes_ptr = com::changes_ptr(&changes).ok_or("No parameter list.")?;
        let events = EventList::new();
        let events_ptr = com::events_ptr(&events).ok_or("No event list.")?;
        // MIDI controllers (sustain, mod wheel, pitch bend...) reach a
        // VST3 plug-in as controls, by its MIDI mapping.
        let mut midi_map = vec![UNMAPPED; 16 * CONTROLLERS];
        if n_ev > 0 {
            stage("reading its MIDI mapping");
            if let Some(m) = controller
                .as_ref()
                .and_then(|c| c.cast::<vst3::Steinberg::Vst::IMidiMapping>())
            {
                use vst3::Steinberg::Vst::IMidiMappingTrait;
                for ch in 0..16i16 {
                    for cc in 0..CONTROLLERS as i16 {
                        let mut id: u32 = 0;
                        let r = m.getMidiControllerAssignment(0, ch, cc, &mut id);
                        if r == kResultOk || r == kResultTrue {
                            midi_map[ch as usize * CONTROLLERS + cc as usize] = id;
                        }
                    }
                }
            }
        }
        let controllers: Vec<u8> = (0..CONTROLLERS)
            .filter(|&cc| midi_map[cc] != UNMAPPED)
            .map(|cc| cc as u8)
            .collect();
        let to_audio = Arc::new(Mutex::new(Vec::new()));
        let mut audio = Audio {
            processor: processor.clone(),
            component: component.clone(),
            in_data: ins.iter().map(|&n| vec![vec![0.0; MAX_BLOCK]; n]).collect(),
            out_data: outs
                .iter()
                .map(|&n| vec![vec![0.0; MAX_BLOCK]; n])
                .collect(),
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            in_buses: Vec::new(),
            out_buses: Vec::new(),
            changes,
            changes_ptr,
            events,
            events_ptr,
            takes_events: n_ev > 0,
            midi_map,
            ctx: Box::new(std::mem::zeroed()),
            rate,
            time: 0,
            sidechain,
            key_on: false,
            last_key: None,
            instrument,
            from_editor: to_audio.clone(),
            pending: Vec::new(),
        };
        audio.wire();
        let plugin = Plugin {
            window: None,
            controller,
            separate,
            points,
            component,
            processor,
            edits,
            to_audio,
            name,
            _host: host,
            _module: module.clone(),
        };
        if !params.is_empty() {
            if let Some(c) = &plugin.controller {
                for &(id, v) in params {
                    c.setParamNormalized(id, v);
                }
            }
            // A block of no samples carrying only the changes, so the
            // values count in the state at once.
            audio.flush(params);
        }
        let values = plugin.values(&infos);
        let has_editor = plugin.controller.is_some();
        Ok((
            plugin,
            audio,
            Loaded {
                latency,
                keyed_latency,
                tail,
                has_editor,
                params: infos,
                sidechain,
                instrument,
                values,
                events: n_ev > 0,
                controllers,
            },
        ))
    }

    /// The controller's current value of every control.
    pub fn values(&self, infos: &[ParamInfo]) -> Vec<(u32, f64)> {
        let Some(c) = &self.controller else {
            return Vec::new();
        };
        infos
            .iter()
            .map(|p| (p.id, unsafe { c.getParamNormalized(p.id) }))
            .collect()
    }

    pub fn latency(&self) -> u32 {
        unsafe { self.processor.getLatencySamples() }
    }

    /// The settings blob (see [`join_state`]).
    pub fn state(&self) -> Result<Vec<u8>, String> {
        unsafe {
            let s = MemStream::empty();
            if self.component.getState(stream_ptr(&s)) != kResultOk {
                return Err("It would not give its settings.".into());
            }
            let comp = s.data.borrow().clone();
            let mut ctrl = Vec::new();
            if let (Some(c), true) = (&self.controller, self.separate) {
                let s = MemStream::empty();
                if c.getState(stream_ptr(&s)) == kResultOk {
                    ctrl = s.data.borrow().clone();
                }
            }
            Ok(join_state(&comp, &ctrl))
        }
    }

    pub fn set_state(&self, blob: &[u8]) -> Result<(), String> {
        let (comp, ctrl) = split_state(blob).ok_or("The settings are damaged.")?;
        unsafe {
            let s = MemStream::with(comp);
            if self.component.setState(stream_ptr(&s)) != kResultOk {
                return Err("It refused the settings.".into());
            }
            if let (Some(c), true) = (&self.controller, self.separate) {
                let s = MemStream::with(comp);
                c.setComponentState(stream_ptr(&s));
                if !ctrl.is_empty() {
                    let s = MemStream::with(ctrl);
                    c.setState(stream_ptr(&s));
                }
            }
        }
        Ok(())
    }

    /// Shows new values in the plug-in's own controls.
    pub fn set_params(&self, changes: &[(u32, f64)]) {
        if let Some(c) = &self.controller {
            for &(id, v) in changes {
                unsafe { c.setParamNormalized(id, v) };
            }
        }
    }

    /// Takes what the plug-in did since last time.
    pub fn take_edits(&self) -> Vec<Edit> {
        self.edits
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.window = None;
        unsafe {
            if let Some((a, b)) = self.points.take() {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = self.controller.take() {
                c.setComponentHandler(std::ptr::null_mut());
                if self.separate {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

impl Audio {
    /// Points the bus structures at the buffers (once; they never move).
    fn wire(&mut self) {
        self.in_ptrs = self
            .in_data
            .iter_mut()
            .map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        self.out_ptrs = self
            .out_data
            .iter_mut()
            .map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        self.in_buses = self
            .in_ptrs
            .iter_mut()
            .map(|p| AudioBusBuffers {
                numChannels: p.len() as i32,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: p.as_mut_ptr(),
                },
            })
            .collect();
        self.out_buses = self
            .out_ptrs
            .iter_mut()
            .map(|p| AudioBusBuffers {
                numChannels: p.len() as i32,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: p.as_mut_ptr(),
                },
            })
            .collect();
    }

    /// Switches the key input on or off (the plug-in has to be off to
    /// change its buses).
    fn key_input(&mut self, on: bool) {
        if self.key_on == on || !self.sidechain {
            return;
        }
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            self.component.activateBus(K_AUDIO, K_IN, 1, on as u8);
            self.component.setActive(1);
            self.processor.setProcessing(1);
        }
        self.key_on = on;
    }

    /// Clears tails: effects pause and resume (switching them off and on
    /// restarts some, like aligners); instruments also switch off and on
    /// so held notes stop.
    pub fn reset(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            if self.instrument {
                self.component.setActive(0);
                self.component.setActive(1);
            }
            self.processor.setProcessing(1);
        }
    }

    /// Hands over control changes in a block of no samples.
    pub fn flush(&mut self, changes: &[(u32, f64)]) {
        self.changes.clear();
        for &(id, v) in changes {
            self.changes.add(id, 0, v);
        }
        let mut data = self.data(0);
        unsafe { self.processor.process(&mut data) };
        self.changes.clear();
    }

    /// Queues changes for the next block.
    pub fn queue(&mut self, changes: &[(u32, f64)]) {
        self.pending.extend_from_slice(changes);
    }

    fn data(&mut self, frames: usize) -> ProcessData {
        let changes = if self.changes.is_empty() {
            std::ptr::null_mut()
        } else {
            self.changes_ptr.as_ptr()
        };
        ProcessData {
            processMode: 0,
            symbolicSampleSize: 0,
            numSamples: frames as i32,
            numInputs: self.in_buses.len() as i32,
            numOutputs: self.out_buses.len() as i32,
            inputs: if self.in_buses.is_empty() {
                std::ptr::null_mut()
            } else {
                self.in_buses.as_mut_ptr()
            },
            outputs: if self.out_buses.is_empty() {
                std::ptr::null_mut()
            } else {
                self.out_buses.as_mut_ptr()
            },
            inputParameterChanges: changes,
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: if self.takes_events {
                self.events_ptr.as_ptr()
            } else {
                std::ptr::null_mut()
            },
            outputEvents: std::ptr::null_mut(),
            processContext: &mut *self.ctx,
        }
    }

    /// Fills the event list (and controller moves as control changes) for
    /// the frames `at..at + n` of a block.
    fn take_events(&mut self, events: &[Timed], at: usize, n: usize) {
        use vst3::Steinberg::Vst::Event_::EventTypes_::{kNoteOffEvent, kNoteOnEvent};
        use vst3::Steinberg::Vst::{Event, Event__type0, NoteOffEvent, NoteOnEvent};
        self.events.clear();
        for e in events {
            let off = e.offset as usize;
            if off < at || off >= at + n {
                continue;
            }
            let rel = (off - at) as i32;
            let note = |r#type: i32, body: Event__type0| Event {
                busIndex: 0,
                sampleOffset: rel,
                ppqPosition: 0.0,
                flags: 0,
                r#type: r#type as u16,
                __field0: body,
            };
            match e.event {
                NoteEvent::On {
                    pitch,
                    velocity,
                    channel,
                    tuning,
                } => self.events.push(note(
                    kNoteOnEvent,
                    Event__type0 {
                        noteOn: NoteOnEvent {
                            channel: channel as i16 & 15,
                            pitch: pitch as i16 & 0x7f,
                            tuning,
                            // Velocity 0 would read as a note off.
                            velocity: velocity.clamp(1.0 / 127.0, 1.0),
                            length: 0,
                            noteId: -1,
                        },
                    },
                )),
                NoteEvent::Off { pitch, channel } => self.events.push(note(
                    kNoteOffEvent,
                    Event__type0 {
                        noteOff: NoteOffEvent {
                            channel: channel as i16 & 15,
                            pitch: pitch as i16 & 0x7f,
                            velocity: 0.0,
                            noteId: -1,
                            tuning: 0.0,
                        },
                    },
                )),
                NoteEvent::Control {
                    controller,
                    value,
                    channel,
                } => {
                    let k = (channel as usize & 15) * CONTROLLERS
                        + (controller as usize).min(CONTROLLERS - 1);
                    let id = self.midi_map[k];
                    if id != UNMAPPED {
                        self.changes.add(id, rel, value.clamp(0.0, 1.0) as f64);
                    }
                }
            }
        }
    }

    /// Runs `input` (interleaved stereo) through the plug-in into `out`
    /// (the same length), with note and controller `events` placed in it.
    /// `key` is the sidechain, when one feeds it. Returns false when the
    /// plug-in did not run (the input passes).
    pub fn process(
        &mut self,
        input: &[f32],
        key: Option<&[f32]>,
        changes: &[(u32, f64)],
        events: &[Timed],
        out: &mut [f32],
    ) -> bool {
        let frames = input.len() / 2;
        out[..frames * 2].copy_from_slice(&input[..frames * 2]);
        // The key input follows the key.
        let now = Instant::now();
        if key.is_some() {
            self.last_key = Some(now);
            self.key_input(true);
        } else if self.key_on && self.last_key.is_none_or(|t| now - t > KEY_HOLD) {
            self.key_input(false);
        }
        if let Ok(mut v) = self.from_editor.try_lock() {
            self.pending.append(&mut v);
        }
        self.pending.extend_from_slice(changes);
        let mut ok = true;
        let mut at = 0;
        while at < frames {
            let n = (frames - at).min(MAX_BLOCK);
            self.changes.clear();
            if at == 0 {
                for &(id, v) in &self.pending {
                    self.changes.add(id, 0, v);
                }
                self.pending.clear();
            }
            self.take_events(events, at, n);
            ok &= self.block(
                &input[at * 2..(at + n) * 2],
                key.map(|k| &k[at * 2..(at + n) * 2]),
                &mut out[at * 2..(at + n) * 2],
            );
            at += n;
        }
        self.changes.clear();
        self.events.clear();
        ok
    }

    fn block(&mut self, input: &[f32], key: Option<&[f32]>, out: &mut [f32]) -> bool {
        let n = input.len() / 2;
        // Main input: stereo as is, mono as the average.
        if let Some(bus) = self.in_data.first_mut() {
            match bus.len() {
                0 => {}
                1 => {
                    for i in 0..n {
                        bus[0][i] = (input[i * 2] + input[i * 2 + 1]) * 0.5;
                    }
                }
                _ => {
                    for i in 0..n {
                        bus[0][i] = input[i * 2];
                        bus[1][i] = input[i * 2 + 1];
                    }
                    for c in &mut bus[2..] {
                        c[..n].fill(0.0);
                    }
                }
            }
        }
        for (b, bus) in self.in_data.iter_mut().enumerate().skip(1) {
            match (b, key, bus.len()) {
                (1, Some(k), 1) if self.key_on => {
                    for i in 0..n {
                        bus[0][i] = (k[i * 2] + k[i * 2 + 1]) * 0.5;
                    }
                }
                (1, Some(k), c) if self.key_on && c >= 2 => {
                    for i in 0..n {
                        bus[0][i] = k[i * 2];
                        bus[1][i] = k[i * 2 + 1];
                    }
                    for c in &mut bus[2..] {
                        c[..n].fill(0.0);
                    }
                }
                _ => {
                    for c in bus.iter_mut() {
                        c[..n].fill(0.0);
                    }
                }
            }
        }
        for bus in &mut self.out_data {
            for c in bus.iter_mut() {
                c[..n].fill(0.0);
            }
        }
        for b in &mut self.in_buses {
            b.silenceFlags = 0;
        }
        for b in &mut self.out_buses {
            b.silenceFlags = 0;
        }
        self.fill_context();
        let mut data = self.data(n);
        let r = unsafe { self.processor.process(&mut data) };
        self.time += n as i64;
        let Some(bus) = self.out_data.first() else {
            return false;
        };
        let wrote = bus.iter().any(|c| c[..n].iter().any(|v| *v != 0.0));
        // Some plug-ins report an error yet fill their outputs: keep them.
        if r != kResultOk && !wrote {
            return false;
        }
        match bus.len() {
            0 => return false,
            1 => {
                for i in 0..n {
                    out[i * 2] = bus[0][i];
                    out[i * 2 + 1] = bus[0][i];
                }
            }
            _ => {
                for i in 0..n {
                    out[i * 2] = bus[0][i];
                    out[i * 2 + 1] = bus[1][i];
                }
            }
        }
        true
    }

    fn fill_context(&mut self) {
        use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::*;
        let c = &mut *self.ctx;
        c.state = (kPlaying | kTempoValid | kTimeSigValid | kProjectTimeMusicValid | kContTimeValid)
            as u32;
        c.sampleRate = self.rate;
        c.projectTimeSamples = self.time;
        c.continousTimeSamples = self.time;
        c.tempo = 120.0;
        c.projectTimeMusic = self.time as f64 / self.rate * 2.0;
        c.timeSigNumerator = 4;
        c.timeSigDenominator = 4;
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
        }
    }
}

/// Moves the audio half to the audio thread.
pub type SendAudio = Sendable<std::cell::RefCell<Option<Audio>>>;
