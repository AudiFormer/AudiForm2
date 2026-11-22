// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The COM objects a host hands to plug-ins: the host application, memory
//! streams for settings, parameter change lists, the component handler
//! (edits from the plug-in's window) and messages between a plug-in's two
//! halves.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::sync::{Arc, Mutex};

use vst3::Steinberg::Vst::{
    IAttributeList, IAttributeListTrait, IComponentHandler, IComponentHandlerTrait,
    IHostApplication, IHostApplicationTrait, IMessage, IMessageTrait, IParamValueQueue,
    IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait, IPlugInterfaceSupport,
    IPlugInterfaceSupportTrait, ParamID, ParamValue, String128, TChar,
};
use vst3::Steinberg::{
    int32, int64, kInvalidArgument, kNoInterface, kResultFalse, kResultOk, tresult, uint32,
    FIDString, IBStream, IBStreamTrait, TUID,
};
use vst3::{Class, ComPtr, ComWrapper, Interface};

/// Something a plug-in did that the host's main loop passes on.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Param(u32, f64),
    Begin(u32),
    End(u32),
    Latency,
    Dirty,
    /// The plug-in asked its window to change size.
    Resize(i32, i32),
    EditorClosed,
    Bypass(bool),
    /// Space in the plug-in's window, outside a text box (Shift: record).
    Space(bool),
}

pub type Edits = Arc<Mutex<Vec<Edit>>>;

fn guid_eq(a: &TUID, b: &vst3::com_scrape_types::Guid) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| *x as u8 == *y)
}

pub(crate) fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s;
        n += 1;
    }
    if n < dst.len() {
        dst[n] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

/// Hands out `obj` as the interface `iid` asks for.
unsafe fn hand_out<C: Class + 'static>(
    obj: ComWrapper<C>,
    iid: *const TUID,
    out: *mut *mut c_void,
) -> tresult {
    let Some(unk) = obj.to_com_ptr::<vst3::Steinberg::FUnknown>() else {
        return kNoInterface;
    };
    let p = unk.as_ptr();
    ((*(*p).vtbl).queryInterface)(p, iid, out)
}

// ---- The host application -----------------------------------------------

pub struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication, IPlugInterfaceSupport);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        copy_wstring("AudiForm 2", &mut *name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        let cid = &*cid;
        if guid_eq(cid, &IMessage::IID) {
            return hand_out(ComWrapper::new(Message::default()), iid, obj);
        }
        if guid_eq(cid, &IAttributeList::IID) {
            return hand_out(ComWrapper::new(Attributes::default()), iid, obj);
        }
        *obj = std::ptr::null_mut();
        kResultFalse
    }
}

impl IPlugInterfaceSupportTrait for HostApp {
    unsafe fn isPlugInterfaceSupported(&self, iid: *const TUID) -> tresult {
        use vst3::Steinberg::Vst::*;
        let iid = &*iid;
        let ours = [
            IComponentHandler::IID,
            IAudioProcessor::IID,
            IComponent::IID,
            IEditController::IID,
            IConnectionPoint::IID,
            IHostApplication::IID,
            IMessage::IID,
            IAttributeList::IID,
            IParameterChanges::IID,
            IParamValueQueue::IID,
            IEventList::IID,
            IMidiMapping::IID,
            vst3::Steinberg::IPlugFrame::IID,
            vst3::Steinberg::IPlugView::IID,
            vst3::Steinberg::IBStream::IID,
        ];
        if ours.iter().any(|g| guid_eq(iid, g)) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

// ---- Messages between a plug-in's halves --------------------------------

#[derive(Default)]
pub struct Message {
    id: RefCell<Vec<u8>>,
    attrs: RefCell<Option<ComPtr<IAttributeList>>>,
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        let id = self.id.borrow();
        if id.is_empty() {
            c"".as_ptr()
        } else {
            id.as_ptr().cast()
        }
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        let mut v = if id.is_null() {
            Vec::new()
        } else {
            CStr::from_ptr(id).to_bytes().to_vec()
        };
        v.push(0);
        *self.id.borrow_mut() = v;
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        let mut a = self.attrs.borrow_mut();
        if a.is_none() {
            *a = ComWrapper::new(Attributes::default()).to_com_ptr::<IAttributeList>();
        }
        // The message keeps its reference; the caller does not release.
        a.as_ref().map_or(std::ptr::null_mut(), |p| p.as_ptr())
    }
}

#[derive(Clone)]
enum Value {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

#[derive(Default)]
pub struct Attributes {
    map: RefCell<HashMap<Vec<u8>, Value>>,
}

impl Class for Attributes {
    type Interfaces = (IAttributeList,);
}

unsafe fn key(id: *const std::ffi::c_char) -> Option<Vec<u8>> {
    (!id.is_null()).then(|| CStr::from_ptr(id).to_bytes().to_vec())
}

impl IAttributeListTrait for Attributes {
    unsafe fn setInt(&self, id: *const std::ffi::c_char, value: int64) -> tresult {
        let Some(k) = key(id) else {
            return kInvalidArgument;
        };
        self.map.borrow_mut().insert(k, Value::Int(value));
        kResultOk
    }

