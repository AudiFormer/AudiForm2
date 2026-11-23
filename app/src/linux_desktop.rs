// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! AudiForm 2 on the Linux desktop: the theme's icon in the taskbar and
//! title bar, and desktop notifications.
//!
//! Reworked from b1g-b8lls's pull request #2 ("Linux desktop: the theme's
//! icon in the taskbar and title bar, and desktop notifications"), whose
//! two finds are kept: a name of its own for every theme's icon, and the
//! notifications themselves. Thanks to them for it, and for testing on
//! Plasma.
//!
//! **The theme's icon.** On Wayland a window can't carry its own icon; the
//! desktop shows the one its desktop entry names. When the AppImage's menu
//! entry (`desktop_entry.rs`) is ours, the theme's icon is written into
//! `~/.local/share/icons/hicolor` at 16 to 256 px and the entry's `Icon=`
//! names it. Every theme's icon gets a name of its own
//! (`io.github.audiformer.AudiForm2-theme-pastel`, or `-theme-` and a
//! short hash of its colours), since Plasma keeps an icon it has loaded by
//! name for as long as it runs: new pixels under an old name would never
//! show. Only files with those names are ever written or removed. The
//! rendering and writing run on a thread of their own; the taskbar follows
//! at once, the title bar (read once, when the window opens) next start.
//! Elsewhere (X11, Windows) the window's own icon is set by `app_icon.rs`.
//!
//! Preferences > General > Desktop turns this off, which also takes out
//! the menu entry and every icon written here. `AF2_NO_DESKTOP_ENTRY` and
//! self-screenshot runs (`AF2_SCREENSHOT`) write nothing either.
//!
//! **Notifications.** Errors, and long jobs (an export, a bounce, an
//! archive, separating stems) finishing, go out through
//! `org.freedesktop.Notifications` while the window isn't focused; plain
//! status messages ("Saved ...") never do. A note within 4 s of the last
//! replaces it, so a burst shows as one. On by default, since that is
//! exactly what you'd otherwise miss when working in another window, and
//! nothing is sent while AudiForm 2 is in front; Preferences > General >
//! Desktop turns it off.
//!
//! Nothing here touches the audio thread, and nothing waits on the UI
//! thread. Every public function is a no-op off Linux.

use crate::state::State;

/// Once a frame: follows the theme's icon, the Preferences, the window's
/// focus, and sends new errors on as notifications.
pub fn frame(ctx: &egui::Context, st: &State) {
    #[cfg(target_os = "linux")]
    imp::frame(ctx, st);
    #[cfg(not(target_os = "linux"))]
    let _ = (ctx, st);
}

/// A long job finished with `text` (already shown as a message): sent as
/// a notification if the window isn't focused.
pub fn job_done(text: &str) {
    #[cfg(target_os = "linux")]
    imp::job_done(text);
    #[cfg(not(target_os = "linux"))]
    let _ = text;
}

/// Whether Preferences allow writing to the user's desktop folders.
pub fn integration_allowed(st: &State) -> bool {
    #[cfg(target_os = "linux")]
    {
        st.prefs.desktop_integration && imp::may_write()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = st;
        false
    }
}

/// The menu entry was just written: write the theme's icon for it.
pub fn entry_added() {
    #[cfg(target_os = "linux")]
    imp::resync();
}

/// The menu entry was removed: take every theme icon written here with it.
pub fn entry_removed() {
    #[cfg(target_os = "linux")]
    imp::clear_all_in_background();
}

#[cfg(target_os = "linux")]
pub(crate) mod imp {
    use crate::app_icon::{self, Picture};
    use crate::state::State;
    use crate::theme::Theme;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    pub const ID: &str = "io.github.audiformer.AudiForm2";
    /// Every icon written here is named this and then a picture's name or
    /// eight hex digits.
    const PREFIX: &str = "io.github.audiformer.AudiForm2-theme-";
    /// The sizes the hicolor theme has folders for.
    pub const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

    /// Writing to the user's desktop folders is off for self-screenshot
    /// runs and while AF2_NO_DESKTOP_ENTRY is set.
    pub fn may_write() -> bool {
        std::env::var_os("AF2_SCREENSHOT").is_none()
            && std::env::var_os("AF2_NO_DESKTOP_ENTRY").is_none()
    }

