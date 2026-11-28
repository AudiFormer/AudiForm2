// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Bringing settings and the VST3 plug-in scan over from Audiform 1.
//!
//! Audiform 1 keeps its settings in `preferences.alprefs` and its plug-in
//! scan in `plugins.json`, in `%APPDATA%\io.github.nwh212.audiform`. Both
//! are JSON; only values are read (it is the user's own data), and only
//! those AudiForm 2 has a place for. The first start after an install
//! asks once; Settings > General can do it again later.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::state::State;
use crate::theme::c;

/// What was found of Audiform 1 on this computer.
#[derive(Debug, Clone)]
pub struct Found {
    pub dir: PathBuf,
    /// `values` of preferences.alprefs.
    pub prefs: Option<Value>,
    /// (plug-ins, extra folders) in its plug-in scan.
    pub plugins: Option<(usize, usize)>,
}

/// Audiform 1's data folder.
pub fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let d = PathBuf::from(base).join("io.github.nwh212.audiform");
    d.is_dir().then_some(d)
}

pub fn find() -> Option<Found> {
    find_in(&data_dir()?)
}

pub fn find_in(dir: &Path) -> Option<Found> {
    let prefs = std::fs::read_to_string(dir.join("preferences.alprefs"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')).ok())
        .and_then(|v| v.get("values").cloned())
        .filter(Value::is_object);
    let plugins = std::fs::read_to_string(dir.join("plugins.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| {
            (
                v["plugins"].as_array().map_or(0, Vec::len),
                v["folders"].as_array().map_or(0, Vec::len),
            )
        })
        .filter(|(n, f)| n + f > 0);
    (prefs.is_some() || plugins.is_some()).then(|| Found {
        dir: dir.to_path_buf(),
        prefs,
        plugins,
    })
}

/// "#rrggbb" to RGB; empty or unreadable is None.
fn hex(v: &Value) -> Option<[u8; 3]> {
    let s = v.as_str()?.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

fn b(v: &Value) -> Option<bool> {
    v.as_bool()
}
fn u(v: &Value) -> Option<u64> {
    v.as_u64()
}
fn f(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// Lays Audiform 1's settings over AudiForm 2's. Returns what changed,
/// one line each.
pub fn apply_prefs(st: &mut State, v: &Value) -> Vec<String> {
    use crate::sessions::MediaPolicy;
    use crate::transport::{PlayMode, TimeFormat};
    let mut done = Vec::new();
    let a = &v["app.settings"];
    let p = &mut st.prefs;
    let mut note = |s: &str| done.push(s.to_string());

    // Autosave and backups.
    if let Some(x) = b(&a["autosave"]) {
        p.autosave = x;
    }
    if let Some(x) = u(&a["autosaveMinutes"]) {
        p.autosave_minutes = x.clamp(1, 60) as u32;
        note("Autosave");
    }
    if let Some(x) = b(&a["backups"]) {
        p.backups = x;
    }
    if let Some(x) = u(&a["backupCount"]) {
        p.backups_keep = x.clamp(1, 50) as u32;
        note("Backups");
    }
    // Session media.
    if let Some(s) = a["copyMedia"].as_str() {
        p.media_policy = match s {
            "always" => MediaPolicy::Copy,
            "ask" => MediaPolicy::Ask,
            _ => MediaPolicy::Leave,
        };
        note("Session media");
    }
    if let Some(s) = a["mediaFolder"].as_str().filter(|s| !s.trim().is_empty()) {
        p.media_folder = s.to_string();
    }
    if let Some(x) = b(&a["collectOnSave"]) {
        p.copy_on_save = x;
    }
    // Audio devices.
    match a["driver"].as_str() {
        Some("asio") if af2_engine::ASIO_BUILT => {
            p.audio.host = af2_engine::HostKind::Asio;
            p.audio.asio_driver = a["asioDriver"].as_str().map(str::to_string);
            p.audio.buffer = u(&a["asioBuffer"]).map(|n| n as u32);
            note("Audio driver (ASIO)");
        }
        Some(_) => {
            p.audio.host = af2_engine::HostKind::System;
            note("Audio driver");
        }
        None => {}
    }
    if let Some(s) = a["output"].as_str() {
        p.audio.output = Some(s.to_string());
    }
    if let Some(s) = a["waveInput"].as_str() {
        p.audio.input = Some(s.to_string());
    }
    if let Some(pair) = a["outputPair"].as_array() {
        if let (Some(l), Some(r)) = (pair.first().and_then(u), pair.get(1).and_then(u)) {
            p.audio.map.stereo = [l as u16, r as u16];
        }
    }
    if let Some(m) = a["speakerOutputs"].as_object() {
        for (k, out) in m {
            if let Some(o) = u(out) {
                p.audio.map.speakers.insert(k.clone(), o as u16);
            }
        }
    }
    // Metronome.
    let m = &a["metronome"];
    if m.is_object() {
        use af2_core::midi::{ClickSound, Subdivide};
        let mt = &mut p.metronome;
        if let Some(x) = b(&m["accent"]) {
            mt.accent = x;
        }
        if let Some(x) = f(&m["levelDb"]) {
            mt.level_db = x as f32;
        }
        if let Some(x) = b(&m["muted"]) {
            mt.muted = x;
        }
        if let Some(s) = m["sound"].as_str() {
            mt.sound = match s {
                "wood" => ClickSound::Wood,
                "beep" => ClickSound::Beep,
                "shaker" => ClickSound::Shaker,
                _ => ClickSound::Tick,
            };
        }
        if let Some(s) = m["subdivide"].as_str() {
            mt.subdivide = match s {
                "halves" | "eighths" => Subdivide::Halves,
                "triplets" => Subdivide::Triplets,
                "quarters" | "sixteenths" => Subdivide::Quarters,
                _ => Subdivide::None,
            };
        }
        if let Some(s) = m["when"].as_str() {
            mt.recording_only = s == "recording";
        }
        note("Metronome");
    }
    if let Some(x) = b(&a["soloExclusive"]) {
        p.solo_exclusive = x;
        note("Exclusive solo");
    }
    if let Some(x) = b(&a["openChainOnAdd"]) {
        p.show_effects_on_pick = x;
    }
    // Colours the user picked.
    let cols = &a["colors"];
    if cols.is_object() {
        let mut any = false;
        for (key, slot) in [
            ("accent", &mut p.colors.accent),
            ("playhead", &mut p.colors.playhead),
            ("selection", &mut p.colors.selection),
            ("waveform", &mut p.colors.waveform),
        ] {
            if let Some(rgb) = hex(&cols[key]) {
                *slot = Some(rgb);
                any = true;
            }
        }
        if any {
            note("Colours");
        }
    }
    // Editor.
    let e = &v["editor.settings"];
    if let Some(x) = b(&e["followPlayhead"]) {
        p.follow_playhead = x;
        note("Follow playhead");
    }
    if let Some(s) = e["playMode"].as_str() {
        p.play_mode = match s {
            "loop" => PlayMode::Loop,
            "skip" => PlayMode::Skip,
            _ => PlayMode::Normal,
        };
    }
    if let Some(x) = b(&e["snap"]["ruler"]).or(b(&e["snap"]["on"])) {
        p.snap_ruler = x;
        note("Snapping");
    }
    if let Some(x) = b(&e["stretchHandles"]) {
        p.stretch_handles = x;
    }
    if let Some(s) = e["timeFormat"].as_str() {
        p.time_format = match s {
            "bars" => TimeFormat::BarsBeats,
            "seconds" | "decimal" => TimeFormat::Seconds,
            "samples" => TimeFormat::Samples,
            "smpte" | "timecode" => TimeFormat::Timecode,
            _ => TimeFormat::MinSec,
        };
        note("Time display");
    }
    // New file and new session defaults.
    let na = &v["newAudio.defaults"];
    if na.is_object() {
        if let Some(x) = u(&na["sampleRate"]) {
            p.new_file.rate = x as u32;
        }
        if let Some(x) = u(&na["channels"]) {
            p.new_file.channels = x.clamp(1, 8) as usize;
        }
        if let Some(x) = u(&na["bitDepth"]) {
            p.new_file.bits = x as u16;
        }
        note("New Audio File defaults");
    }
    let ns = &v["newSession.defaults"];
    if ns.is_object() {
        if let Some(x) = u(&ns["sampleRate"]) {
            p.new_session.rate = x as u32;
        }
        if let Some(x) = u(&ns["bitDepth"]) {
            p.new_session.bits = x as u16;
        }
        if let Some(s) = ns["folder"].as_str().filter(|s| !s.trim().is_empty()) {
            p.new_session.folder = Some(PathBuf::from(s));
        }
        note("New Session defaults (folder, rate, bit depth)");
    }
    st.return_on_stop = st.prefs.space_returns;
    st.prefs.save();

    // Zoom and the wheel.
    let mut z = st.zoom;
    let mut zoom_changed = false;
    if let Some(s) = a["wheelAnchor"].as_str() {
        z.wheel_anchor = match s {
            "playhead" => crate::zoom::WheelAnchor::Playhead,
            _ => crate::zoom::WheelAnchor::Pointer,
        };
        zoom_changed = true;
    }
    if let Some(x) = b(&a["wheelZooms"]) {
        z.wheel_zooms_waveform = x;
        zoom_changed = true;
    }
    if let Some(x) = f(&a["zoomStep"]) {
        z.step_percent = (x as f32).clamp(1.0, 100.0);
        zoom_changed = true;
    }
    if zoom_changed {
        st.zoom = z;
        st.zoom.save();
        done.push("Zoom and wheel".into());
    }
    // Phase Wheel.
    let pw = &a["phaseWheel"];
    if pw.is_object() {
        use crate::phasewheel::Sig;
        let sig = |s: &str| match s {
            "right" => Sig::Right,
            "mid" => Sig::Mid,
            "side" => Sig::Side,
            _ => Sig::Left,
        };
        let ph = &mut st.phase;
        if let Some(s) = pw["a"].as_str() {
            ph.a = sig(s);
        }
        if let Some(s) = pw["b"].as_str() {
            ph.b = sig(s);
        }
        if let Some(x) = u(&pw["fps"]) {
            ph.fps = x.clamp(10, 240) as u32;
        }
        if let Some(x) = u(&pw["size"]) {
            ph.size = (x as usize).clamp(256, 65_536);
        }
        if let Some(x) = b(&pw["details"]) {
            ph.details = x;
        }
        if let Some(rgb) = hex(&cols["phaseLow"]) {
            ph.low = Some(rgb);
        }
        if let Some(rgb) = hex(&cols["phaseHigh"]) {
            ph.high = Some(rgb);
        }
        ph.save_now();
        done.push("Phase Wheel".into());
    }
    // Save Audio As and Export.
    let (mut ex, _) = crate::export::ExportPrefs::load_for(false);
    let mut ex_changed = false;
    let sa = &v["save-audio.format"];
    if sa.is_object() {
        if sa["kind"].as_str() == Some("flac") {
            ex.container = af2_io::export::Container::Flac;
        } else if sa["kind"].as_str() == Some("wav") {
            ex.container = af2_io::export::Container::Wav;
        }
        if let Some(x) = u(&sa["flacLevel"]) {
            ex.flac.flac_level = x.min(8) as u8;
        }
        ex_changed = true;
    }
    if let Some(s) = v["export.mixdown"]["folder"]
        .as_str()
        .or(v["save-audio.folder"].as_str())
        .filter(|s| !s.trim().is_empty())
    {
        ex.folder = Some(PathBuf::from(s));
        ex_changed = true;
    }
    if ex_changed {
        ex.save_for(false);
        done.push("Save Audio As and Export format and folder".into());
    }
    // Workspace.
    if let Some(k) = v["workspace.current"].as_str() {
        let w = crate::workspace::Workspace::from_key(k);
        if w != st.workspace {
            st.want_workspace = Some(w);
        }
        done.push(format!("Workspace ({})", w.name()));
    }
    st.live_metronome();
    done
}

/// Brings in Audiform 1's plug-in scan: every plug-in found, the extra
/// folders, the makers run on their own, and the scan stamps (so a rescan
/// only reads bundles that changed). Merged with what AudiForm 2 already
/// found. Returns (plug-ins added, folders added).
pub fn apply_plugins(dir: &Path) -> Result<(usize, usize), String> {
    let text = std::fs::read_to_string(dir.join("plugins.json")).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mgr = af2_vst3::live::manager();
    let (added, folders) = {
        let mut reg = mgr.registry();
        merge(&mut reg, &v)
    };
    mgr.save_registry()?;
    Ok((added, folders))
}

/// Merges an Audiform 1 scan (`v`) into `reg`.
pub fn merge(reg: &mut af2_vst3::registry::Registry, v: &Value) -> (usize, usize) {
    use af2_vst3::registry::{Failed, Found};
    let mut added = 0;
    for p in v["plugins"].as_array().into_iter().flatten() {
        let Ok(found) = serde_json::from_value::<Found>(p.clone()) else {
            continue;
        };
        if !reg
            .plugins
            .iter()
            .any(|x| x.info.cid == found.info.cid && x.bundle == found.bundle)
        {
            reg.plugins.push(found);
            added += 1;
        }
    }
    let mut folders = 0;
    for f in v["folders"].as_array().into_iter().flatten() {
        if let Some(s) = f.as_str() {
            let p = PathBuf::from(s);
            if !reg.folders.contains(&p) {
                reg.folders.push(p);
                folders += 1;
            }
        }
    }
    for f in v["skipped"].as_array().into_iter().flatten() {
        if let Some(s) = f.as_str() {
            let p = PathBuf::from(s);
            if !reg.skipped.contains(&p) {
                reg.skipped.push(p);
            }
        }
    }
    for f in v["failed"].as_array().into_iter().flatten() {
        if let Ok(x) = serde_json::from_value::<Failed>(f.clone()) {
            if !reg.failed.iter().any(|y| y.bundle == x.bundle) {
                reg.failed.push(x);
            }
        }
    }
    if let Some(m) = v["stamps"].as_object() {
        for (k, s) in m {
            if let Some(s) = s.as_u64() {
                reg.stamps.entry(PathBuf::from(k)).or_insert(s);
            }
        }
    }
    for m in v["separate_makers"].as_array().into_iter().flatten() {
        if let Some(s) = m.as_str() {
            let s = s.to_lowercase();
            if !reg.separate_makers.contains(&s) {
                reg.separate_makers.push(s);
            }
        }
    }
    reg.dedupe();
    (added, folders)
}

// ------------------------------------------------------------ the prompt

/// The import window's state.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub found: Found,
    pub settings: bool,
    pub plugins: bool,
    /// After Import: what happened.
    pub report: Option<Vec<String>>,
}

/// Opens the window if Audiform 1 is here (Settings button: always;
/// start-up: once).
pub fn open(st: &mut State) -> bool {
    match find() {
        Some(found) => {
            st.af1_prompt = Some(Prompt {
                settings: found.prefs.is_some(),
                plugins: found.plugins.is_some(),
                found,
                report: None,
            });
            true
        }
        None => {
            st.toast("Audiform 1's settings were not found on this computer.");
            false
        }
    }
}

/// At start-up: ask once, if there is anything to bring over.
pub fn ask_once(st: &mut State) {
    if st.prefs.af1_import_asked {
        return;
    }
    st.prefs.af1_import_asked = true;
    st.prefs.save();
    if find().is_some() {
        open(st);
    }
}

pub fn window(ctx: &egui::Context, st: &mut State) {
    let Some(mut pr) = st.af1_prompt.take() else {
        return;
    };
    let th = st.theme.clone();
    let mut keep = true;
    let mut go = false;
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "af1_import.rs", 0)), "Import from Audiform 1")
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| {
            ui.set_width(420.0);
            if let Some(rep) = &pr.report {
                ui.label(egui::RichText::new("Brought over:").strong());
                for line in rep {
                    ui.label(format!("• {line}"));
                }
                ui.add_space(8.0);
                if ui.button("  Done  ").clicked() {
                    keep = false;
                }
                return;
            }
            ui.label("Audiform 1 is installed on this computer. Bring its setup over to AudiForm 2?");
            ui.add_space(6.0);
            ui.add_enabled_ui(pr.found.prefs.is_some(), |ui| {
                ui.checkbox(&mut pr.settings, "Settings");
            });
            ui.label(
                egui::RichText::new("Audio devices and driver, autosave and backups, session media, metronome, zoom and wheel, snapping, time display, colours, Phase Wheel, new file and session defaults, export format and folder, workspace.")
                    .size(10.5)
                    .color(c(th.text_dim)),
            );
            ui.add_space(4.0);
            let plug_text = match pr.found.plugins {
                Some((n, f)) => format!("VST3 plug-in scan ({n} plug-ins, {f} extra folders)"),
                None => "VST3 plug-in scan (none found)".into(),
            };
            ui.add_enabled_ui(pr.found.plugins.is_some(), |ui| {
                ui.checkbox(&mut pr.plugins, plug_text);
            });
            ui.label(
                egui::RichText::new("Plug-ins appear in the Effects menus right away; a rescan only reads bundles that changed since.")
                    .size(10.5)
                    .color(c(th.text_dim)),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("From {}", pr.found.dir.display()))
                    .size(10.0)
                    .color(c(th.text_dim)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ok = egui::Button::new(
                    egui::RichText::new("  Import  ").strong().color(c(th.background)),
                )
                .fill(c(th.accent));
                if ui.add_enabled(pr.settings || pr.plugins, ok).clicked() {
                    go = true;
                }
                if ui.button("Not now").on_hover_text("Settings > General can import later").clicked() {
                    keep = false;
                }
            });
        });
    if go {
        let mut rep = Vec::new();
        if pr.settings {
            if let Some(v) = pr.found.prefs.clone() {
                rep.extend(apply_prefs(st, &v));
            }
        }
        if pr.plugins {
            match apply_plugins(&pr.found.dir) {
                Ok((n, f)) => rep.push(format!("{n} VST3 plug-ins and {f} plug-in folders")),
                Err(e) => rep.push(format!("Plug-in scan: could not read it ({e})")),
            }
        }
        if rep.is_empty() {
            rep.push("Nothing needed changing.".into());
        }
        pr.report = Some(rep);
    }
    if keep {
        st.af1_prompt = Some(pr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hex_colours_read() {
        assert_eq!(hex(&json!("#358c1d")), Some([0x35, 0x8c, 0x1d]));
        assert_eq!(hex(&json!("")), None);
        assert_eq!(hex(&json!(null)), None);
    }

    #[test]
    fn plugin_scans_merge_without_duplicates() {
        let v = json!({
            "schema_version": 4,
            "folders": ["C:\\Program Files\\VstPlugins"],
            "skipped": [],
            "plugins": [{"bundle": "C:\\X.vst3", "info": {"cid": "AB", "name": "X", "vendor": "Waves", "sub_categories": "Fx|EQ", "params": [], "latency": 0, "has_editor": true, "instrument": false, "sidechain": false}}],
            "failed": [],
            "stamps": {"C:\\X.vst3": 5},
            "separate_makers": ["Waves"]
        });
        let mut reg = af2_vst3::registry::Registry::default();
        assert_eq!(merge(&mut reg, &v), (1, 1));
        assert_eq!(merge(&mut reg, &v), (0, 0), "a second import adds nothing");
        assert_eq!(reg.plugins.len(), 1);
        assert_eq!(reg.separate_makers, vec!["waves".to_string()]);
        assert_eq!(reg.stamps.len(), 1);
    }

    #[test]
    fn real_audiform1_data_reads_if_present() {
        // Reads the real folder on a machine that has Audiform 1.
        if let Some(found) = find() {
            if let Some((n, _)) = found.plugins {
                let text = std::fs::read_to_string(found.dir.join("plugins.json")).unwrap();
                let v: Value = serde_json::from_str(&text).unwrap();
                let mut reg = af2_vst3::registry::Registry::default();
                let (added, _) = merge(&mut reg, &v);
                assert!(added > 0 && added <= n);
            }
        }
    }
}
