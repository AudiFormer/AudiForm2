// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Multitrack's right-click menus: on empty track space, on a clip
//! (then the track's own items), and on a clip's fade handle.

use af27_core::{ClipId, Send, TrackId, TrackKind};
use egui::RichText;

use crate::state::{Mode, State};

/// Where the menu was opened.
#[derive(Debug, Clone, Copy, Default)]
pub struct MenuAt {
    /// Timeline time under the pointer.
    pub t: f64,
    pub track: Option<usize>,
    pub clip: Option<ClipId>,
    /// On a fade handle: the clip, and true for the fade in.
    pub fade: Option<(ClipId, bool)>,
    /// Clicked in the lower half of the track (new tracks go below it).
    pub below: bool,
}

const SOON: &str = "Coming soon";

fn soon(ui: &mut egui::Ui, label: &str) {
    ui.add_enabled(false, egui::Button::new(label))
        .on_disabled_hover_text(SOON);
}

/// One undoable edit from a menu item.
fn edit(st: &mut State, label: &str, f: impl FnOnce(&mut af27_core::Session) -> bool) {
    let before = st.session.clone();
    if f(&mut st.session) {
        st.session_version += 1;
        st.commit_session(label, before);
    }
}

pub fn show(ui: &mut egui::Ui, st: &mut State, at: MenuAt) {
    ui.set_min_width(220.0);
    if let Some((clip, fade_in)) = at.fade {
        fade_menu(ui, st, clip, fade_in);
        return;
    }
    if let Some(clip) = at.clip {
        clip_items(ui, st, clip);
        ui.separator();
        if let Some(ti) = at.track {
            track_items(ui, st, ti, at.below);
        }
        return;
    }
    empty_items(ui, st, at);
}

fn fade_menu(ui: &mut egui::Ui, st: &mut State, clip: ClipId, fade_in: bool) {
    let Some((ti, ci)) = st.session.find_clip(clip) else {
        return;
    };
    let c = &st.session.tracks[ti].clips[ci];
    let skew = if fade_in {
        c.props.fade_in_skew
    } else {
        c.props.fade_out_skew
    };
    let set = |st: &mut State, v: Option<f32>, remove: bool| {
        edit(st, "Fade", |s| {
            let Some((ti, ci)) = s.find_clip(clip) else {
                return false;
            };
            let c = &mut s.tracks[ti].clips[ci];
            if remove {
                if fade_in {
                    c.fade_in = 0.0;
                } else {
                    c.fade_out = 0.0;
                }
            }
            if let Some(v) = v {
                if fade_in {
                    c.props.fade_in_skew = v;
                } else {
                    c.props.fade_out_skew = v;
                }
            }
            true
        })
    };
    if ui.radio(skew == 0.0, "Straight").clicked() {
        set(st, Some(0.0), false);
        ui.close_menu();
    }
    if ui.radio(skew != 0.0, "Curve").clicked() {
        set(st, Some(if skew == 0.0 { 50.0 } else { skew }), false);
        ui.close_menu();
    }
    ui.add_enabled(false, egui::Button::new(format!("Skew: {skew:+.0}")))
        .on_disabled_hover_text("Drag the fade handle up or down to bend the curve");
    if ui.button("Reset Skew").clicked() {
        set(st, Some(0.0), false);
        ui.close_menu();
    }
    ui.separator();
    let label = if fade_in {
        "Remove Fade In"
    } else {
        "Remove Fade Out"
    };
    if ui.button(label).clicked() {
        set(st, None, true);
        ui.close_menu();
    }
}

