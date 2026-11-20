// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.
// The plug-in skeleton (factory, string helpers, bus answers) follows the
// `vst3` crate's examples/gain.rs, Copyright (c) Micah Johnston, MIT OR
// Apache-2.0.

//! A tiny VST3 plug-in for AudiForm 2's hosting tests. Never shipped.
//!
//! Two plug-ins share one controller:
//!
//! - "AF2 Test Gain": stereo gain (control 0, 0..1 = 0..2x, 0.5 = unity),
//!   32 samples late. Control 1 set to 1 crashes the process; control 2
//!   set to 1 makes one block take 6 s. `AF2_TESTPLUG_CRASH_ON_START=1`
//!   crashes it as it switches on.
//! - "AF2 Test Keyed": the same, plus a sidechain input. While the key
//!   input is on, its output is the key (64 samples late, the latency it
//!   reports then) instead of the gained input.

#![allow(
    non_snake_case,
    clippy::missing_safety_doc,
    clippy::unnecessary_cast,
    clippy::explicit_counter_loop
)]

use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{uid, Class, ComRef, ComWrapper};

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    for (d, s) in dst.iter_mut().zip(src.bytes().chain([0])) {
        *d = s as c_char;
    }
    if src.len() >= dst.len() {
        if let Some(last) = dst.last_mut() {
            *last = 0;
        }
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s as TChar;
        n += 1;
    }
    if n < dst.len() {
        dst[n] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

const GAIN_CID: TUID = uid(0x4146_3254, 0x4741_494E, 0x0000_0000, 0x0000_0001);
const KEYED_CID: TUID = uid(0x4146_3254, 0x4B455945, 0x0000_0000, 0x0000_0002);
const CTRL_CID: TUID = uid(0x4146_3254, 0x4354524C, 0x0000_0000, 0x0000_0003);

const LATENCY: u32 = 32;
const KEYED_LATENCY: u32 = 64;
const DELAY: usize = 128;

struct Delay {
    buf: [Vec<f32>; 2],
    pos: usize,
}

struct Proc {
    keyed: bool,
    gain: AtomicU64,
    key_on: AtomicBool,
    delay: Mutex<Delay>,
}

impl Class for Proc {
    type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
}

impl Proc {
    fn new(keyed: bool) -> Proc {
        Proc {
            keyed,
            gain: AtomicU64::new(0.5f64.to_bits()),
            key_on: AtomicBool::new(false),
            delay: Mutex::new(Delay {
                buf: [vec![0.0; DELAY], vec![0.0; DELAY]],
                pos: 0,
            }),
        }
    }

    fn latency(&self) -> u32 {
        if self.keyed && self.key_on.load(Ordering::Relaxed) {
            KEYED_LATENCY
        } else {
            LATENCY
        }
    }
}

impl IPluginBaseTrait for Proc {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Proc {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        *class_id = CTRL_CID;
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        match (media as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, BusDirections_::kInput) => 1 + self.keyed as i32,
            (MediaTypes_::kAudio, BusDirections_::kOutput) => 1,
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
        if media as MediaTypes != MediaTypes_::kAudio {
            return kInvalidArgument;
        }
        let bus = &mut *bus;
        bus.mediaType = media;
        bus.direction = dir;
        bus.channelCount = 2;
        match (dir as BusDirections, index) {
            (BusDirections_::kInput, 0) => {
                copy_wstring("Input", &mut bus.name);
                bus.busType = BusTypes_::kMain as BusType;
                bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
            }
            (BusDirections_::kInput, 1) if self.keyed => {
                copy_wstring("Sidechain", &mut bus.name);
                bus.busType = BusTypes_::kAux as BusType;
                bus.flags = 0;
            }
            (BusDirections_::kOutput, 0) => {
                copy_wstring("Output", &mut bus.name);
                bus.busType = BusTypes_::kMain as BusType;
                bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
            }
            _ => return kInvalidArgument,
        }
        kResultOk
    }

    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        state: TBool,
    ) -> tresult {
        if media as MediaTypes == MediaTypes_::kAudio
            && dir as BusDirections == BusDirections_::kInput
            && index == 1
        {
            self.key_on.store(state != 0, Ordering::Relaxed);
        }
        kResultOk
    }

    unsafe fn setActive(&self, state: TBool) -> tresult {
        if state != 0 && std::env::var_os("AF2_TESTPLUG_CRASH_ON_START").is_some() {
            std::process::abort();
        }
        if let Ok(mut d) = self.delay.lock() {
            for b in &mut d.buf {
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
            self.gain
                .store(f64::from_le_bytes(b).to_bits(), Ordering::Relaxed);
        }
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut b = f64::from_bits(self.gain.load(Ordering::Relaxed)).to_le_bytes();
        let mut n = 0;
        s.write(b.as_mut_ptr().cast(), 8, &mut n);
        kResultOk
    }
}

impl IAudioProcessorTrait for Proc {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        let ins = slice::from_raw_parts(inputs, num_ins.max(0) as usize);
        let outs = slice::from_raw_parts(outputs, num_outs.max(0) as usize);
        if ins.iter().chain(outs).all(|a| *a == SpeakerArr::kStereo) {
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
        self.latency()
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = &*data;
        if let Some(changes) = ComRef::from_raw(d.inputParameterChanges) {
            for i in 0..changes.getParameterCount() {
                let Some(q) = ComRef::from_raw(changes.getParameterData(i)) else {
                    continue;
                };
                let (mut off, mut v) = (0, 0.0);
                if q.getPoint(q.getPointCount() - 1, &mut off, &mut v) != kResultOk {
                    continue;
                }
                match q.getParameterId() {
                    0 => self.gain.store(v.to_bits(), Ordering::Relaxed),
                    1 if v >= 0.5 => std::process::abort(),
                    2 if v >= 0.5 => std::thread::sleep(std::time::Duration::from_secs(6)),
                    _ => {}
                }
            }
        }
        let n = d.numSamples.max(0) as usize;
        if n == 0 || d.numInputs < 1 || d.numOutputs < 1 {
            return kResultOk;
        }
        let ins = slice::from_raw_parts(d.inputs, d.numInputs as usize);
        let outs = slice::from_raw_parts(d.outputs, d.numOutputs as usize);
        let gain = f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32 * 2.0;
        let keyed = self.keyed && self.key_on.load(Ordering::Relaxed) && ins.len() > 1;
        let src = if keyed { &ins[1] } else { &ins[0] };
        let lat = self.latency() as usize;
        let src_ch = slice::from_raw_parts(src.__field0.channelBuffers32, src.numChannels as usize);
        let out_ch = slice::from_raw_parts(
            outs[0].__field0.channelBuffers32,
            outs[0].numChannels as usize,
        );
        let mut dl = self.delay.lock().unwrap();
        let start = dl.pos;
        for c in 0..2.min(out_ch.len()).min(src_ch.len()) {
            let input = slice::from_raw_parts(src_ch[c], n);
            let output = slice::from_raw_parts_mut(out_ch[c], n);
            let buf = &mut dl.buf[c];
            let mut pos = start;
            for i in 0..n {
                buf[pos % DELAY] = if keyed { input[i] } else { input[i] * gain };
                output[i] = buf[(pos + DELAY - lat) % DELAY];
                pos += 1;
            }
        }
        dl.pos = (start + n) % DELAY;
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IProcessContextRequirementsTrait for Proc {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        0
    }
}

struct Ctrl {
    gain: Cell<f64>,
}

impl Class for Ctrl {
    type Interfaces = (IEditController,);
}

impl IPluginBaseTrait for Ctrl {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

const PARAMS: [(&str, i32, f64); 3] = [("Gain", 0, 0.5), ("Crash", 1, 0.0), ("Stall", 1, 0.0)];

impl IEditControllerTrait for Ctrl {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut b = [0u8; 8];
        let mut n = 0;
        s.read(b.as_mut_ptr().cast(), 8, &mut n);
        if n == 8 {
            self.gain.set(f64::from_le_bytes(b));
        }
        kResultOk
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = ComRef::from_raw(state) else {
            return kInvalidArgument;
        };
        let mut b = *b"ctrl";
        let mut n = 0;
        s.write(b.as_mut_ptr().cast(), 4, &mut n);
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> i32 {
        PARAMS.len() as i32
    }

    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        let Some(&(name, steps, default)) = PARAMS.get(index.max(0) as usize) else {
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
        if id == 0 {
            self.gain.get()
        } else {
            0.0
        }
    }

    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        if id == 0 {
            self.gain.set(v);
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

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = &mut *info;
        copy_cstring("AudiForm Tests", &mut info.vendor);
        copy_cstring("", &mut info.url);
        copy_cstring("", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> i32 {
        3
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let info = &mut *info;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        let (cid, cat, name) = match index {
            0 => (GAIN_CID, "Audio Module Class", "AF2 Test Gain"),
            1 => (KEYED_CID, "Audio Module Class", "AF2 Test Keyed"),
            2 => (
                CTRL_CID,
                "Component Controller Class",
                "AF2 Test Controller",
            ),
            _ => return kInvalidArgument,
        };
        info.cid = cid;
        copy_cstring(cat, &mut info.category);
        copy_cstring(name, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let cid = *(cid as *const TUID);
        let unk = if cid == GAIN_CID {
            ComWrapper::new(Proc::new(false)).to_com_ptr::<FUnknown>()
        } else if cid == KEYED_CID {
            ComWrapper::new(Proc::new(true)).to_com_ptr::<FUnknown>()
        } else if cid == CTRL_CID {
            ComWrapper::new(Ctrl {
                gain: Cell::new(0.5),
            })
            .to_com_ptr::<FUnknown>()
        } else {
            None
        };
        match unk {
            Some(u) => {
                let p = u.as_ptr();
                ((*(*p).vtbl).queryInterface)(p, iid as *const TUID, obj)
            }
            None => kInvalidArgument,
        }
    }
}

#[cfg(windows)]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(windows)]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_h: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map_or(std::ptr::null_mut(), |p| p.into_raw())
}
