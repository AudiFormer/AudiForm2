// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! AudiForm 2 on the Linux desktop: its launcher entry, its icon in the
//! theme's colours, and desktop notifications.
//!
//! On Wayland a window can't carry its own icon: the desktop finds it
//! through the window's app ID (`audiform2`), the launcher entry of that
//! name in `~/.local/share/applications`, and the icon of that name in
//! `~/.local/share/icons/hicolor`. So AudiForm 2 writes both itself
//! whenever the theme's icon changes (Mall Pastel '95, its Night and
//! Undercurrent have their own pictures; every other theme the AudiForm 2
//! icon in its colours), and again if the program moves.
//!
//! Each icon gets a name of its own (`audiform2-pastel`, or
//! `audiform2-<hash of its colours>`) and the entry's `Icon=` names it.
//! Plasma keeps an icon it has loaded for as long as it runs, by name, so
//! new pixels under the same name would never show; a new name always
//! does. The plain `audiform2` icon is kept up to date too, for anything
//! that asks for it by the app ID. A launcher entry the user wrote
//! themselves is left alone.
//!
//! Messages that come up while AudiForm 2 isn't the focused window (an
//! export finishing, a bounce, an error) are also sent as desktop
//! notifications through `org.freedesktop.Notifications`, carrying the
//! theme's icon. Everything here runs off the UI thread except deciding
//! what to send; none of it touches the audio thread.

use crate::app_icon::{self, Picture};
use crate::state::Toast;
use crate::theme::Theme;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The window's app ID, and the name of its launcher entry and icon.
pub const APP_ID: &str = "audiform2";

/// The line that marks a launcher entry as written by AudiForm 2.
const MARK: &str = "X-AudiForm2-Written-By=AudiForm 2";

/// The icon sizes the hicolor theme has folders for.
const ICON_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// Writing to the user's desktop folders is off for self-screenshot runs
/// and when AF2_NO_DESKTOP_ENTRY is set.
fn may_write() -> bool {
    std::env::var_os("AF2_SCREENSHOT").is_none()
        && std::env::var_os("AF2_NO_DESKTOP_ENTRY").is_none()
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))
}

/// A path quoted for a launcher entry's Exec line.
fn exec_quote(p: &Path) -> String {
    let mut s = String::from("\"");
    for ch in p.to_string_lossy().chars() {
        if matches!(ch, '"' | '`' | '$' | '\\') {
            s.push('\\');
        }
        s.push(ch);
    }
    s.push('"');
    s
}

fn entry_text(exe: &Path, icon: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=AudiForm 2\n\
         GenericName=Audio Editor\n\
         Comment=Record, edit and mix audio\n\
         Exec={} %F\n\
         Icon={icon}\n\
         StartupWMClass={APP_ID}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;AudioVideoEditing;\n\
         {MARK}\n",
        exec_quote(exe)
    )
}

/// Writes `bytes` to `path` through a temporary file, so nothing ever
/// reads half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("af2-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Writes the launcher entry `~/.local/share/applications/audiform2.desktop`
/// for this program and `icon`, unless it is already right or the user
/// wrote their own.
fn write_entry(home: &Path, icon: &str) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path = home.join("applications").join(format!("{APP_ID}.desktop"));
    let want = entry_text(&exe, icon);
    match std::fs::read_to_string(&path) {
        Ok(have) if have == want || !have.contains(MARK) => {}
        _ => {
            let _ = write_atomic(&path, want.as_bytes());
        }
    }
}

/// The installed name of the theme's icon: `audiform2-<picture>` for a
/// theme's own picture, else `audiform2-` and a short hash of what the
/// icon is made from.
pub fn icon_name(th: &Theme) -> String {
    use sha2::{Digest, Sha256};
    let k = app_icon::key(th);
    match k.strip_prefix("picture:") {
        Some(p) => format!("{APP_ID}-{p}"),
        None => {
            let h = Sha256::digest(k.as_bytes());
            let hex: String = h[..4].iter().map(|b| format!("{b:02x}")).collect();
            format!("{APP_ID}-{hex}")
        }
    }
}

/// The latest icon asked for; an older one still being written gives way.
static ICON_GEN: AtomicU64 = AtomicU64::new(0);
/// One icon written at a time.
static ICON_LOCK: Mutex<()> = Mutex::new(());

/// Writes the theme's icon into `~/.local/share/icons/hicolor` at each
/// size, under its own name and as plain `audiform2`, clears away the
/// names of earlier icons, then points the launcher entry at it, on a
/// thread of its own. Called by `app_icon::apply` when the theme's icon
/// changes.
pub fn install_icon(th: &Theme) {
    if !may_write() {
        return;
    }
    let Some(home) = data_home() else {
        return;
    };
    let gen = ICON_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    let th = th.clone();
    let _ = std::thread::Builder::new()
        .name("desktop-icon".into())
        .spawn(move || install(&home, &th, gen));
}

