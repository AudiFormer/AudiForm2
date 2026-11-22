// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The WASAPI side of system-audio capture. Every COM call is checked and
//! turned into a message; nothing here panics on an OS error.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use windows::core::{implement, Interface, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eConsole, eRender, ActivateAudioInterfaceAsync, AudioSessionStateActive,
    AudioSessionStateExpired, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, DEVICE_STATE_ACTIVE, PROCESS_LOOPBACK_MODE,
    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, OpenProcess,
    QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{
    build_app_list, convert, push_frame, sample_fmt, silence_to_insert, AudioApp, LoopbackCapture,
    OutputDevice, ProcInfo, RawSession, RecordSource, SampleFmt,
};

type WinResult<T> = windows::core::Result<T>;

/// The real Windows build number (not the manifest-dependent one).
pub fn os_build() -> Option<u32> {
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut v = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `v` is a valid, sized OSVERSIONINFOW.
    let st = unsafe { RtlGetVersion(&mut v) };
    st.is_ok().then_some(v.dwBuildNumber)
}

/// COM for the current thread, undone on drop if we did it.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: plain COM initialisation; a thread already in another
        // apartment reports an error we tolerate.
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balanced with the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

struct Event(HANDLE);

impl Event {
    fn new() -> WinResult<Event> {
        // SAFETY: an unnamed auto-reset event.
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map(Event)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Runs `f` on a fresh MTA thread (the UI thread is usually an STA).
fn on_com_thread<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    std::thread::Builder::new()
        .name("af2-audio-query".into())
        .spawn(move || {
            let _com = Com::init();
            f()
        })
        .map_err(|e| format!("Could not start a thread: {e}"))?
        .join()
        .map_err(|_| "The audio query failed unexpectedly.".to_string())?
}

fn pwstr_take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: a COM-allocated, NUL-terminated string we now own.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn enumerator() -> WinResult<IMMDeviceEnumerator> {
    // SAFETY: standard MMDevice enumerator creation.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn device_id(d: &IMMDevice) -> String {
    // SAFETY: GetId hands back a string we free.
    unsafe { d.GetId() }.map(pwstr_take).unwrap_or_default()
}

fn device_name(d: &IMMDevice) -> String {
    // SAFETY: read-only property store access.
    unsafe {
        d.OpenPropertyStore(STGM_READ)
            .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

fn render_devices(en: &IMMDeviceEnumerator) -> WinResult<Vec<IMMDevice>> {
    // SAFETY: COM calls on a live enumerator.
    unsafe {
        let col = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let n = col.GetCount()?;
        Ok((0..n).filter_map(|i| col.Item(i).ok()).collect())
    }
}

pub fn list_output_devices() -> Result<Vec<OutputDevice>, String> {
    on_com_thread(|| {
        let en = enumerator().map_err(|e| format!("Audio devices are unavailable: {e}"))?;
        // SAFETY: COM call on a live enumerator.
        let default = unsafe { en.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map(|d| device_id(&d))
            .unwrap_or_default();
        let devs = render_devices(&en).map_err(|e| format!("Could not list outputs: {e}"))?;
        Ok(devs
            .iter()
            .map(|d| {
                let id = device_id(d);
                OutputDevice {
                    is_default: id == default,
                    name: device_name(d),
                    id,
                }
            })
            .collect())
    })
}

// ------------------------------------------------------- applications

fn wide_to_string(w: &[u16]) -> String {
    let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..n])
}

fn process_table() -> HashMap<u32, ProcInfo> {
    let mut out = HashMap::new();
    // SAFETY: a process snapshot walked with a correctly sized entry.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            out.insert(
                e.th32ProcessID,
                ProcInfo {
                    parent: e.th32ParentProcessID,
                    exe: wide_to_string(&e.szExeFile),
                    path: None,
                    description: None,
                },
            );
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn image_path(pid: u32) -> Option<String> {
    // SAFETY: a limited-rights handle, closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn file_description(path: &str) -> Option<String> {
    let wpath = HSTRING::from(path);
    // SAFETY: the version block is sized by the OS and outlives the
    // pointers VerQueryValueW returns into it.
    unsafe {
        let size = GetFileVersionInfoSizeW(&wpath, None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(&wpath, 0, size, block.as_mut_ptr() as *mut _).ok()?;
        let mut langs: Vec<(u16, u16)> = Vec::new();
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        if VerQueryValueW(
            block.as_ptr() as *const _,
            &HSTRING::from("\\VarFileInfo\\Translation"),
            &mut p,
            &mut len,
        )
        .as_bool()
            && !p.is_null()
        {
            let pairs = std::slice::from_raw_parts(p as *const u16, (len / 2) as usize);
            let lang = pairs.iter().step_by(2);
            let cp = pairs.iter().skip(1).step_by(2);
            langs.extend(lang.zip(cp).map(|(&l, &c)| (l, c)));
        }
        langs.push((0x0409, 0x04B0));
        langs.push((0x0409, 0x04E4));
        for (lang, cp) in langs {
            let key = format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription");
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            if VerQueryValueW(
                block.as_ptr() as *const _,
                &HSTRING::from(key),
                &mut p,
                &mut len,
            )
            .as_bool()
                && !p.is_null()
                && len > 0
            {
                let s = std::slice::from_raw_parts(p as *const u16, len as usize);
                let s = wide_to_string(s);
                if !s.trim().is_empty() {
                    return Some(s.trim().to_string());
                }
            }
        }
        None
    }
}

fn sessions() -> Result<Vec<RawSession>, String> {
    let en = enumerator().map_err(|e| format!("Audio devices are unavailable: {e}"))?;
    let devs = render_devices(&en).map_err(|e| format!("Could not list outputs: {e}"))?;
    let mut out = Vec::new();
    for d in devs {
        // SAFETY: COM calls on live interfaces; failures skip the item.
        unsafe {
            let Ok(mgr) = d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(list) = mgr.GetSessionEnumerator() else {
                continue;
            };
            let n = list.GetCount().unwrap_or(0);
            for i in 0..n {
                let Ok(ctl) = list.GetSession(i) else {
                    continue;
                };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let state = ctl.GetState().unwrap_or(AudioSessionStateExpired);
                if state == AudioSessionStateExpired {
                    continue;
                }
                out.push(RawSession {
                    pid: ctl2.GetProcessId().unwrap_or(0),
                    display_name: ctl.GetDisplayName().map(pwstr_take).unwrap_or_default(),
                    active: state == AudioSessionStateActive,
                    system_sounds: ctl2.IsSystemSoundsSession() == HRESULT(0),
                });
            }
        }
    }
    Ok(out)
}

pub fn list_audio_apps() -> Result<Vec<AudioApp>, String> {
    on_com_thread(|| {
        let raw = sessions()?;
        let mut procs = process_table();
        // Paths and descriptions only for the processes that matter.
        let own = std::process::id();
        let mut wanted: Vec<u32> = Vec::new();
        for s in &raw {
            wanted.push(s.pid);
            wanted.push(super::app_root(s.pid, &procs));
        }
        wanted.sort_unstable();
        wanted.dedup();
        for pid in wanted {
            if pid == 0 {
                continue;
            }
            let path = image_path(pid);
            let desc = path.as_deref().and_then(file_description);
            let e = procs.entry(pid).or_default();
            if e.exe.is_empty() {
                if let Some(p) = &path {
                    e.exe = p.rsplit(['\\', '/']).next().unwrap_or("").to_string();
                }
            }
            e.path = path;
            e.description = desc;
        }
        Ok(build_app_list(&raw, &procs, own))
    })
}

// ------------------------------------------------------------ capture

/// Signals a condvar when activation finishes. Holding an `Arc` keeps it
/// valid however late the OS calls back.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Completion(Arc<(Mutex<bool>, Condvar)>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
    fn ActivateCompleted(
        &self,
        _op: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> WinResult<()> {
        let (m, cv) = &*self.0;
        if let Ok(mut done) = m.lock() {
            *done = true;
        }
        cv.notify_all();
        Ok(())
    }
}

/// A `PROPVARIANT` holding a `VT_BLOB`, laid out like the real thing.
/// `ActivateAudioInterfaceAsync` only reads it, and it is never cleared,
/// so the blob may point at our own stack.
#[repr(C)]
struct BlobVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    size: u32,
    data: *const u8,
}

const VT_BLOB: u16 = 65;

fn activate_process_loopback(
    pid: u32,
    mode: PROCESS_LOOPBACK_MODE,
) -> Result<IAudioClient, String> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: mode,
            },
        },
    };
    let pv = BlobVariant {
        vt: VT_BLOB,
        r1: 0,
        r2: 0,
        r3: 0,
        size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
        data: &params as *const _ as *const u8,
    };
    let signal = Arc::new((Mutex::new(false), Condvar::new()));
    let handler: IActivateAudioInterfaceCompletionHandler = Completion(signal.clone()).into();
    let fail =
        |e: windows::core::Error| format!("Per-app capture could not start: {}", e.message());
    // SAFETY: `pv` and `params` outlive the call, which copies them; the
    // handler is a valid agile COM object.
    let op = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&pv as *const BlobVariant as *const windows::core::PROPVARIANT),
            &handler,
        )
    }
    .map_err(fail)?;
    {
        let (m, cv) = &*signal;
        let guard = m
            .lock()
            .map_err(|_| "Per-app capture failed.".to_string())?;
        let (guard, _) = cv
            .wait_timeout_while(guard, Duration::from_secs(5), |done| !*done)
            .map_err(|_| "Per-app capture failed.".to_string())?;
        if !*guard {
            return Err("Per-app capture did not answer in time.".into());
        }
    }
    let mut hr = HRESULT(0);
    let mut unk = None;
    // SAFETY: the operation completed; both out-params are valid.
    unsafe { op.GetActivateResult(&mut hr, &mut unk) }.map_err(fail)?;
    hr.ok().map_err(fail)?;
    unk.ok_or_else(|| "Per-app capture returned nothing.".to_string())?
        .cast::<IAudioClient>()
        .map_err(fail)
}