    fn data_home() -> Option<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }

    // ------------------------------------------------------------ icons

    /// The installed name of the theme's icon.
    pub fn icon_name(th: &Theme) -> String {
        use sha2::{Digest, Sha256};
        let k = app_icon::key(th);
        match k.strip_prefix("picture:") {
            Some(p) => format!("{PREFIX}{p}"),
            None => {
                let h = Sha256::digest(k.as_bytes());
                let hex: String = h[..4].iter().map(|b| format!("{b:02x}")).collect();
                format!("{PREFIX}{hex}")
            }
        }
    }

    /// Whether `file` is an icon this module wrote: the prefix, then a
    /// picture's name or exactly eight lowercase hex digits, then `.png`.
    pub fn is_ours(file: &str) -> bool {
        let Some(rest) = file.strip_prefix(PREFIX).and_then(|r| r.strip_suffix(".png")) else {
            return false;
        };
        app_icon::picture_names().any(|n| n == rest)
            || (rest.len() == 8 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    }

    fn size_dir(home: &Path, size: u32) -> PathBuf {
        home.join("icons/hicolor").join(format!("{size}x{size}")).join("apps")
    }

    /// Writes `bytes` through a temporary file, so nothing reads half of it.
    fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("png.af2-tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }

    fn png(p: &Picture) -> Option<Vec<u8>> {
        let img = image::RgbaImage::from_raw(p.size, p.size, p.rgba.clone())?;
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?;
        Some(out)
    }

    /// Removes this module's icons from `dir`, all but `keep` (a file name).
    pub fn clear_ours(dir: &Path, keep: Option<&str>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let f = e.file_name();
            let Some(f) = f.to_str() else { continue };
            if is_ours(f) && Some(f) != keep {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// Icon caches look at the theme folder's time to see a change.
    fn touch_theme(home: &Path) {
        if let Ok(f) = std::fs::File::open(home.join("icons/hicolor")) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    }

    /// The newest request; an older one still being written gives way.
    static GEN: AtomicU64 = AtomicU64::new(0);
    /// One writer at a time.
    static LOCK: Mutex<()> = Mutex::new(());

    /// Writes the theme's icon at every size (those not there yet), points
    /// our entry at it, and clears our older icons. Does nothing while
    /// there is no entry of ours to point.
    pub fn write_icons(home: &Path, th: &Theme, gen: u64) {
        let _one = LOCK.lock();
        if !crate::desktop_entry::installed() {
            return;
        }
        let name = icon_name(th);
        let file = format!("{name}.png");
        for size in SIZES {
            if GEN.load(Ordering::Acquire) != gen {
                return;
            }
            let path = size_dir(home, size).join(&file);
            if path.is_file() {
                continue;
            }
            let Some(bytes) = app_icon::build(th, size).and_then(|p| png(&p)) else {
                return;
            };
            if write_atomic(&path, &bytes).is_err() {
                return;
            }
        }
        if GEN.load(Ordering::Acquire) != gen {
            return;
        }
        if crate::desktop_entry::set_icon(&name).is_ok() {
            for size in SIZES {
                clear_ours(&size_dir(home, size), Some(&file));
            }
            touch_theme(home);
        }
    }

    /// Takes out every icon this module wrote.
    pub fn clear_all() {
        let Some(home) = data_home() else { return };
        let _one = LOCK.lock();
        for size in SIZES {
            clear_ours(&size_dir(&home, size), None);
        }
        touch_theme(&home);
    }

    pub fn clear_all_in_background() {
        GEN.fetch_add(1, Ordering::AcqRel);
        let _ = std::thread::Builder::new()
            .name("desktop-icons".into())
            .spawn(clear_all);
    }

    // -------------------------------------------------------- per frame

    /// The window has the keyboard focus (as of the last frame).
    static FOCUSED: AtomicBool = AtomicBool::new(true);
    /// Preferences > General > Desktop's notifications setting.
    static NOTIFY: AtomicBool = AtomicBool::new(false);
    /// The next frame writes the icon again even if the theme's is the same.
    static RESYNC: AtomicBool = AtomicBool::new(false);

    pub fn resync() {
        RESYNC.store(true, Ordering::Release);
    }

    #[derive(Default)]
    struct Seen {
        /// The icon last asked for (written or not: a failed one is not
        /// tried again every frame).
        icon: Option<String>,
        /// The desktop integration setting as of the last frame.
        integration: Option<bool>,
        /// The newest message already looked at.
        toast: Option<Instant>,
    }

    thread_local! {
        static SEEN: std::cell::RefCell<Seen> = std::cell::RefCell::new(Seen::default());
    }

    pub fn frame(ctx: &egui::Context, st: &State) {
        FOCUSED.store(
            ctx.input(|i| i.viewport().focused.unwrap_or(true)),
            Ordering::Release,
        );
        NOTIFY.store(st.prefs.desktop_notifications, Ordering::Release);
        SEEN.with(|s| {
            let mut s = s.borrow_mut();
            icons(&mut s, st);
            errors(&mut s, st);
        });
    }

    fn icons(s: &mut Seen, st: &State) {
        if !may_write() {
            return;
        }
        let on = st.prefs.desktop_integration;
        // Turned off in Preferences: take out what was written.
        if s.integration.replace(on) == Some(true) && !on {
            s.icon = None;
            GEN.fetch_add(1, Ordering::AcqRel);
            let _ = std::thread::Builder::new()
                .name("desktop-icons".into())
                .spawn(|| {
                    let _ = crate::desktop_entry::remove();
                    clear_all();
                });
            return;
        }
        if !on {
            return;
        }
        let key = app_icon::key(&st.theme);
        if !RESYNC.swap(false, Ordering::AcqRel) && s.icon.as_deref() == Some(key.as_str()) {
            return;
        }
        if crate::desktop_entry::appimage().is_none() {
            s.icon = Some(key);
            return;
        }
        s.icon = Some(key);
        let Some(home) = data_home() else { return };
        let gen = GEN.fetch_add(1, Ordering::AcqRel) + 1;
        let th = st.theme.clone();
        let _ = std::thread::Builder::new()
            .name("desktop-icons".into())
            .spawn(move || write_icons(&home, &th, gen));
    }

    /// Errors that came up since the last frame, while away.
    fn errors(s: &mut Seen, st: &State) {
        let seen = *s.toast.get_or_insert_with(Instant::now);
        let newest = st.toasts.iter().map(|t| t.at).max().unwrap_or(seen).max(seen);
        s.toast = Some(newest);
        if !away() {
            return;
        }
        for t in st.toasts.iter().filter(|t| t.error && t.at > seen) {
            send(Note { body: t.text.clone(), error: true });
        }
    }

    fn away() -> bool {
        NOTIFY.load(Ordering::Acquire) && !FOCUSED.load(Ordering::Acquire)
    }

    pub fn job_done(text: &str) {
        if away() {
            send(Note { body: text.to_string(), error: false });
        }
    }

    // ---------------------------------------------------- notifications

    struct Note {
        body: String,
        error: bool,
    }

    /// A note within this long of the last replaces it.
    const REPLACE_WITHIN: Duration = Duration::from_secs(4);

    fn send(note: Note) {
        static TX: OnceLock<Option<Mutex<mpsc::Sender<Note>>>> = OnceLock::new();
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Note>();
            std::thread::Builder::new()
                .name("notifications".into())
                .spawn(move || pollster::block_on(send_all(rx)))
                .ok()?;
            Some(Mutex::new(tx))
        });
        if let Some(tx) = tx.as_ref().and_then(|t| t.lock().ok()) {
            let _ = tx.send(note);
        }
    }

    async fn send_all(rx: mpsc::Receiver<Note>) {
        let Ok(conn) = zbus::Connection::session().await else {
            return;
        };
        let mut last: Option<(u32, Instant)> = None;
        while let Ok(note) = rx.recv() {
            let replaces = match last {
                Some((id, at)) if at.elapsed() < REPLACE_WITHIN => id,
                _ => 0,
            };
            if let Some(id) = notify(&conn, &note, replaces).await {
                last = Some((id, Instant::now()));
            }
        }
    }

    /// The body as notification markup: servers that render it would
    /// otherwise break on `Tom & Jerry.wav`.
    pub fn escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                _ => out.push(ch),
            }
        }
        out
    }

    async fn notify(conn: &zbus::Connection, note: &Note, replaces: u32) -> Option<u32> {
        use std::collections::HashMap;
        use zbus::zvariant::Value;
        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("desktop-entry", Value::from(ID));
        hints.insert("urgency", Value::U8(if note.error { 2 } else { 1 }));
        let summary = if note.error { "AudiForm 2: something went wrong" } else { "AudiForm 2" };
        let reply = conn
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "AudiForm 2",
                    replaces,
                    ID,
                    summary,
                    escape(&note.body),
                    Vec::<&str>::new(),
                    hints,
                    -1i32,
                ),
            )
            .await
            .ok()?;
        reply.body().deserialize::<u32>().ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_icon_has_a_name_of_its_own() {
            let mut th = Theme::zero_point();
            let plain = icon_name(&th);
            assert!(plain.starts_with(PREFIX) && plain.len() == PREFIX.len() + 8);
            assert_eq!(icon_name(&Theme::zero_point()), plain, "stable");
            assert_ne!(icon_name(&Theme::solar_flare()), plain);
            th.icon = "pastel".into();
            assert_eq!(icon_name(&th), format!("{PREFIX}pastel"));
            assert!(is_ours(&format!("{}.png", icon_name(&th))));
            assert!(is_ours(&format!("{plain}.png")));
        }

        #[test]
        fn only_our_own_icons_are_cleared() {
            let dir = std::env::temp_dir().join(format!("af2-icons-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let keep = format!("{PREFIX}pastel.png");
            for f in [
                "io.github.audiformer.AudiForm2.png".to_string(),
                keep.clone(),
                format!("{PREFIX}1a2b3c4d.png"),
                format!("{PREFIX}undercurrent.png"),
                format!("{PREFIX}mine.png"),
                format!("{PREFIX}1A2B3C4D.png"),
                format!("{PREFIX}1a2b3c4d5.png"),
                "audiform2-1a2b3c4d.png".to_string(),
                "other.png".to_string(),
            ] {
                std::fs::write(dir.join(f), b"x").unwrap();
            }
            clear_ours(&dir, Some(&keep));
            let mut left: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            left.sort();
            let mut want = vec![
                "audiform2-1a2b3c4d.png".to_string(),
                "io.github.audiformer.AudiForm2.png".to_string(),
                format!("{PREFIX}1A2B3C4D.png"),
                format!("{PREFIX}1a2b3c4d5.png"),
                format!("{PREFIX}mine.png"),
                keep,
                "other.png".to_string(),
            ];
            want.sort();
            assert_eq!(left, want);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn icons_build_at_every_hicolor_size() {
            for name in std::iter::once("").chain(app_icon::picture_names()) {
                let mut th = Theme::zero_point();
                th.icon = name.into();
                for size in SIZES {
                    let p = app_icon::build(&th, size).unwrap();
                    assert_eq!(p.size, size);
                    let back = image::load_from_memory(&png(&p).unwrap()).unwrap();
                    assert_eq!((back.width(), back.height()), (size, size));
                }
            }
        }

        #[test]
        fn bodies_are_markup_escaped() {
            assert_eq!(escape("Tom & Jerry <1>.wav"), "Tom &amp; Jerry &lt;1&gt;.wav");
            assert_eq!(escape("plain"), "plain");
        }

        /// Writes an entry and the theme's icons into a scratch
        /// XDG_DATA_HOME, changes theme, then cleans up. Ignored since it
        /// sets the environment: `cargo test -- --ignored --exact
        /// linux_desktop::imp::tests::icons_follow_the_theme_and_clean_up`.
        #[test]
        #[ignore]
        fn icons_follow_the_theme_and_clean_up() {
            let home = std::env::temp_dir().join(format!("af2-xdg-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::env::set_var("XDG_DATA_HOME", &home);
            std::env::set_var("XDG_DATA_DIRS", home.join("none"));
            crate::desktop_entry::add(Path::new("/opt/AudiForm2.AppImage")).unwrap();
            let entry = home.join("applications").join(format!("{ID}.desktop"));
            let mut th = Theme::zero_point();
            for pic in ["pastel", ""] {
                th.icon = pic.into();
                let gen = GEN.fetch_add(1, Ordering::AcqRel) + 1;
                write_icons(&home, &th, gen);
                let name = icon_name(&th);
                let text = std::fs::read_to_string(&entry).unwrap();
                assert!(text.contains(&format!("\nIcon={name}\n")), "{text}");
                for size in SIZES {
                    let dir = size_dir(&home, size);
                    let files: Vec<String> = std::fs::read_dir(&dir)
                        .unwrap()
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|f| f.starts_with(PREFIX))
                        .collect();
                    assert_eq!(files, [format!("{name}.png")], "{size}");
                }
            }
            crate::desktop_entry::remove().unwrap();
            clear_all();
            assert!(!entry.exists());
            for size in SIZES {
                let left = std::fs::read_dir(size_dir(&home, size))
                    .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(PREFIX)).count())
                    .unwrap_or(0);
                assert_eq!(left, 0);
            }
            let _ = std::fs::remove_dir_all(&home);
        }

        /// Pops up a real notification: `cargo test -- --ignored notification`.
        #[test]
        #[ignore]
        fn sends_a_notification() {
            let id = pollster::block_on(async {
                let conn = zbus::Connection::session().await.expect("session bus");
                let note = Note { body: "Exported Tom & Jerry.wav".into(), error: false };
                notify(&conn, &note, 0).await
            });
            assert!(id.is_some_and(|id| id > 0), "{id:?}");
        }
    }
}