/// Puts the icon and launcher entry in place for the theme the window will
/// open in, before it opens. KWin reads the entry once, when the window
/// appears, for the icon in its title bar; a change after that reaches
/// the taskbar but not the title bar until the next start. Does nothing
/// when they are already right, which is almost every start.
pub fn prepare_before_window() {
    if !may_write() {
        return;
    }
    let Some(home) = data_home() else {
        return;
    };
    let th = crate::state::opening_theme();
    let gen = ICON_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    install(&home, &th, gen);
}

/// Whether the icon `name` is installed at every size and the entry
/// names it (or the entry is the user's own).
fn up_to_date(home: &Path, name: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let root = home.join("icons").join("hicolor");
    let icons = ICON_SIZES.iter().all(|s| {
        root.join(format!("{s}x{s}"))
            .join("apps")
            .join(format!("{name}.png"))
            .is_file()
    });
    let entry = home.join("applications").join(format!("{APP_ID}.desktop"));
    icons
        && std::fs::read_to_string(entry)
            .is_ok_and(|have| have == entry_text(&exe, name) || !have.contains(MARK))
}

fn install(home: &Path, th: &Theme, gen: u64) {
    let _one = ICON_LOCK.lock();
    let name = icon_name(th);
    if up_to_date(home, &name) {
        return;
    }
    let root = home.join("icons").join("hicolor");
    for size in ICON_SIZES {
        if ICON_GEN.load(Ordering::Acquire) != gen {
            return;
        }
        let Some(png) = app_icon::build(th, size).and_then(|p| png(&p)) else {
            continue;
        };
        let dir = root.join(format!("{size}x{size}")).join("apps");
        for n in [name.as_str(), APP_ID] {
            let _ = write_atomic(&dir.join(format!("{n}.png")), &png);
        }
        clear_old(&dir, &name);
    }
    // Icon caches look at the theme folder's time to see a change.
    if let Ok(f) = std::fs::File::open(&root) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
    if ICON_GEN.load(Ordering::Acquire) == gen {
        write_entry(home, &name);
    }
}

/// Removes earlier `audiform2-*.png` icons from `dir`, all but `keep`.
fn clear_old(dir: &Path, keep: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{APP_ID}-");
    for e in rd.flatten() {
        let f = e.file_name();
        let f = f.to_string_lossy();
        if let Some(stem) = f.strip_suffix(".png") {
            if stem.starts_with(&prefix) && stem != keep {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

fn png(p: &Picture) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(p.size, p.size, p.rgba.clone())?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

// -------------------------------------------------------- notifications

/// One desktop notification.
struct Note {
    body: String,
    error: bool,
    icon: Option<Picture>,
}

/// A notification within this long of the last replaces it, so a burst
/// (exporting many files) shows as one.
const REPLACE_WITHIN: Duration = Duration::from_secs(4);

fn notifier() -> Option<mpsc::Sender<Note>> {
    static TX: OnceLock<Option<Mutex<mpsc::Sender<Note>>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Note>();
        std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || pollster::block_on(send_all(rx)))
            .ok()?;
        Some(Mutex::new(tx))
    })
    .as_ref()?
    .lock()
    .ok()
    .map(|tx| tx.clone())
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

async fn notify(conn: &zbus::Connection, note: &Note, replaces: u32) -> Option<u32> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;
    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("desktop-entry", Value::from(APP_ID));
    hints.insert("urgency", Value::U8(1));
    if let Some(p) = &note.icon {
        let s = p.size as i32;
        hints.insert(
            "image-data",
            Value::from((s, s, s * 4, true, 8i32, 4i32, p.rgba.clone())),
        );
    }
    let summary = if note.error {
        "AudiForm 2: something went wrong"
    } else {
        "AudiForm 2"
    };
    let reply = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "AudiForm 2",
                replaces,
                APP_ID,
                summary,
                note.body.as_str(),
                Vec::<&str>::new(),
                hints,
                -1i32,
            ),
        )
        .await
        .ok()?;
    reply.body().deserialize::<u32>().ok()
}

thread_local! {
    /// The newest message already looked at.
    static SEEN: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// The theme's icon for notifications, by what it was made from.
    static ICON: std::cell::RefCell<Option<(String, Picture)>> = const { std::cell::RefCell::new(None) };
}