/// How the capture thread reads the stream it opened.
struct Opened {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Option<Event>,
    fmt: SampleFmt,
    channels: usize,
    block: usize,
    rate: u32,
}

const BUFFER_HNS: i64 = 2_000_000; // 200 ms

fn init_with_event(
    client: &IAudioClient,
    flags: u32,
    fmt: *const WAVEFORMATEX,
) -> WinResult<Option<Event>> {
    let ev = Event::new()?;
    // SAFETY: `fmt` is a valid format for this client.
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            BUFFER_HNS,
            0,
            fmt,
            None,
        )?;
        client.SetEventHandle(ev.0)?;
    }
    Ok(Some(ev))
}

fn open_device(id: Option<&str>) -> Result<Opened, String> {
    let fail = |e: windows::core::Error| {
        format!("Could not open the output for recording: {}", e.message())
    };
    let en = enumerator().map_err(fail)?;
    // SAFETY: COM calls on live interfaces; the mix format is freed once
    // the clients are initialised.
    unsafe {
        let dev = match id {
            Some(id) => en.GetDevice(&HSTRING::from(id)),
            None => en.GetDefaultAudioEndpoint(eRender, eConsole),
        }
        .map_err(fail)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(fail)?;
        let mix = client.GetMixFormat().map_err(fail)?;
        if mix.is_null() {
            return Err("The output device has no format.".into());
        }
        let wf = *mix;
        let (tag, bits, cb) = (wf.wFormatTag, wf.wBitsPerSample, wf.cbSize);
        let (channels, block, rate) = (wf.nChannels, wf.nBlockAlign, wf.nSamplesPerSec);
        let sub_tag = (tag == 0xFFFE && cb >= 22).then(|| {
            // SubFormat sits after cbSize, 2-byte Samples union and 4-byte
            // channel mask: 18 + 2 + 4 = byte 24 of the struct.
            let p = (mix as *const u8).add(24);
            u16::from_le_bytes([*p, *p.add(1)])
        });
        let parsed = sample_fmt(tag, bits, sub_tag);
        let result = (|| {
            let fmt = parsed.ok_or_else(|| {
                format!("The output's format ({bits} bit, tag {tag:#x}) can't be recorded.")
            })?;
            // Event-driven loopback; older systems that refuse it are polled.
            let (client, event) = match init_with_event(&client, AUDCLNT_STREAMFLAGS_LOOPBACK, mix)
            {
                Ok(ev) => (client, ev),
                Err(_) => {
                    let c2: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(fail)?;
                    c2.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK,
                        BUFFER_HNS,
                        0,
                        mix,
                        None,
                    )
                    .map_err(fail)?;
                    (c2, None)
                }
            };
            let capture: IAudioCaptureClient = client.GetService().map_err(fail)?;
            Ok(Opened {
                client,
                capture,
                event,
                fmt,
                channels: channels.max(1) as usize,
                block: block.max(1) as usize,
                rate,
            })
        })();
        CoTaskMemFree(Some(mix as *const _));
        result
    }
}

