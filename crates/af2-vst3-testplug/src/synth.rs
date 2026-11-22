// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform2 contributors.

//! "AF2 Test Synth": an instrument for the hosting tests. Each held note
//! is a sine at its key (tuned by the note's tuning and the pitch bend,
//! two semitones either way), at velocity times twice the Level control
//! (0.5 = velocity as is), 16 samples late. Its MIDI mapping takes the
//! sustain pedal (control 2), pitch bend (3) and mod wheel (4); while the
//! pedal is down, released notes keep sounding. Control 1 set to 1, or a
//! note on key 0, crashes the process.

use super::*;

pub const SYNTH_CID: TUID = uid(0x4146_3254, 0x5359_4E54, 0x0000_0000, 0x0000_0004);
pub const SYNTH_CTRL_CID: TUID = uid(0x4146_3254, 0x5343_5452, 0x0000_0000, 0x0000_0005);

pub const SYNTH_LATENCY: usize = 16;

const P_LEVEL: u32 = 0;
const P_CRASH: u32 = 1;
const P_SUSTAIN: u32 = 2;
const P_BEND: u32 = 3;
const P_MOD: u32 = 4;

struct Voice {
    pitch: i16,
    channel: i16,
    velocity: f32,
    tuning: f32,
    phase: f64,
    /// Let go while the pedal was down: sounds until it comes up.
    held: bool,
}

struct State {
    voices: Vec<Voice>,
    level: f64,
    sustain: bool,
    bend: f64,
    rate: f64,
    delay: [Vec<f32>; 2],
    pos: usize,
}

pub struct SynthProc {
    st: Mutex<State>,
}

impl Class for SynthProc {
    type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
}

impl SynthProc {
    pub fn new() -> SynthProc {
        SynthProc {
            st: Mutex::new(State {
                voices: Vec::new(),
                level: 0.5,
                sustain: false,
                bend: 0.5,
                rate: 48_000.0,
                delay: [vec![0.0; 64], vec![0.0; 64]],
                pos: 0,
            }),
        }
    }
}

impl IPluginBaseTrait for SynthProc {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for SynthProc {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        *class_id = SYNTH_CTRL_CID;
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        match (media as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, BusDirections_::kOutput) => 1,
            (MediaTypes_::kEvent, BusDirections_::kInput) => 1,
            _ => 0,
        }
    }

    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let bus = &mut *bus;
        bus.mediaType = media;
        bus.direction = dir;
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        match (media as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, BusDirections_::kOutput) => {
                copy_wstring("Output", &mut bus.name);
                bus.channelCount = 2;
            }
            (MediaTypes_::kEvent, BusDirections_::kInput) => {
                copy_wstring("Notes", &mut bus.name);
                bus.channelCount = 16;
            }
            _ => return kInvalidArgument,
        }
        kResultOk
    }

    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, state: TBool) -> tresult {
        if state != 0 && std::env::var_os("AF2_TESTPLUG_CRASH_ON_START").is_some() {
            std::process::abort();
        }
        if let Ok(mut s) = self.st.lock() {
            s.voices.clear();
            s.sustain = false;
            for b in &mut s.delay {
                b.fill(0.0);
            }
        }
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut b = [0u8; 8];
        let mut n = 0;
        s.read(b.as_mut_ptr().cast(), 8, &mut n);
        if n == 8 {
            self.st.lock().unwrap().level = f64::from_le_bytes(b);
        }
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut b = self.st.lock().unwrap().level.to_le_bytes();
        let mut n = 0;
        s.write(b.as_mut_ptr().cast(), 8, &mut n);
        kResultOk
    }
}

enum Step {
    Param(u32, f64),
    Note(Event),
}

