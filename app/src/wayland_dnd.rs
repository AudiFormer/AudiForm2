// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Files dragged in from the file manager on Wayland. winit only reports
//! dropped files on X11 and Windows, so on Wayland this listens itself: a
//! second event queue on the window's own display connection, with a data
//! device for the seat (as smithay-clipboard does for copy and paste). A
//! drag of `text/uri-list` over the main window is offered as a copy (never
//! a move: a file manager deletes the originals after a move) and read; its
//! files and position are handed to egui as hovered and dropped files, so
//! the same code takes them as on other systems.

use std::path::PathBuf;

/// Where a drag is, in the window's logical pixels.
type Pos = (f64, f64);

#[derive(Default)]
struct Shared {
    hover: Option<(Vec<PathBuf>, Pos)>,
    drops: Vec<Vec<PathBuf>>,
    /// The drag's last place, for `hwcursor::system_pointer`.
    last: Option<Pos>,
    /// Logical to physical pixels, as egui last saw it.
    scale: f32,
}

static SHARED: std::sync::Mutex<Shared> = std::sync::Mutex::new(Shared {
    hover: None,
    drops: Vec::new(),
    last: None,
    scale: 1.0,
});

fn shared() -> std::sync::MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts listening, when the window is a Wayland one.
pub fn install(cc: &eframe::CreationContext) {
    #[cfg(target_os = "linux")]
    imp::install(cc);
    #[cfg(not(target_os = "linux"))]
    let _ = cc;
}

/// Each frame, before anything reads dropped files: a drag over the window
/// becomes egui's hovered files, and a drop its dropped files.
pub fn frame(ctx: &egui::Context) {
    let mut s = shared();
    s.scale = ctx
        .input(|i| i.viewport().native_pixels_per_point)
        .unwrap_or_else(|| ctx.pixels_per_point());
    let hover = s.hover.as_ref().map(|(p, _)| p.clone());
    let drops = std::mem::take(&mut s.drops);
    drop(s);
    if hover.is_none() && drops.is_empty() {
        return;
    }
    ctx.input_mut(|i| {
        if let Some(paths) = hover {
            i.raw.hovered_files = paths
                .into_iter()
                .map(|p| egui::HoveredFile { path: Some(p), ..Default::default() })
                .collect();
        }
        for paths in drops {
            i.raw.dropped_files.extend(
                paths
                    .into_iter()
                    .map(|p| egui::DroppedFile { path: Some(p), ..Default::default() }),
            );
        }
    });
}

/// Where the last drag over the window was, in physical pixels (as
/// `hwcursor::system_pointer` gives it on Windows).
pub fn pointer() -> Option<(f32, f32)> {
    let s = shared();
    let (x, y) = s.last?;
    Some((x as f32 * s.scale, y as f32 * s.scale))
}

/// The local files in a `text/uri-list` (comments and other schemes left
/// out; `file://host/path` keeps the path).
pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            let path = &rest[rest.find('/')?..];
            Some(PathBuf::from(percent_decode(path)))
        })
        .collect()
}

#[cfg(unix)]
fn percent_decode(s: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    std::ffi::OsString::from_vec(out)
}