/// The rate per-app capture is asked for.
const PROCESS_RATE: u32 = 48_000;

fn open_process(pid: u32, mode: PROCESS_LOOPBACK_MODE) -> Result<Opened, String> {
    let client = activate_process_loopback(pid, mode)?;
    let wf = WAVEFORMATEX {
        wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
        nChannels: 2,
        nSamplesPerSec: PROCESS_RATE,
        nAvgBytesPerSec: PROCESS_RATE * 8,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let fail =
        |e: windows::core::Error| format!("Per-app capture could not start: {}", e.message());
    let event = init_with_event(
        &client,
        AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        &wf,
    )
    .map_err(fail)?;
    // SAFETY: the client is initialised.
    let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(fail)?;
    Ok(Opened {
        client,
        capture,
        event,
        fmt: SampleFmt::F32,
        channels: 2,
        block: 8,
        rate: PROCESS_RATE,
    })
}

pub fn open(
    source: &RecordSource,
    tx: rtrb::Producer<f32>,
    peak: Arc<AtomicU32>,
) -> Result<LoopbackCapture, String> {
    let per_app = super::support().per_app;
    let own = std::process::id();
    let (what, note): (Target, Option<String>) = match source {
        RecordSource::Input | RecordSource::Channels(_) => {
            return Err("Not a system audio source.".into())
        }
        RecordSource::System(dev) => (Target::Device(dev.as_ref().map(|d| d.id.clone())), None),
        RecordSource::SystemExceptSelf if per_app => (
            Target::Process(own, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE),
            None,
        ),
        RecordSource::SystemExceptSelf => (
            Target::Device(None),
            Some(
                "This version of Windows can't leave AudiForm 2 out, so the whole \
                 desktop was recorded (mute monitoring to avoid feedback)."
                    .into(),
            ),
        ),
        RecordSource::App(_) if !per_app => {
            return Err(
                "Recording one application needs Windows 10 build 20348 or Windows 11. \
                 Choose System audio to record the whole desktop."
                    .into(),
            )
        }
        RecordSource::App(a) => (
            Target::Process(a.pid, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE),
            None,
        ),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let error = Arc::new(Mutex::new(None));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    let label = source.label();
    let thread = {
        let stop = stop.clone();
        let error = error.clone();
        std::thread::Builder::new()
            .name("af2-loopback".into())
            .spawn(move || run(what, tx, peak, stop, error, ready_tx, label))
            .map_err(|e| format!("Could not start the capture thread: {e}"))?
    };
    let rate = match ready_rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(rate)) => rate,
        Ok(Err(e)) => {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
            return Err(e);
        }
        Err(_) => {
            stop.store(true, Ordering::Relaxed);
            return Err("the capture did not start in time.".into());
        }
    };
    Ok(LoopbackCapture {
        stop,
        thread: Some(thread),
        error,
        rate,
        note,
    })
}