fn clip_items(ui: &mut egui::Ui, st: &mut State, clip: ClipId) {
    let Some((ti, ci)) = st.session.find_clip(clip) else {
        return;
    };
    let c = st.session.tracks[ti].clips[ci].clone();
    let cursor = st.session_cursor;
    let under = cursor > c.start && cursor < c.end();
    if ui
        .add_enabled(under, egui::Button::new("Split at Playhead"))
        .clicked()
    {
        edit(st, "Split", |s| s.split_clip(clip, cursor).is_some());
        ui.close_menu();
    }
    if ui
        .add_enabled(under, egui::Button::new("Trim Start to Playhead"))
        .clicked()
    {
        edit(st, "Trim", |s| s.trim_start_to(clip, cursor));
        ui.close_menu();
    }
    if ui
        .add_enabled(under, egui::Button::new("Trim End to Playhead"))
        .clicked()
    {
        edit(st, "Trim", |s| s.trim_end_to(clip, cursor));
        ui.close_menu();
    }
    ui.separator();
    let toggle = |st: &mut State, label: &str, f: fn(&mut af27_core::Clip)| {
        edit(st, label, |s| {
            let Some((ti, ci)) = s.find_clip(clip) else {
                return false;
            };
            f(&mut s.tracks[ti].clips[ci]);
            true
        });
    };
    if ui
        .button(if c.muted { "Unmute Clip" } else { "Mute Clip" })
        .clicked()
    {
        toggle(st, "Mute clip", |c| c.muted = !c.muted);
        ui.close_menu();
    }
    if ui
        .button(if c.props.locked {
            "Unlock Position"
        } else {
            "Lock Position"
        })
        .clicked()
    {
        toggle(st, "Lock clip", |c| c.props.locked = !c.props.locked);
        ui.close_menu();
    }
    if ui.button("Clip Properties...").clicked() {
        st.selected_clip = Some(clip);
        st.want_properties = true;
        ui.close_menu();
    }
    if ui.button("Clip Effects...").clicked() {
        st.selected_clip = Some(clip);
        st.rack_tab = crate::rack::RackTab::Clip;
        st.want_effects = true;
        ui.close_menu();
    }
    ui.menu_button("Clip Automation", |ui| {
        crate::clip_lanes::menu(ui, st, clip)
    });
    ui.separator();
    if ui
        .checkbox(&mut c.props.reversed.clone(), "Reverse")
        .clicked()
    {
        toggle(st, "Reverse clip", |c| c.props.reversed = !c.props.reversed);
        ui.close_menu();
    }
    if ui
        .checkbox(&mut c.props.inverted.clone(), "Invert Polarity")
        .clicked()
    {
        toggle(st, "Invert clip", |c| c.props.inverted = !c.props.inverted);
        ui.close_menu();
    }
    ui.separator();
    if ui.button("Stretch and Pitch...").clicked() {
        st.selected_clip = Some(clip);
        st.want_properties = true;
        ui.close_menu();
    }
    if ui
        .add_enabled(c.props.stretch.is_on(), egui::Button::new("Remove Stretch"))
        .clicked()
    {
        edit(st, "Remove stretch", |s| {
            let Some((ti, ci)) = s.find_clip(clip) else {
                return false;
            };
            let c = &mut s.tracks[ti].clips[ci];
            c.length /= c.props.stretch.applied_ratio();
            c.props.stretch = Default::default();
            true
        });
        ui.close_menu();
    }
    let fade_len = (c.length * 0.25).min(1.0);
    if c.fade_in > 0.0 {
        if ui.button("Remove Fade In").clicked() {
            toggle(st, "Fade in", |c| c.fade_in = 0.0);
            ui.close_menu();
        }
    } else if ui.button("Fade In").clicked() {
        edit(st, "Fade in", |s| {
            let Some((ti, ci)) = s.find_clip(clip) else {
                return false;
            };
            s.tracks[ti].clips[ci].fade_in = fade_len;
            true
        });
        ui.close_menu();
    }
    if c.fade_out > 0.0 {
        if ui.button("Remove Fade Out").clicked() {
            toggle(st, "Fade out", |c| c.fade_out = 0.0);
            ui.close_menu();
        }
    } else if ui.button("Fade Out").clicked() {
        edit(st, "Fade out", |s| {
            let Some((ti, ci)) = s.find_clip(clip) else {
                return false;
            };
            s.tracks[ti].clips[ci].fade_out = fade_len;
            true
        });
        ui.close_menu();
    }
    ui.separator();
    soon(ui, "Group Clips");
    soon(ui, "Separate Stems");
    ui.separator();
    if ui.button("Delete Clip").clicked() {
        st.selected_clip = Some(clip);
        st.delete_clip();
        ui.close_menu();
    }
    if ui.button("Ripple Delete Clip").clicked() {
        edit(st, "Ripple delete", |s| {
            s.ripple_delete_clips(&[clip]);
            true
        });
        st.selected_clip = None;
        ui.close_menu();
    }
}

/// Adds a track of `kind`: above or below the track the menu was opened
/// on (by which half was clicked), else at the end.
fn add_at(st: &mut State, kind: TrackKind, at: Option<(usize, bool)>) {
    let id = st.add_track_kind(kind);
    if let Some((ti, below)) = at {
        // One undo step: the add's snapshot is from before the add.
        st.session.move_track(id, ti + below as usize);
        st.session_version += 1;
    }
}