/// Once a frame: messages that came up since the last look go out as
/// desktop notifications if the window isn't focused (and `enabled`).
/// Messages from before the first frame are never sent.
pub fn notify_toasts(toasts: &[Toast], focused: bool, enabled: bool, th: &Theme) {
    let seen = SEEN.with(|s| s.get()).unwrap_or_else(Instant::now);
    let newest = toasts.iter().map(|t| t.at).max().unwrap_or(seen).max(seen);
    SEEN.with(|s| s.set(Some(newest)));
    if focused || !enabled {
        return;
    }
    let fresh: Vec<&Toast> = toasts.iter().filter(|t| t.at > seen).collect();
    if fresh.is_empty() {
        return;
    }
    let Some(tx) = notifier() else {
        return;
    };
    let icon = theme_icon(th);
    for t in fresh {
        let _ = tx.send(Note {
            body: t.text.clone(),
            error: t.error,
            icon: icon.clone(),
        });
    }
}

fn theme_icon(th: &Theme) -> Option<Picture> {
    let k = app_icon::key(th);
    ICON.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((have, p)) = &*c {
            if *have == k {
                return Some(p.clone());
            }
        }
        let p = app_icon::build(th, 128)?;
        *c = Some((k, p.clone()));
        Some(p)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_paths_are_quoted() {
        assert_eq!(
            exec_quote(Path::new("/opt/AudiForm 2/audiform2")),
            "\"/opt/AudiForm 2/audiform2\""
        );
        assert_eq!(exec_quote(Path::new("/a/$b\"c")), "\"/a/\\$b\\\"c\"");
    }

    #[test]
    fn the_entry_names_the_app_id_and_is_marked() {
        let e = entry_text(Path::new("/usr/bin/audiform2"), "audiform2-pastel");
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nIcon=audiform2-pastel\n"));
        assert!(e.contains("\nStartupWMClass=audiform2\n"));
        assert!(e.contains("\nExec=\"/usr/bin/audiform2\" %F\n"));
        assert!(e.contains(MARK));
    }

    #[test]
    fn every_icon_has_a_name_of_its_own() {
        let mut th = Theme::zero_point();
        let plain = icon_name(&th);
        assert!(plain.starts_with("audiform2-") && plain.len() == "audiform2-".len() + 8);
        assert_eq!(icon_name(&Theme::zero_point()), plain, "stable");
        assert_ne!(icon_name(&Theme::solar_flare()), plain);
        th.icon = "pastel".into();
        assert_eq!(icon_name(&th), "audiform2-pastel");
        th.icon = "pastel-night".into();
        assert_eq!(icon_name(&th), "audiform2-pastel-night");
    }

    #[test]
    fn old_icons_are_cleared_but_not_others() {
        let dir = std::env::temp_dir().join(format!("af2-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "audiform2.png",
            "audiform2-pastel.png",
            "audiform2-1a2b3c4d.png",
            "other.png",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        clear_old(&dir, "audiform2-pastel");
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["audiform2-pastel.png", "audiform2.png", "other.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icons_build_at_every_hicolor_size_for_the_picture_themes() {
        for name in ["", "pastel", "pastel-night", "undercurrent"] {
            let mut th = Theme::zero_point();
            th.icon = name.into();
            for size in ICON_SIZES {
                let p = app_icon::build(&th, size).unwrap();
                assert_eq!(p.size, size);
                let bytes = png(&p).unwrap();
                let back = image::load_from_memory(&bytes).unwrap();
                assert_eq!((back.width(), back.height()), (size, size));
            }
        }
    }

    #[test]
    fn only_new_messages_while_away_count() {
        let th = Theme::zero_point();
        let toast = |text: &str, at| Toast {
            text: text.into(),
            at,
            error: false,
        };
        let t0 = Instant::now();
        // The first look only marks what is there (and anything older
        // than now).
        notify_toasts(&[toast("before", t0)], false, false, &th);
        let first = SEEN.with(|s| s.get()).unwrap();
        assert!(first >= t0);
        let t1 = first + Duration::from_secs(1);
        // Focused: seen, not sent.
        notify_toasts(&[toast("before", t0), toast("after", t1)], true, true, &th);
        assert_eq!(SEEN.with(|s| s.get()), Some(t1));
        // Pruned messages don't move it back.
        notify_toasts(&[], false, false, &th);
        assert_eq!(SEEN.with(|s| s.get()), Some(t1));
    }

    /// Pops up a real notification: `cargo test -- --ignored notification`.
    #[test]
    #[ignore]
    fn sends_a_notification() {
        let mut th = Theme::zero_point();
        th.icon = "undercurrent".into();
        let id = pollster::block_on(async {
            let conn = zbus::Connection::session().await.expect("session bus");
            let note = Note {
                body: "Test notification with the Undercurrent icon".into(),
                error: false,
                icon: theme_icon(&th),
            };
            notify(&conn, &note, 0).await
        });
        assert!(id.is_some_and(|id| id > 0), "{id:?}");
    }
}