enum Target {
    Device(Option<String>),
    Process(u32, PROCESS_LOOPBACK_MODE),
}

fn run(
    what: Target,
    mut tx: rtrb::Producer<f32>,
    peak: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    ready: mpsc::Sender<Result<u32, String>>,
    label: String,
) {
    let _com = Com::init();
    let opened = match what {
        Target::Device(id) => open_device(id.as_deref()),
        Target::Process(pid, mode) => open_process(pid, mode),
    };
    let o = match opened {
        Ok(o) => o,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // SAFETY: the client is initialised.
    if let Err(e) = unsafe { o.client.Start() } {
        let _ = ready.send(Err(format!("could not start ({})", e.message())));
        return;
    }
    let _ = ready.send(Ok(o.rate));

    // Ask for audio-thread scheduling; fine without it.
    let mut task = 0u32;
    // SAFETY: MMCSS registration with a static task name.
    let mmcss = unsafe { AvSetMmThreadCharacteristicsW(&HSTRING::from("Audio"), &mut task) }.ok();

    let started = Instant::now();
    let mut delivered: u64 = 0;
    let rate = o.rate.max(1);
    let fail = |e: windows::core::Error| {
        if e.code() == AUDCLNT_E_DEVICE_INVALIDATED {
            format!("{label}: the output device went away; the take stops here.")
        } else {
            format!("{label}: capture stopped ({})", e.message())
        }
    };
    let result: Result<(), String> = (|| {
        while !stop.load(Ordering::Relaxed) {
            match &o.event {
                // SAFETY: a live event handle.
                Some(ev) => unsafe {
                    WaitForSingleObject(ev.0, 20);
                },
                None => std::thread::sleep(Duration::from_millis(10)),
            }
            loop {
                // SAFETY: GetBuffer/ReleaseBuffer pairs on a started client;
                // the slice covers exactly the frames the OS lent us.
                let frames = unsafe {
                    let n = o.capture.GetNextPacketSize().map_err(fail)?;
                    if n == 0 {
                        break;
                    }
                    let mut data = std::ptr::null_mut();
                    let mut frames = 0u32;
                    let mut flags = 0u32;
                    o.capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .map_err(fail)?;
                    let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                    if silent || data.is_null() {
                        for _ in 0..frames {
                            push_frame(&mut tx, 0.0, 0.0);
                        }
                    } else {
                        let bytes = std::slice::from_raw_parts(data, frames as usize * o.block);
                        let pk = convert(bytes, o.fmt, o.channels, |l, r| {
                            push_frame(&mut tx, l, r);
                        });
                        crate::store_max_f32(&peak, pk);
                    }
                    o.capture.ReleaseBuffer(frames).map_err(fail)?;
                    frames
                };
                delivered += frames as u64;
            }
            let expected = (started.elapsed().as_secs_f64() * rate as f64) as u64;
            let fill = silence_to_insert(expected, delivered, rate);
            for _ in 0..fill {
                push_frame(&mut tx, 0.0, 0.0);
            }
            delivered += fill;
        }
        Ok(())
    })();
    // SAFETY: stopping a started client and undoing MMCSS.
    unsafe {
        let _ = o.client.Stop();
        if let Some(h) = mmcss {
            let _ = AvRevertMmThreadCharacteristics(h);
        }
    }
    if let Err(e) = result {
        if let Ok(mut slot) = error.lock() {
            *slot = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_variant_matches_propvariant_layout() {
        assert_eq!(
            std::mem::size_of::<BlobVariant>(),
            std::mem::size_of::<windows::core::PROPVARIANT>()
        );
        assert_eq!(
            std::mem::align_of::<BlobVariant>(),
            std::mem::align_of::<windows::core::PROPVARIANT>()
        );
    }

    #[test]
    fn the_build_number_is_readable() {
        assert!(os_build().is_some_and(|b| b > 7000));
    }
}