fn add_track_items(ui: &mut egui::Ui, st: &mut State, at: Option<(usize, bool)>) {
    let place = match at {
        Some((_, true)) => " Below",
        Some((_, false)) => " Above",
        None => "",
    };
    for (label, kind) in [
        ("Add Mono Audio Track", TrackKind::Audio),
        ("Add Stereo Audio Track", TrackKind::Audio),
        ("Add MIDI Track", TrackKind::Midi),
        ("Add Bus Track", TrackKind::Bus),
    ] {
        if ui.button(format!("{label}{place}")).clicked() {
            add_at(st, kind, at);
            ui.close_menu();
        }
    }
}

/// A track setting a menu item switches.
type Flag = fn(&mut af27_core::Track) -> &mut bool;

fn track_items(ui: &mut egui::Ui, st: &mut State, ti: usize, below: bool) {
    let Some(t) = st.session.tracks.get(ti).cloned() else {
        return;
    };
    let me = t.id;
    ui.label(RichText::new(&t.name).small().strong());
    if ui.button("Duplicate Track").clicked() {
        edit(st, "Duplicate track", |s| s.duplicate_track(me).is_some());
        ui.close_menu();
    }
    if t.kind != TrackKind::Bus && ui.button("Bounce Track to New Track").clicked() {
        st.selected_track = Some(me);
        crate::export::bounce_track(st);
        ui.close_menu();
    }
    if ui
        .button(if t.kind == TrackKind::Bus {
            "Delete Bus"
        } else {
            "Delete Track"
        })
        .clicked()
    {
        edit(st, "Delete track", |s| s.delete_track(me));
        ui.close_menu();
    }
    ui.separator();
    add_track_items(ui, st, Some((ti, below)));
    ui.separator();
    let buses: Vec<(TrackId, String, bool)> = st
        .session
        .tracks
        .iter()
        .filter(|b| b.kind == TrackKind::Bus && b.id != me)
        .map(|b| (b.id, b.name.clone(), st.session.can_route(me, b.id)))
        .collect();
    ui.menu_button("Output To", |ui| {
        if ui.radio(t.output.is_none(), "Master").clicked() {
            edit(st, "Output", |s| {
                s.track_mut(me).map(|t| t.output = None).is_some()
            });
            ui.close_menu();
        }
        for (id, name, ok) in &buses {
            let r = ui.add_enabled(*ok, egui::RadioButton::new(t.output == Some(*id), name));
            if r.clicked() {
                let id = *id;
                edit(st, "Output", |s| {
                    s.track_mut(me).map(|t| t.output = Some(id)).is_some()
                });
                ui.close_menu();
            }
        }
    });
    ui.menu_button("Add Send To", |ui| {
        if buses.is_empty() {
            ui.label("Add a bus track first.");
        }
        for (id, name, ok) in &buses {
            let has = t.sends.iter().any(|s| s.bus == *id);
            if ui
                .add_enabled(*ok && !has, egui::Button::new(name))
                .clicked()
            {
                let id = *id;
                edit(st, "Add send", |s| {
                    s.track_mut(me)
                        .map(|t| {
                            t.sends.push(Send {
                                bus: id,
                                level_db: 0.0,
                                pre_fader: false,
                                pan: 0.0,
                                enabled: true,
                                ext_id: None,
                            })
                        })
                        .is_some()
                });
                ui.close_menu();
            }
        }
    });
    ui.separator();
    let toggles: [(&str, bool, Flag, &str); 3] = [
        (
            "Mono",
            t.mono,
            |t| &mut t.mono,
            "Fold left and right together after the effects",
        ),
        (
            "Invert Polarity",
            t.polarity,
            |t| &mut t.polarity,
            "Flip the track's polarity",
        ),
        (
            "Solo Safe",
            t.solo_safe,
            |t| &mut t.solo_safe,
            "Stay heard while other tracks are soloed (a shared reverb bus)",
        ),
    ];
    for (label, on, field, tip) in toggles {
        if label == "Invert Polarity" && t.kind == TrackKind::Bus {
            continue;
        }
        if ui
            .add(egui::SelectableLabel::new(on, label))
            .on_hover_text(tip)
            .clicked()
        {
            edit(st, label, |s| {
                s.track_mut(me)
                    .map(|t| {
                        let f = field(t);
                        *f = !*f;
                    })
                    .is_some()
            });
            ui.close_menu();
        }
    }
    ui.menu_button("Automation", |ui| {
        for m in af27_core::automation::AutomationMode::ALL {
            if ui.radio(t.automation == m, m.label()).clicked() {
                edit(st, "Automation mode", |s| {
                    s.track_mut(me).map(|t| t.automation = m).is_some()
                });
                ui.close_menu();
            }
        }
        ui.separator();
        if ui
            .add(egui::SelectableLabel::new(t.show_lanes, "Show Lanes"))
            .clicked()
        {
            edit(st, "Show lanes", |s| {
                s.track_mut(me)
                    .map(|t| t.show_lanes = !t.show_lanes)
                    .is_some()
            });
            ui.close_menu();
        }
        ui.menu_button("Add Lane", |ui| crate::mixing::lanes_menu(ui, st, ti));
        if !t.lanes.is_empty() {
            ui.menu_button("Remove Lane", |ui| {
                for (k, l) in t.lanes.iter().enumerate() {
                    if ui.button(l.target.label()).clicked() {
                        edit(st, "Remove lane", |s| {
                            s.track_mut(me).map(|t| t.lanes.remove(k)).is_some()
                        });
                        ui.close_menu();
                    }
                }
            });
        }
    });
}

