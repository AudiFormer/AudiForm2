// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Audio drivers and devices: which host (the system's own, or ASIO),
//! which devices it has, and which stream format to open.
//!
//! ASIO is a Cargo feature (`asio`, off by default; see `docs/asio.md`).
//! Without it the ASIO choice is shown as unavailable, and the installed
//! ASIO drivers are still listed (from the registry; nothing is loaded),
//! so the user can see why. With it, only the chosen driver is ever
//! loaded: cpal's own ASIO listing loads every installed driver in turn,
//! which on a machine with many drivers is slow and has crashed apps.
//!
//! Every ASIO call happens on the UI thread (the one that opens streams):
//! drivers are COM objects and some expect the thread that created them.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{SampleFormat, SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange};
use serde::{Deserialize, Serialize};

/// Which driver layer plays and records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum HostKind {
    /// The system's own: WASAPI on Windows, Core Audio, ALSA.
    #[default]
    System,
    /// Steinberg ASIO (Windows, builds with the `asio` feature).
    Asio,
}

/// True when this build can use ASIO.
pub const ASIO_BUILT: bool = cfg!(all(windows, feature = "asio"));

/// The credit the About box shows in builds with ASIO (None without).
pub const ASIO_CREDIT: Option<&str> = if ASIO_BUILT {
    Some("ASIO is a trademark and software of Steinberg Media Technologies GmbH.")
} else {
    None
};

/// The system host's name ("WASAPI" on Windows).
pub fn system_name() -> String {
    cpal::default_host().id().name().to_string()
}

/// A device and how many channels it offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
}

/// What a host has, listed for Settings and the track input menus.
#[derive(Debug, Clone, Default)]
pub struct DeviceList {
    pub host: HostKind,
    /// "WASAPI", or "ASIO" with the driver.
    pub driver: String,
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
    pub default_out: Option<String>,
    pub default_in: Option<String>,
    /// Installed ASIO drivers (listed on every host; nothing is loaded).
    pub asio_drivers: Vec<String>,
    /// The ASIO driver in use or that would be used.
    pub asio_driver: Option<String>,
    /// Sample rates the chosen (or default) output can run at.
    pub rates: Vec<u32>,
    /// The device's own sample rate now.
    pub rate: Option<u32>,
    /// Buffer sizes the device accepts (min, max), when it says.
    pub buffer_range: Option<(u32, u32)>,
    /// Why the list is empty or short.
    pub error: Option<String>,
}

impl DeviceList {
    pub fn output(&self, name: &str) -> Option<&DeviceInfo> {
        self.outputs.iter().find(|d| d.name == name)
    }

    pub fn input(&self, name: &str) -> Option<&DeviceInfo> {
        self.inputs.iter().find(|d| d.name == name)
    }
}

/// Lists `host`'s devices. On ASIO that is one device, named after the
/// driver (`asio_driver`, or the best guess among those installed), for
/// both playing and recording. `output` picks whose rates and buffer sizes
/// are reported (None: the default output).
pub fn list(host: HostKind, asio_driver: Option<&str>, output: Option<&str>) -> DeviceList {
    let asio_drivers = asio_drivers();
    let picked = pick_driver(&asio_drivers, asio_driver);
    let mut l = match host {
        HostKind::System => list_system(output),
        HostKind::Asio => list_asio(picked.as_deref(), &asio_drivers),
    };
    l.asio_drivers = asio_drivers;
    l.asio_driver = picked;
    l
}

fn list_system(output: Option<&str>) -> DeviceList {
    let h = cpal::default_host();
    let mut outputs: Vec<DeviceInfo> = h
        .output_devices()
        .map(|it| it.filter_map(|d| output_info(&d)).collect())
        .unwrap_or_default();
    let mut inputs: Vec<DeviceInfo> = h
        .input_devices()
        .map(|it| it.filter_map(|d| input_info(&d)).collect())
        .unwrap_or_default();
    dedup(&mut outputs);
    dedup(&mut inputs);
    let default_out = h.default_output_device().and_then(|d| d.name().ok());
    let default_in = h.default_input_device().and_then(|d| d.name().ok());
    let dev = output
        .and_then(|n| find_by_name(h.output_devices().ok(), n))
        .or_else(|| h.default_output_device());
    let mut l = DeviceList {
        host: HostKind::System,
        driver: system_name(),
        outputs,
        inputs,
        default_out,
        default_in,
        ..Default::default()
    };
    if let Some(d) = dev {
        caps_into(&d, &mut l);
    }
    if l.outputs.is_empty() && l.inputs.is_empty() {
        l.error = Some("No audio devices were found.".into());
    }
    l
}