    unsafe fn getInt(&self, id: *const std::ffi::c_char, value: *mut int64) -> tresult {
        match key(id).and_then(|k| self.map.borrow().get(&k).cloned()) {
            Some(Value::Int(v)) => {
                *value = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: *const std::ffi::c_char, value: f64) -> tresult {
        let Some(k) = key(id) else {
            return kInvalidArgument;
        };
        self.map.borrow_mut().insert(k, Value::Float(value));
        kResultOk
    }

    unsafe fn getFloat(&self, id: *const std::ffi::c_char, value: *mut f64) -> tresult {
        match key(id).and_then(|k| self.map.borrow().get(&k).cloned()) {
            Some(Value::Float(v)) => {
                *value = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: *const std::ffi::c_char, string: *const TChar) -> tresult {
        let Some(k) = key(id) else {
            return kInvalidArgument;
        };
        let mut v = Vec::new();
        if !string.is_null() {
            let mut i = 0;
            while *string.add(i) != 0 {
                v.push(*string.add(i));
                i += 1;
            }
        }
        self.map.borrow_mut().insert(k, Value::Str(v));
        kResultOk
    }

    unsafe fn getString(
        &self,
        id: *const std::ffi::c_char,
        string: *mut TChar,
        size_in_bytes: uint32,
    ) -> tresult {
        match key(id).and_then(|k| self.map.borrow().get(&k).cloned()) {
            Some(Value::Str(v)) => {
                let cap = size_in_bytes as usize / 2;
                if cap == 0 {
                    return kResultFalse;
                }
                let n = v.len().min(cap - 1);
                std::ptr::copy_nonoverlapping(v.as_ptr(), string, n);
                *string.add(n) = 0;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setBinary(
        &self,
        id: *const std::ffi::c_char,
        data: *const c_void,
        size_in_bytes: uint32,
    ) -> tresult {
        let Some(k) = key(id) else {
            return kInvalidArgument;
        };
        let v = if data.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize).to_vec()
        };
        self.map.borrow_mut().insert(k, Value::Bin(v));
        kResultOk
    }

    unsafe fn getBinary(
        &self,
        id: *const std::ffi::c_char,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        let map = self.map.borrow();
        match key(id).and_then(|k| map.get(&k)) {
            Some(Value::Bin(v)) => {
                // Valid while the list lives and the entry is unchanged.
                *data = v.as_ptr().cast();
                *size_in_bytes = v.len() as u32;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

// ---- Memory streams ------------------------------------------------------

#[derive(Default)]
pub struct MemStream {
    pub data: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl MemStream {
    pub fn with(data: &[u8]) -> ComWrapper<MemStream> {
        ComWrapper::new(MemStream {
            data: RefCell::new(data.to_vec()),
            pos: Cell::new(0),
        })
    }

    pub fn empty() -> ComWrapper<MemStream> {
        ComWrapper::new(MemStream::default())
    }
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, read: *mut int32) -> tresult {
        let data = self.data.borrow();
        let pos = self.pos.get().min(data.len());
        let n = (num_bytes.max(0) as usize).min(data.len() - pos);
        if n > 0 && !buffer.is_null() {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(pos), buffer as *mut u8, n);
        }
        self.pos.set(pos + n);
        if !read.is_null() {
            *read = n as int32;
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, written: *mut int32) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let n = num_bytes as usize;
        let mut data = self.data.borrow_mut();
        let pos = self.pos.get();
        if data.len() < pos + n {
            data.resize(pos + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(pos), n);
        self.pos.set(pos + n);
        if !written.is_null() {
            *written = num_bytes;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let len = self.data.borrow().len() as i64;
        let base = match mode as u32 {
            0 => 0,
            1 => self.pos.get() as i64,
            2 => len,
            _ => return kInvalidArgument,
        };
        let p = base + pos;
        if p < 0 {
            return kResultFalse;
        }
        self.pos.set(p as usize);
        if !result.is_null() {
            *result = p;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        *pos = self.pos.get() as i64;
        kResultOk
    }
}

// ---- Parameter changes ----------------------------------------------------

/// One parameter's points within a block.
pub struct ParamQueue {
    id: Cell<u32>,
    points: RefCell<Vec<(i32, f64)>>,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.points.borrow().len() as int32
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        match self.points.borrow().get(index.max(0) as usize) {
            Some(&(o, v)) if index >= 0 => {
                *offset = o;
                *value = v;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        let mut p = self.points.borrow_mut();
        if p.len() == p.capacity() {
            return kResultFalse;
        }
        p.push((offset, value));
        if !index.is_null() {
            *index = p.len() as int32 - 1;
        }
        kResultOk
    }
}

/// A block's parameter changes. The queues are made once and reused, so a
/// block allocates nothing unless it changes more controls than ever
/// before.
pub struct ParamChanges {
    queues: RefCell<Vec<(ComWrapper<ParamQueue>, ComPtr<IParamValueQueue>)>>,
    used: Cell<usize>,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

const POINTS: usize = 16;

impl ParamChanges {
    pub fn new() -> ComWrapper<ParamChanges> {
        ComWrapper::new(ParamChanges {
            queues: RefCell::new(Vec::new()),
            used: Cell::new(0),
        })
    }

    pub fn clear(&self) {
        for (q, _) in self.queues.borrow().iter().take(self.used.get()) {
            q.points.borrow_mut().clear();
        }
        self.used.set(0);
    }

    pub fn is_empty(&self) -> bool {
        self.used.get() == 0
    }

    fn queue_for(&self, id: u32) -> Option<usize> {
        let qs = self.queues.borrow();
        if let Some(i) = qs[..self.used.get()]
            .iter()
            .position(|(q, _)| q.id.get() == id)
        {
            return Some(i);
        }
        drop(qs);
        let i = self.used.get();
        let mut qs = self.queues.borrow_mut();
        if i == qs.len() {
            let q = ComWrapper::new(ParamQueue {
                id: Cell::new(id),
                points: RefCell::new(Vec::with_capacity(POINTS)),
            });
            let p = q.to_com_ptr::<IParamValueQueue>()?;
            qs.push((q, p));
        }
        qs[i].0.id.set(id);
        qs[i].0.points.borrow_mut().clear();
        self.used.set(i + 1);
        Some(i)
    }

    /// Adds a point (a later point at the same offset replaces it).
    pub fn add(&self, id: u32, offset: i32, value: f64) {
        let Some(i) = self.queue_for(id) else {
            return;
        };
        let qs = self.queues.borrow();
        let mut p = qs[i].0.points.borrow_mut();
        match p.last_mut() {
            Some(last) if last.0 >= offset => *last = (last.0.min(offset), value),
            _ => p.push((offset, value)),
        }
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.used.get() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        let qs = self.queues.borrow();
        if index < 0 || index as usize >= self.used.get() {
            return std::ptr::null_mut();
        }
        qs[index as usize].1.as_ptr()
    }

    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if id.is_null() {
            return std::ptr::null_mut();
        }
        let Some(i) = self.queue_for(*id) else {
            return std::ptr::null_mut();
        };
        if !index.is_null() {
            *index = i as int32;
        }
        self.queues.borrow()[i].1.as_ptr()
    }
}

// ---- Note events ----------------------------------------------------------

/// A block's note events (made once, reused; it allocates only past its
/// first capacity).
pub struct EventList {
    events: RefCell<Vec<vst3::Steinberg::Vst::Event>>,
}

impl Class for EventList {
    type Interfaces = (vst3::Steinberg::Vst::IEventList,);
}

impl EventList {
    pub fn new() -> ComWrapper<EventList> {
        ComWrapper::new(EventList {
            events: RefCell::new(Vec::with_capacity(512)),
        })
    }

    pub fn clear(&self) {
        self.events.borrow_mut().clear();
    }

    pub fn is_empty(&self) -> bool {
        self.events.borrow().is_empty()
    }

    pub fn len(&self) -> usize {
        self.events.borrow().len()
    }

    pub fn push(&self, e: vst3::Steinberg::Vst::Event) {
        self.events.borrow_mut().push(e);
    }
}

impl vst3::Steinberg::Vst::IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.events.borrow().len() as int32
    }

    unsafe fn getEvent(&self, index: int32, e: *mut vst3::Steinberg::Vst::Event) -> tresult {
        match self.events.borrow().get(index.max(0) as usize) {
            Some(x) if index >= 0 && !e.is_null() => {
                *e = *x;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, e: *mut vst3::Steinberg::Vst::Event) -> tresult {
        if e.is_null() {
            return kInvalidArgument;
        }
        self.events.borrow_mut().push(*e);
        kResultOk
    }
}

pub fn events_ptr(c: &ComWrapper<EventList>) -> Option<ComPtr<vst3::Steinberg::Vst::IEventList>> {
    c.to_com_ptr::<vst3::Steinberg::Vst::IEventList>()
}

// ---- The component handler ------------------------------------------------

pub struct Handler {
    pub edits: Edits,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

fn push(e: &Edits, x: Edit) {
    if let Ok(mut v) = e.lock() {
        v.push(x);
    }
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, id: ParamID) -> tresult {
        push(&self.edits, Edit::Begin(id));
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        push(&self.edits, Edit::Param(id, value));
        kResultOk
    }

    unsafe fn endEdit(&self, id: ParamID) -> tresult {
        push(&self.edits, Edit::End(id));
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        use vst3::Steinberg::Vst::RestartFlags_::*;
        let f = flags as u32;
        if f & kLatencyChanged as u32 != 0 {
            push(&self.edits, Edit::Latency);
        }
        if f & (kParamValuesChanged as u32 | kReloadComponent as u32) != 0 {
            push(&self.edits, Edit::Dirty);
        }
        kResultOk
    }
}

// ---- A send-anywhere wrapper ----------------------------------------------

/// Moves a COM pointer to another thread. Plug-ins are written for a
/// host whose audio thread and main thread both touch the processor;
/// the host keeps each object's calls on the thread the SDK expects.
pub struct Sendable<T>(pub T);
unsafe impl<T> Send for Sendable<T> {}

impl<T> std::ops::Deref for Sendable<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

pub fn handler(edits: &Edits) -> Option<ComPtr<IComponentHandler>> {
    ComWrapper::new(Handler {
        edits: edits.clone(),
    })
    .to_com_ptr::<IComponentHandler>()
}

pub fn host_app() -> Option<ComPtr<IHostApplication>> {
    ComWrapper::new(HostApp).to_com_ptr::<IHostApplication>()
}

pub fn changes_ptr(c: &ComWrapper<ParamChanges>) -> Option<ComPtr<IParameterChanges>> {
    c.to_com_ptr::<IParameterChanges>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_changes_reuse_their_queues() {
        let c = ParamChanges::new();
        c.add(5, 0, 0.25);
        c.add(5, 0, 0.5);
        c.add(9, 10, 1.0);
        unsafe {
            assert_eq!(c.getParameterCount(), 2);
            let q = vst3::ComRef::from_raw(c.getParameterData(0)).unwrap();
            assert_eq!(q.getParameterId(), 5);
            assert_eq!(q.getPointCount(), 1, "same offset replaced");
            let (mut o, mut v) = (0, 0.0);
            q.getPoint(0, &mut o, &mut v);
            assert_eq!((o, v), (0, 0.5));
        }
        c.clear();
        assert!(c.is_empty());
        c.add(7, 0, 0.1);
        unsafe { assert_eq!(c.getParameterCount(), 1) };
    }

    #[test]
    fn memory_streams_read_write_and_seek() {
        let s = MemStream::empty();
        unsafe {
            let mut w = 0;
            let mut buf = *b"abcdef";
            s.write(buf.as_mut_ptr().cast(), 6, &mut w);
            assert_eq!(w, 6);
            let mut at = 0;
            s.seek(2, 0, &mut at);
            let mut out = [0u8; 10];
            let mut r = 0;
            s.read(out.as_mut_ptr().cast(), 10, &mut r);
            assert_eq!(&out[..r as usize], b"cdef");
        }
        assert_eq!(&s.data.borrow()[..], b"abcdef");
    }
}