fn empty_items(ui: &mut egui::Ui, st: &mut State, at: MenuAt) {
    let sel = st.session_sel;
    let sel_track = st.selected_track;
    soon(ui, "Paste");
    ui.menu_button("Ripple Delete", |ui| {
        if ui
            .add_enabled(
                st.selected_clip.is_some(),
                egui::Button::new("Selected Clips"),
            )
            .clicked()
        {
            if let Some(c) = st.selected_clip.take() {
                edit(st, "Ripple delete", |s| {
                    s.ripple_delete_clips(&[c]);
                    true
                });
            }
            ui.close_menu();
        }
        if ui
            .add_enabled(
                sel.is_some() && sel_track.is_some(),
                egui::Button::new("Time Selection in Selected Track"),
            )
            .clicked()
        {
            if let (Some((a, b)), Some(t)) = (sel, sel_track) {
                edit(st, "Ripple delete", |s| {
                    s.ripple_delete_time(a, b, Some(&[t]));
                    true
                });
                st.session_sel = None;
            }
            ui.close_menu();
        }
        if ui
            .add_enabled(
                sel.is_some(),
                egui::Button::new("Time Selection in All Tracks"),
            )
            .clicked()
        {
            if let Some((a, b)) = sel {
                edit(st, "Ripple delete", |s| {
                    s.ripple_delete_time(a, b, None);
                    true
                });
                st.session_sel = None;
            }
            ui.close_menu();
        }
        let track = at
            .track
            .and_then(|i| st.session.tracks.get(i))
            .map(|t| t.id);
        if ui
            .add_enabled(track.is_some(), egui::Button::new("Gap Between Clips"))
            .clicked()
        {
            if let Some(t) = track {
                edit(st, "Ripple delete gap", |s| s.ripple_delete_gap(t, at.t));
            }
            ui.close_menu();
        }
    });
    ui.menu_button("Select", |ui| {
        if ui.button("Select All Time").clicked() {
            let d = st.session.duration();
            if d > 0.0 {
                st.session_sel = Some((0.0, d));
            }
            ui.close_menu();
        }
        if ui.button("Select Current View Time").clicked() {
            let v = st.session_view;
            st.session_sel = Some((v.start.max(0.0), v.start + v.len));
            ui.close_menu();
        }
        if ui
            .add_enabled(
                st.selected_clip.is_some(),
                egui::Button::new("Select Time Around Selected Clip"),
            )
            .clicked()
        {
            if let Some((ti, ci)) = st.selected_clip.and_then(|c| st.session.find_clip(c)) {
                let c = &st.session.tracks[ti].clips[ci];
                st.session_sel = Some((c.start, c.end()));
            }
            ui.close_menu();
        }
        if ui
            .add_enabled(
                sel_track.is_some(),
                egui::Button::new("Next Clip in Selected Track"),
            )
            .clicked()
        {
            if let Some(t) = sel_track.and_then(|t| st.session.track_index(t)) {
                let cur = st.session_cursor;
                let next = st.session.tracks[t]
                    .clips
                    .iter()
                    .filter(|c| c.start > cur + 1e-6)
                    .min_by(|a, b| a.start.total_cmp(&b.start))
                    .map(|c| (c.id, c.start));
                if let Some((id, start)) = next {
                    st.selected_clip = Some(id);
                    st.set_cursor(start);
                }
            }
            ui.close_menu();
        }
        if ui
            .add_enabled(sel.is_some(), egui::Button::new("Clear Time Selection"))
            .clicked()
        {
            st.session_sel = None;
            ui.close_menu();
        }
    });
    ui.menu_button("Groups", |ui| {
        soon(ui, "Group Clips");
        soon(ui, "Ungroup Selected Clips");
    });
    ui.menu_button("Track", |ui| {
        add_track_items(ui, st, at.track.map(|t| (t, at.below)));
        ui.separator();
        if ui.button("Delete Empty Tracks").clicked() {
            edit(st, "Delete empty tracks", |s| s.delete_empty_tracks() > 0);
            ui.close_menu();
        }
        ui.separator();
        if ui
            .add_enabled(
                sel_track.is_some(),
                egui::Button::new("Delete Selected Track"),
            )
            .clicked()
        {
            if let Some(t) = sel_track {
                edit(st, "Delete track", |s| s.delete_track(t));
                st.selected_track = None;
            }
            ui.close_menu();
        }
        if ui
            .add_enabled(
                sel_track.is_some(),
                egui::Button::new("Duplicate Selected Track"),
            )
            .clicked()
        {
            if let Some(t) = sel_track {
                edit(st, "Duplicate track", |s| s.duplicate_track(t).is_some());
            }
            ui.close_menu();
        }
    });
    ui.menu_button("Insert", |ui| {
        if ui.button("Files at Playhead...").clicked() {
            ui.close_menu();
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter("Audio", af27_io::EXTENSIONS)
                .pick_files()
            {
                let track = at
                    .track
                    .and_then(|i| st.session.tracks.get(i))
                    .map(|t| t.id)
                    .or(sel_track);
                let cursor = st.session_cursor;
                st.import_paths(
                    paths,
                    crate::sessions::Layout::Drop {
                        track,
                        at: cursor,
                        alt: false,
                    },
                );
            }
        }
        if ui.button("Files as Tracks...").clicked() {
            ui.close_menu();
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter("Audio", af27_io::EXTENSIONS)
                .pick_files()
            {
                st.import_paths(paths, crate::sessions::Layout::AsTracks);
            }
        }
        soon(ui, "Silence...");
    });
    ui.menu_button("Marker", |ui| {
        if ui.button("Add Cue Marker at Playhead").clicked() {
            st.add_marker();
            ui.close_menu();
        }
        if ui.button("Add Cue Marker Here").clicked() {
            st.add_marker_at(at.t);
            ui.close_menu();
        }
    });
    ui.separator();
    ui.menu_button("Mixdown Session to New File", |ui| {
        if ui.button("Entire Session").clicked() {
            mixdown_new(st, None);
            ui.close_menu();
        }
        if ui
            .add_enabled(sel.is_some(), egui::Button::new("Time Selection"))
            .clicked()
        {
            mixdown_new(st, sel);
            ui.close_menu();
        }
    });
    ui.menu_button("Export Mixdown", |ui| {
        if ui.button("Entire Session...").clicked() {
            crate::export::open_mixdown(st);
            ui.close_menu();
        }
    });
    ui.menu_button("Bounce to New Track", |ui| {
        if ui
            .add_enabled(sel_track.is_some(), egui::Button::new("Selected Track"))
            .clicked()
        {
            crate::export::bounce_track(st);
            ui.close_menu();
        }
    });
    ui.separator();
    if ui.button("Session Properties...").clicked() {
        st.sessions.props = Some(crate::sessions::PropsDraft {
            name: st.session.name.clone(),
            rate: st.session.rate,
        });
        ui.close_menu();
    }
}

/// Mixes the session (or a part of it) into a new file in the Waveform
/// editor.
fn mixdown_new(st: &mut State, range: Option<(f64, f64)>) {
    let (a, b) = range.unwrap_or((0.0, st.session.duration()));
    if b <= a {
        st.toast("Nothing to mix down yet");
        return;
    }
    let buf = st.render_mix(a, b, false);
    let name = format!("{} Mixdown", st.session.name);
    st.add_doc(name, None, buf);
    st.mode = Mode::Waveform;
}