fn list_asio(driver: Option<&str>, installed: &[String]) -> DeviceList {
    let mut l = DeviceList {
        host: HostKind::Asio,
        driver: "ASIO".into(),
        ..Default::default()
    };
    let Some(name) = driver else {
        l.error = Some("No ASIO drivers are installed.".into());
        return l;
    };
    l.driver = format!("ASIO ({name})");
    if !ASIO_BUILT {
        l.error = Some(NO_ASIO.into());
        return l;
    }
    // Right after a restart the old process may still hold the driver for
    // a moment: try a few times before calling it missing.
    let tries = if installed.is_empty() { 1 } else { 5 };
    let dev = retry(
        tries,
        || crate::asio::device(name),
        || std::thread::sleep(std::time::Duration::from_millis(400)),
    );
    match dev {
        Ok(d) => {
            if let Some(o) = output_info(&d) {
                l.outputs.push(o);
            }
            if let Some(i) = input_info(&d) {
                l.inputs.push(i);
            }
            l.default_out = Some(name.to_string());
            l.default_in = Some(name.to_string());
            caps_into(&d, &mut l);
        }
        Err(e) => l.error = Some(e),
    }
    l
}

/// Shown where ASIO would be.
pub const NO_ASIO: &str = "This build of AudiForm 2 has no ASIO. It plays and records \
     through the system's own driver instead.";

fn caps_into(d: &cpal::Device, l: &mut DeviceList) {
    let ranges: Vec<SupportedStreamConfigRange> = d
        .supported_output_configs()
        .map(|it| it.collect())
        .unwrap_or_default();
    l.rates = rates_in(&ranges);
    if let Ok(c) = d.default_output_config() {
        l.rate = Some(sane_rate(c.sample_rate().0));
        if let SupportedBufferSize::Range { min, max } = c.buffer_size() {
            l.buffer_range = Some((*min, *max));
        }
        if let Some(r) = l.rate.filter(|r| !l.rates.contains(r)) {
            l.rates.push(r);
            l.rates.sort_unstable();
        }
    }
}

fn find_by_name(
    it: Option<impl Iterator<Item = cpal::Device>>,
    name: &str,
) -> Option<cpal::Device> {
    it?.find(|d| d.name().is_ok_and(|n| n == name))
}

fn output_info(d: &cpal::Device) -> Option<DeviceInfo> {
    let name = d.name().ok()?;
    let widest = d
        .supported_output_configs()
        .ok()
        .and_then(|it| it.map(|c| c.channels()).max());
    let def = d.default_output_config().ok().map(|c| c.channels());
    let channels = widest.max(def)?;
    Some(DeviceInfo { name, channels })
}

fn input_info(d: &cpal::Device) -> Option<DeviceInfo> {
    let name = d.name().ok()?;
    // Recording opens the default config, so that is what can be picked.
    let channels = d.default_input_config().ok()?.channels();
    (channels > 0).then_some(DeviceInfo { name, channels })
}

/// Drops repeated names, keeping the first.
fn dedup(list: &mut Vec<DeviceInfo>) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|d| seen.insert(d.name.clone()));
}

/// Calls `f` up to `tries` times until it succeeds, waiting between tries.
pub fn retry<T>(
    tries: usize,
    mut f: impl FnMut() -> Result<T, String>,
    wait: impl Fn(),
) -> Result<T, String> {
    let mut last = Err("Nothing was tried.".to_string());
    for i in 0..tries.max(1) {
        if i > 0 {
            wait();
        }
        last = f();
        if last.is_ok() {
            break;
        }
    }
    last
}

// ------------------------------------------------------------ ASIO drivers

/// Names that are rarely the interface's own driver: wrappers, virtual
/// cables and generic drivers. Picked only when nothing else is installed.
const UNLIKELY: [&str; 8] = [
    "asio4all",
    "flexasio",
    "virtual",
    "generic low latency",
    "realtek",
    "fl studio",
    "vb-",
    "voicemeeter",
];

/// The driver to use: the chosen one if installed, else the first that
/// looks like real hardware, else the first installed.
pub fn pick_driver(installed: &[String], chosen: Option<&str>) -> Option<String> {
    if let Some(c) = chosen.filter(|c| installed.iter().any(|i| i == c)) {
        return Some(c.to_string());
    }
    let likely = |n: &String| {
        let l = n.to_lowercase();
        !UNLIKELY.iter().any(|u| l.contains(u))
    };
    installed
        .iter()
        .find(|n| likely(n))
        .or_else(|| installed.first())
        .cloned()
}