#[cfg(not(unix))]
fn percent_decode(s: &str) -> std::ffi::OsString {
    s.into()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{parse_uri_list, shared, Pos};
    use std::io::Read;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
    use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
    use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};

    const URI_LIST: &str = "text/uri-list";

    pub fn install(cc: &eframe::CreationContext) {
        use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
        let (Ok(d), Ok(w)) = (cc.display_handle(), cc.window_handle()) else {
            return;
        };
        let (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) = (d.as_raw(), w.as_raw()) else {
            return;
        };
        let display = d.display.as_ptr() as usize;
        let surface = w.surface.as_ptr() as usize;
        let ctx = cc.egui_ctx.clone();
        let _ = std::thread::Builder::new()
            .name("af2-wayland-dnd".into())
            .spawn(move || {
                if let Err(e) = run(display, surface, ctx) {
                    eprintln!("Wayland drag and drop is off: {e}");
                }
            });
    }

    fn run(display: usize, surface: usize, ctx: egui::Context) -> Result<(), String> {
        // SAFETY: winit's wl_display, which lives as long as the program.
        let backend = unsafe {
            wayland_backend::client::Backend::from_foreign_display(display as *mut _)
        };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let manager: WlDataDeviceManager =
            globals.bind(&qh, 3..=3, ()).map_err(|e| format!("no data device manager v3 ({e})"))?;
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("no seat ({e})"))?;
        let _device = manager.get_data_device(&seat, &qh, ());
        let mut st = State { conn: conn.clone(), surface, ctx, offer: None };
        loop {
            queue.blocking_dispatch(&mut st).map_err(|e| e.to_string())?;
        }
    }

    #[derive(Default)]
    struct OfferData {
        mimes: Mutex<Vec<String>>,
        action: Mutex<Option<DndAction>>,
    }

    /// The drag in progress over the window.
    struct Drag {
        offer: WlDataOffer,
        paths: Vec<PathBuf>,
        pos: Pos,
    }

    struct State {
        conn: Connection,
        /// The main window's wl_surface.
        surface: usize,
        ctx: egui::Context,
        offer: Option<Drag>,
    }

    impl State {
        fn publish(&self) {
            let mut s = shared();
            s.hover = self.offer.as_ref().map(|d| (d.paths.clone(), d.pos));
            if let Some(d) = &self.offer {
                s.last = Some(d.pos);
            }
            drop(s);
            self.ctx.request_repaint();
        }

        /// The offer's files, read through a socket pair (the source just
        /// writes to the descriptor, as to a pipe).
        fn read(&self, offer: &WlDataOffer) -> Vec<PathBuf> {
            use std::os::fd::AsFd;
            let Ok((mut r, w)) = std::os::unix::net::UnixStream::pair() else {
                return Vec::new();
            };
            offer.receive(URI_LIST.into(), w.as_fd());
            drop(w);
            if self.conn.flush().is_err() {
                return Vec::new();
            }
            let mut text = String::new();
            let _ = r.read_to_string(&mut text);
            parse_uri_list(&text)
        }

        fn end(&mut self) {
            if let Some(d) = self.offer.take() {
                d.offer.destroy();
            }
            shared().hover = None;
            self.ctx.request_repaint();
        }
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<WlSeat, ()> for State {
        fn event(_: &mut Self, _: &WlSeat, _: wayland_client::protocol::wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<WlDataDeviceManager, ()> for State {
        fn event(_: &mut Self, _: &WlDataDeviceManager, _: wayland_client::protocol::wl_data_device_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<WlDataOffer, OfferData> for State {
        fn event(_: &mut Self, _: &WlDataOffer, event: wl_data_offer::Event, data: &OfferData, _: &Connection, _: &QueueHandle<Self>) {
            match event {
                wl_data_offer::Event::Offer { mime_type } => {
                    data.mimes.lock().unwrap_or_else(|e| e.into_inner()).push(mime_type);
                }
                wl_data_offer::Event::Action { dnd_action: WEnum::Value(a) } => {
                    *data.action.lock().unwrap_or_else(|e| e.into_inner()) = Some(a);
                }
                _ => {}
            }
        }
    }

    impl Dispatch<WlDataDevice, ()> for State {
        fn event(st: &mut Self, _: &WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            match event {
                wl_data_device::Event::Enter { serial, surface, x, y, id } => {
                    st.end();
                    let Some(offer) = id else { return };
                    let has_uris = offer
                        .data::<OfferData>()
                        .is_some_and(|d| d.mimes.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|m| m == URI_LIST));
                    if surface.id().as_ptr() as usize != st.surface || !has_uris {
                        offer.accept(serial, None);
                        offer.destroy();
                        return;
                    }
                    offer.accept(serial, Some(URI_LIST.into()));
                    // A copy only: after a move the file manager deletes
                    // the originals.
                    offer.set_actions(DndAction::Copy, DndAction::Copy);
                    let paths = st.read(&offer);
                    st.offer = Some(Drag { offer, paths, pos: (x, y) });
                    st.publish();
                }
                wl_data_device::Event::Motion { x, y, .. } => {
                    if let Some(d) = &mut st.offer {
                        d.pos = (x, y);
                        st.publish();
                    }
                }
                wl_data_device::Event::Leave => st.end(),
                wl_data_device::Event::Drop => {
                    let Some(mut d) = st.offer.take() else { return };
                    if d.paths.is_empty() {
                        d.paths = st.read(&d.offer);
                    }
                    let copy = d
                        .offer
                        .data::<OfferData>()
                        .and_then(|o| *o.action.lock().unwrap_or_else(|e| e.into_inner()))
                        == Some(DndAction::Copy);
                    // finish() is a protocol error unless a copy (or move)
                    // was agreed, and a protocol error ends the window's
                    // whole connection.
                    if copy {
                        d.offer.finish();
                    }
                    d.offer.destroy();
                    let mut s = shared();
                    s.last = Some(d.pos);
                    s.hover = None;
                    if copy && !d.paths.is_empty() {
                        s.drops.push(d.paths);
                    }
                    drop(s);
                    st.ctx.request_repaint();
                }
                wl_data_device::Event::Selection { id: Some(offer) } => offer.destroy(),
                _ => {}
            }
        }

        event_created_child!(State, WlDataDevice, [
            wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, OfferData::default())
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_become_local_paths() {
        let text = "# from Dolphin\r\nfile:///home/me/Music/My%20Song.wav\r\nfile://host/tmp/a%25b.flac\r\nhttps://example.com/x.mp3\r\n\r\nfile:///caf%C3%A9.ogg\n";
        assert_eq!(
            parse_uri_list(text),
            vec![
                PathBuf::from("/home/me/Music/My Song.wav"),
                PathBuf::from("/tmp/a%b.flac"),
                PathBuf::from("/café.ogg"),
            ]
        );
        // A stray % is kept as it is.
        assert_eq!(parse_uri_list("file:///100%"), vec![PathBuf::from("/100%")]);
    }
}