impl IAudioProcessorTrait for SynthProc {
    unsafe fn setBusArrangements(
        &self,
        _inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        let outs = slice::from_raw_parts(outputs, num_outs.max(0) as usize);
        if num_ins == 0 && outs.iter().all(|a| *a == SpeakerArr::kStereo) {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe fn getBusArrangement(
        &self,
        _dir: BusDirection,
        _index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        *arr = SpeakerArr::kStereo;
        kResultOk
    }

    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size as SymbolicSampleSizes == SymbolicSampleSizes_::kSample32 {
            kResultOk
        } else {
            kNotImplemented
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        SYNTH_LATENCY as u32
    }

    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        if let Some(s) = setup.as_ref() {
            self.st.lock().unwrap().rate = s.sampleRate;
        }
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = &*data;
        let n = d.numSamples.max(0) as usize;
        // Every control point and note event, at its frame.
        let mut steps: Vec<(i32, Step)> = Vec::new();
        if let Some(changes) = ComRef::from_raw(d.inputParameterChanges) {
            for i in 0..changes.getParameterCount() {
                let Some(q) = ComRef::from_raw(changes.getParameterData(i)) else {
                    continue;
                };
                let id = q.getParameterId();
                for k in 0..q.getPointCount() {
                    let (mut off, mut v) = (0, 0.0);
                    if q.getPoint(k, &mut off, &mut v) == kResultOk {
                        steps.push((off, Step::Param(id, v)));
                    }
                }
            }
        }
        if let Some(events) = ComRef::from_raw(d.inputEvents) {
            for i in 0..events.getEventCount() {
                let mut e: Event = std::mem::zeroed();
                if events.getEvent(i, &mut e) == kResultOk {
                    steps.push((e.sampleOffset, Step::Note(e)));
                }
            }
        }
        steps.sort_by_key(|s| s.0);
        let mut st = self.st.lock().unwrap();
        let st = &mut *st;
        let out_ch = if d.numOutputs >= 1 && n > 0 {
            let outs = slice::from_raw_parts(d.outputs, d.numOutputs as usize);
            slice::from_raw_parts(outs[0].__field0.channelBuffers32, outs[0].numChannels as usize)
        } else {
            &[]
        };
        let mut next = 0;
        for i in 0..n.max(1) {
            while next < steps.len() && (steps[next].0 as usize <= i || n == 0) {
                match &steps[next].1 {
                    Step::Param(id, v) => match *id {
                        P_LEVEL => st.level = *v,
                        P_CRASH if *v >= 0.5 => std::process::abort(),
                        P_SUSTAIN => {
                            st.sustain = *v >= 0.5;
                            if !st.sustain {
                                st.voices.retain(|x| !x.held);
                            }
                        }
                        P_BEND => st.bend = *v,
                        _ => {}
                    },
                    Step::Note(e) => {
                        if e.r#type == Event_::EventTypes_::kNoteOnEvent as u16 {
                            let on = e.__field0.noteOn;
                            if on.pitch == 0 {
                                std::process::abort();
                            }
                            st.voices.push(Voice {
                                pitch: on.pitch,
                                channel: on.channel,
                                velocity: on.velocity,
                                tuning: on.tuning,
                                phase: 0.0,
                                held: false,
                            });
                        } else if e.r#type == Event_::EventTypes_::kNoteOffEvent as u16 {
                            let off = e.__field0.noteOff;
                            let sustain = st.sustain;
                            for v in st
                                .voices
                                .iter_mut()
                                .filter(|v| v.pitch == off.pitch && v.channel == off.channel)
                            {
                                v.held = true;
                            }
                            if !sustain {
                                st.voices.retain(|v| !v.held);
                            }
                        }
                    }
                }
                next += 1;
            }
            if n == 0 {
                break;
            }
            let bend = (st.bend - 0.5) * 4.0;
            let gain = (st.level * 2.0) as f32;
            let mut s = 0.0f32;
            for v in &mut st.voices {
                let semis = v.pitch as f64 - 69.0 + v.tuning as f64 / 100.0 + bend;
                let f = 440.0 * 2f64.powf(semis / 12.0);
                s += (v.phase * std::f64::consts::TAU).sin() as f32 * v.velocity * gain;
                v.phase = (v.phase + f / st.rate).fract();
            }
            let len = st.delay[0].len();
            let w = st.pos % len;
            let rd = (st.pos + len - SYNTH_LATENCY) % len;
            for (c, out) in out_ch.iter().enumerate().take(2) {
                st.delay[c][w] = s;
                *out.add(i) = st.delay[c][rd];
            }
            st.pos += 1;
        }
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IProcessContextRequirementsTrait for SynthProc {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        0
    }
}

pub struct SynthCtrl {
    vals: Mutex<[f64; 5]>,
}

impl SynthCtrl {
    pub fn new() -> SynthCtrl {
        SynthCtrl {
            vals: Mutex::new([0.5, 0.0, 0.0, 0.5, 0.0]),
        }
    }
}

impl Class for SynthCtrl {
    type Interfaces = (IEditController, IMidiMapping);
}

impl IPluginBaseTrait for SynthCtrl {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

const SYNTH_PARAMS: [(&str, i32, f64); 5] = [
    ("Level", 0, 0.5),
    ("Crash", 1, 0.0),
    ("Sustain", 1, 0.0),
    ("Bend", 0, 0.5),
    ("Mod", 0, 0.0),
];

impl IEditControllerTrait for SynthCtrl {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut b = [0u8; 8];
        let mut n = 0;
        s.read(b.as_mut_ptr().cast(), 8, &mut n);
        if n == 8 {
            self.vals.lock().unwrap()[0] = f64::from_le_bytes(b);
        }
        kResultOk
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> i32 {
        SYNTH_PARAMS.len() as i32
    }

    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        let Some(&(name, steps, default)) = SYNTH_PARAMS.get(index.max(0) as usize) else {
            return kInvalidArgument;
        };
        let info = &mut *info;
        info.id = index as u32;
        copy_wstring(name, &mut info.title);
        copy_wstring(name, &mut info.shortTitle);
        copy_wstring("", &mut info.units);
        info.stepCount = steps;
        info.defaultNormalizedValue = default;
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as i32;
        kResultOk
    }

    unsafe fn getParamStringByValue(&self, _id: u32, v: f64, s: *mut String128) -> tresult {
        copy_wstring(&format!("{v:.3}"), &mut *s);
        kResultOk
    }

    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        kNotImplemented
    }

    unsafe fn normalizedParamToPlain(&self, _id: u32, v: f64) -> f64 {
        v
    }

    unsafe fn plainParamToNormalized(&self, _id: u32, v: f64) -> f64 {
        v
    }

    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        self.vals.lock().unwrap().get(id as usize).copied().unwrap_or(0.0)
    }

    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        if let Some(x) = self.vals.lock().unwrap().get_mut(id as usize) {
            *x = v;
        }
        kResultOk
    }

    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }

    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

impl IMidiMappingTrait for SynthCtrl {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: i32,
        _channel: i16,
        cc: CtrlNumber,
        id: *mut ParamID,
    ) -> tresult {
        if bus != 0 || id.is_null() {
            return kResultFalse;
        }
        let p = match cc as u32 {
            64 => P_SUSTAIN,
            129 => P_BEND,
            1 => P_MOD,
            _ => return kResultFalse,
        };
        *id = p;
        kResultTrue
    }
}