/// Sorted, without repeats, blanks or the bare "ASIO" key.
pub fn clean_driver_names(mut names: Vec<String>) -> Vec<String> {
    names.retain(|n| !n.trim().is_empty() && !n.eq_ignore_ascii_case("asio"));
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// The installed ASIO drivers, read from the registry (`HKLM\SOFTWARE\ASIO`;
/// a 64-bit app sees the 64-bit drivers, the only ones it can load).
/// Loads nothing.
pub fn asio_drivers() -> Vec<String> {
    #[cfg(windows)]
    {
        clean_driver_names(registry_asio_keys())
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn registry_asio_keys() -> Vec<String> {
    use windows::core::{w, PWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    };
    let mut out = Vec::new();
    let mut key = HKEY::default();
    // SAFETY: plain registry calls with buffers that outlive them.
    unsafe {
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\ASIO"),
            0,
            KEY_READ,
            &mut key,
        )
        .is_err()
        {
            return out;
        }
        for i in 0..512u32 {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            let r = RegEnumKeyExW(
                key,
                i,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
                None,
                PWSTR::null(),
                None,
                None,
            );
            if r.is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        let _ = RegCloseKey(key);
    }
    out
}

// ------------------------------------------------------------ formats

/// Rates offered when a device gives ranges.
pub const COMMON_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Some drivers report 0 Hz until a stream runs: plan for 48 kHz then.
pub fn sane_rate(rate: u32) -> u32 {
    if rate == 0 {
        48_000
    } else {
        rate
    }
}

/// Sample formats the output and input callbacks convert.
pub fn usable_format(f: SampleFormat) -> bool {
    matches!(
        f,
        SampleFormat::F32
            | SampleFormat::F64
            | SampleFormat::I16
            | SampleFormat::I32
            | SampleFormat::U16
            | SampleFormat::U32
            | SampleFormat::I8
            | SampleFormat::U8
    )
}

fn covers(r: &SupportedStreamConfigRange, rate: u32) -> bool {
    r.min_sample_rate().0 <= rate && rate <= r.max_sample_rate().0
}

/// The common rates any usable range covers.
pub fn rates_in(ranges: &[SupportedStreamConfigRange]) -> Vec<u32> {
    COMMON_RATES
        .into_iter()
        .filter(|&r| {
            ranges
                .iter()
                .any(|x| usable_format(x.sample_format()) && covers(x, r))
        })
        .collect()
}

/// The output format to open: at `want_rate` when the device offers it
/// (else its own rate, with a note saying so), and wide enough for
/// `want_channels` outputs when it can be: the device's default if that
/// is wide enough, else its widest config at the rate, preferring the
/// default's sample format, then 32-bit float.
pub fn choose_config(
    default: &SupportedStreamConfig,
    ranges: &[SupportedStreamConfigRange],
    want_channels: usize,
    want_rate: Option<u32>,
) -> (SupportedStreamConfig, Option<String>) {
    let def_rate = sane_rate(default.sample_rate().0);
    let def = SupportedStreamConfig::new(
        default.channels(),
        cpal::SampleRate(def_rate),
        *default.buffer_size(),
        default.sample_format(),
    );
    let usable: Vec<&SupportedStreamConfigRange> = ranges
        .iter()
        .filter(|r| usable_format(r.sample_format()))
        .collect();
    let mut note = None;
    let rate = match want_rate {
        Some(w) if w != def_rate => {
            if usable.iter().any(|r| covers(r, w)) {
                w
            } else {
                note = Some(format!(
                    "The device cannot run at {w} Hz, so it runs at {def_rate} Hz."
                ));
                def_rate
            }
        }
        _ => def_rate,
    };
    let def_ok = usable_format(def.sample_format());
    if rate == def_rate && def_ok && usize::from(def.channels()) >= want_channels {
        return (def, note);
    }
    let best = usable.iter().filter(|r| covers(r, rate)).max_by_key(|r| {
        (
            r.channels(),
            r.sample_format() == def.sample_format(),
            r.sample_format() == SampleFormat::F32,
        )
    });
    match best {
        Some(b) if rate != def_rate || !def_ok || b.channels() > def.channels() => {
            ((**b).with_sample_rate(cpal::SampleRate(rate)), note)
        }
        _ => (def, note),
    }
}

/// Buffer sizes to offer: powers of two from 32 to 4096 that the device
/// accepts (all of them when it does not say).
pub fn buffer_choices(range: Option<(u32, u32)>) -> Vec<u32> {
    let all = (5..=12).map(|p| 1u32 << p);
    match range {
        Some((min, max)) if min <= max && max > 0 => {
            let mut v: Vec<u32> = all.filter(|n| (min..=max).contains(n)).collect();
            // A driver with a fixed size (or an odd range) still gets one.
            if v.is_empty() {
                v.push(min.max(1));
            }
            v
        }
        _ => all.collect(),
    }
}

/// `n` moved into the device's range.
pub fn fit_buffer(n: u32, range: &SupportedBufferSize) -> u32 {
    match range {
        SupportedBufferSize::Range { min, max } if min <= max => n.clamp(*min, *max),
        _ => n,
    }
}

/// Frames to latency, for display.
pub fn latency_ms(frames: u32, rate: u32) -> f64 {
    frames as f64 * 1000.0 / rate.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::SampleRate;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_driver_guess_skips_wrappers_and_virtual_cables() {
        let installed = clean_driver_names(names(&[
            "ASIO4ALL v2",
            "FL Studio ASIO",
            "Generic Low Latency ASIO Driver",
            "Realtek ASIO",
            "SSL ASIO Driver",
            "Voicemeeter AUX Virtual ASIO",
            "VB-Audio ASIO",
        ]));
        assert_eq!(
            pick_driver(&installed, None).as_deref(),
            Some("SSL ASIO Driver")
        );
        // A chosen driver that is installed wins, even a wrapper.
        assert_eq!(
            pick_driver(&installed, Some("ASIO4ALL v2")).as_deref(),
            Some("ASIO4ALL v2")
        );
        // A chosen one that is gone: the guess.
        assert_eq!(
            pick_driver(&installed, Some("Focusrite USB ASIO")).as_deref(),
            Some("SSL ASIO Driver")
        );
        // Only wrappers: the first of them.
        let wrappers = clean_driver_names(names(&["Voicemeeter Virtual ASIO", "ASIO4ALL v2"]));
        assert_eq!(pick_driver(&wrappers, None).as_deref(), Some("ASIO4ALL v2"));
        assert_eq!(pick_driver(&[], None), None);
        assert_eq!(pick_driver(&[], Some("SSL ASIO Driver")), None);
    }

    #[test]
    fn registry_names_are_cleaned() {
        assert_eq!(
            clean_driver_names(names(&["Yamaha", "ASIO", "", "asio4all", "Yamaha"])),
            names(&["asio4all", "Yamaha"])
        );
    }

    #[test]
    fn asio_stays_off_when_not_built() {
        assert_eq!(ASIO_CREDIT.is_some(), ASIO_BUILT);
        if !ASIO_BUILT {
            let l = list_asio(Some("SSL ASIO Driver"), &names(&["SSL ASIO Driver"]));
            assert!(l.outputs.is_empty() && l.inputs.is_empty());
            assert_eq!(l.error.as_deref(), Some(NO_ASIO));
            assert_eq!(l.driver, "ASIO (SSL ASIO Driver)");
        }
        let none = list_asio(None, &[]);
        assert_eq!(
            none.error.as_deref(),
            Some("No ASIO drivers are installed.")
        );
    }

    #[test]
    fn a_zero_rate_becomes_48k() {
        assert_eq!(sane_rate(0), 48_000);
        assert_eq!(sane_rate(44_100), 44_100);
        let def = SupportedStreamConfig::new(
            2,
            SampleRate(0),
            SupportedBufferSize::Unknown,
            SampleFormat::F32,
        );
        let (c, note) = choose_config(&def, &[], 2, None);
        assert_eq!(c.sample_rate().0, 48_000);
        assert!(note.is_none());
    }

    fn range(ch: u16, min: u32, max: u32, f: SampleFormat) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            ch,
            SampleRate(min),
            SampleRate(max),
            SupportedBufferSize::Range { min: 32, max: 4096 },
            f,
        )
    }

    fn cfg(ch: u16, rate: u32, f: SampleFormat) -> SupportedStreamConfig {
        SupportedStreamConfig::new(
            ch,
            SampleRate(rate),
            SupportedBufferSize::Range { min: 32, max: 4096 },
            f,
        )
    }

    #[test]
    fn a_wide_enough_default_is_kept() {
        let def = cfg(2, 48_000, SampleFormat::F32);
        let ranges = [range(8, 44_100, 96_000, SampleFormat::F32)];
        let (c, _) = choose_config(&def, &ranges, 2, None);
        assert_eq!(c, def);
    }

    #[test]
    fn the_widest_config_at_the_rate_is_taken_for_more_outputs() {
        let def = cfg(2, 48_000, SampleFormat::I32);
        let ranges = [
            range(2, 48_000, 48_000, SampleFormat::I32),
            range(12, 48_000, 48_000, SampleFormat::F32),
            range(12, 48_000, 48_000, SampleFormat::I32),
            range(16, 96_000, 96_000, SampleFormat::I32),
        ];
        let (c, _) = choose_config(&def, &ranges, 12, None);
        assert_eq!(c.channels(), 12);
        // The default's format before float at the same width.
        assert_eq!(c.sample_format(), SampleFormat::I32);
        assert_eq!(c.sample_rate().0, 48_000);
        // Not more outputs to be had: the default.
        let narrow = [range(2, 48_000, 48_000, SampleFormat::F32)];
        assert_eq!(choose_config(&def, &narrow, 6, None).0, def);
    }

    #[test]
    fn a_chosen_rate_is_used_when_offered_and_noted_when_not() {
        let def = cfg(2, 48_000, SampleFormat::F32);
        let ranges = [
            range(2, 44_100, 44_100, SampleFormat::F32),
            range(2, 48_000, 48_000, SampleFormat::F32),
        ];
        let (c, note) = choose_config(&def, &ranges, 2, Some(44_100));
        assert_eq!(c.sample_rate().0, 44_100);
        assert!(note.is_none());
        let (c, note) = choose_config(&def, &ranges, 2, Some(96_000));
        assert_eq!(c.sample_rate().0, 48_000);
        assert_eq!(
            note.as_deref(),
            Some("The device cannot run at 96000 Hz, so it runs at 48000 Hz.")
        );
    }

    #[test]
    fn formats_the_callbacks_cannot_convert_are_avoided() {
        let def = cfg(2, 48_000, SampleFormat::I64);
        let ranges = [
            range(2, 48_000, 48_000, SampleFormat::I64),
            range(2, 48_000, 48_000, SampleFormat::I16),
        ];
        let (c, _) = choose_config(&def, &ranges, 2, None);
        assert_eq!(c.sample_format(), SampleFormat::I16);
        assert_eq!(rates_in(&ranges), vec![48_000]);
    }

    #[test]
    fn rates_come_from_the_ranges() {
        let ranges = [range(2, 44_100, 96_000, SampleFormat::F32)];
        assert_eq!(rates_in(&ranges), vec![44_100, 48_000, 88_200, 96_000]);
    }

    #[test]
    fn buffer_sizes_follow_the_driver_range() {
        assert_eq!(buffer_choices(Some((64, 512))), vec![64, 128, 256, 512]);
        assert_eq!(buffer_choices(Some((96, 96))), vec![96]);
        assert_eq!(buffer_choices(None).first(), Some(&32));
        assert_eq!(buffer_choices(None).last(), Some(&4096));
        let r = SupportedBufferSize::Range { min: 64, max: 1024 };
        assert_eq!(fit_buffer(16, &r), 64);
        assert_eq!(fit_buffer(4096, &r), 1024);
        assert_eq!(fit_buffer(256, &r), 256);
        assert_eq!(fit_buffer(77, &SupportedBufferSize::Unknown), 77);
        assert!((latency_ms(480, 48_000) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn retries_stop_at_the_first_success() {
        let mut n = 0;
        let waits = std::cell::Cell::new(0);
        let r = retry(
            5,
            || {
                n += 1;
                if n < 3 {
                    Err("busy".into())
                } else {
                    Ok(n)
                }
            },
            || waits.set(waits.get() + 1),
        );
        assert_eq!(r, Ok(3));
        assert_eq!(waits.get(), 2);
        let mut m = 0;
        let r: Result<(), String> = retry(
            4,
            || {
                m += 1;
                Err(format!("busy {m}"))
            },
            || {},
        );
        assert_eq!(r, Err("busy 4".into()));
    }

    #[test]
    fn repeated_device_names_are_listed_once() {
        let mut v = vec![
            DeviceInfo {
                name: "A".into(),
                channels: 2,
            },
            DeviceInfo {
                name: "A".into(),
                channels: 8,
            },
            DeviceInfo {
                name: "B".into(),
                channels: 2,
            },
        ];
        dedup(&mut v);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].channels, 2);
    }
}
