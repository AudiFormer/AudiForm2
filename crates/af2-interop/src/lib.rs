// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Opening other editors' projects as Audiform2 sessions.
//!
//! * `.sesx`: the XML multitrack session format used by a widely used
//!   commercial editor ([`sesx`]); also written by [`sesx_out`].
//! * `.aup3`: an open-source editor's SQLite projects ([`aup3`]).
//! * `.audfrm` / `.altaudio`: Audiform's JSON sessions ([`audfrm`]), and
//!   the session inside an Audiform template (`.altemplate`).
//!
//! Media durations and formats that a file does not state are filled in
//! when the audio is loaded.

pub mod audfrm;
pub mod aup3;
mod b64;
pub mod check;
pub mod paths;
pub mod sesx;
pub mod sesx_out;
pub mod smf;

use std::path::{Path, PathBuf};

use af2_core::session::Session;

/// The result of an import, with anything that could not be carried over.
#[derive(Debug)]
pub struct Imported {
    pub session: Session,
    pub notices: Vec<String>,
    /// The document as read, for lossless saving back (Audiform sessions).
    pub base: Option<serde_json::Value>,
}

/// The kinds of session file this crate opens, by extension (lower case).
pub const SESSION_EXTENSIONS: [&str; 5] = ["audfrm", "altaudio", "altemplate", "sesx", "aup3"];

/// Whether files with this extension (any case) open as sessions here.
pub fn is_session_ext(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    SESSION_EXTENSIONS.contains(&e.as_str())
}

/// Opens a session file of any kind this crate reads. A 5.1 `.sesx` is
/// read as mixed with the surround speakers behind; use
/// [`import_with`] to say otherwise.
pub fn import(path: &Path) -> Result<Imported, String> {
    import_with(path, sesx::Surrounds::Behind)
}

pub fn import_with(path: &Path, surrounds: sesx::Surrounds) -> Result<Imported, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "sesx" => sesx::import_file(path, surrounds),
        "aup3" => aup3::import(path, &aup3::audio_folder(path)),
        "audfrm" | "altaudio" | "altemplate" => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
            audfrm::import(&text, path.parent().unwrap_or(Path::new(".")))
        }
        _ => Err(format!(
            "{} is not a session type this app can open.",
            path.display()
        )),
    }
}

/// Finds a media file: the path relative to the session first (so a show
/// folder that moved, or was copied, uses its own files), then the
/// absolute path, then a file of that name beside the session. Else the
/// absolute path (shown as missing media).
pub(crate) fn resolve(abs: Option<&str>, rel: Option<&str>, base: &Path) -> Option<PathBuf> {
    let a = abs.filter(|s| !s.is_empty()).map(PathBuf::from);
    let r = rel
        .filter(|s| !s.is_empty())
        .map(|r| paths::normalize(&base.join(r.replace('\\', "/"))));
    if let Some(p) = r.as_ref().filter(|p| p.is_file()) {
        // The same file as the absolute path: keep that spelling.
        if let Some(a) = a.as_ref().filter(|a| paths::same(a, p)) {
            return Some(a.clone());
        }
        return Some(p.clone());
    }
    if let Some(p) = a.as_ref().filter(|p| p.exists()) {
        return Some(p.clone());
    }
    // A file moved alongside the session: try its name next to the session.
    if let Some(name) = a.as_ref().and_then(|p| {
        let s = p.to_string_lossy().replace('\\', "/");
        s.rsplit('/').next().map(str::to_string)
    }) {
        let near = base.join(&name);
        if near.exists() {
            return Some(near);
        }
    }
    a.or(r)
}

pub(crate) fn file_name(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let last = s.rsplit('/').next().unwrap_or(&s);
    last.rsplit_once('.').map_or(last, |(a, _)| a).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESX: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<!DOCTYPE sesx>
<sesx version="1.9">
  <session audioChannelType="stereo" bitDepth="32" duration="441000" sampleRate="44100">
    <tracks>
      <audioTrack id="10002" index="2"><trackParameters><name>Second</name></trackParameters>
        <trackAudioParameters solo="true">
          <component id="trackFader"><parameter index="0" name="volume" parameterValue="0.5"/></component>
          <component id="trackMute"><parameter index="0" parameterValue="1"/><parameter index="1" name="mute" parameterValue="1"/></component>
          <component id="trackPan"><parameter index="0" name="Pan" parameterValue="-50"/></component>
        </trackAudioParameters>
      </audioTrack>
      <audioTrack id="10001" index="1"><trackParameters><name>First</name></trackParameters>
        <trackAudioParameters solo="false"/>
        <audioClip endPoint="88200" fileID="0" name="Take" sourceInPoint="4410" sourceOutPoint="48510" startPoint="44100">
          <component id="clipGain"><parameter index="0" name="volume" parameterValue="2"/></component>
          <component id="clipMute"><parameter index="0" parameterValue="0"/><parameter index="1" name="mute" parameterValue="0"/></component>
          <fadeIn endPoint="4410" startPoint="0" type="log"/>
          <fadeOut endPoint="44100" startPoint="22050" type="log"/>
        </audioClip>
      </audioTrack>
    </tracks>
  </session>
  <files>
    <file absolutePath="C:\nowhere\take.wav" id="0" relativePath="Rec/take.wav"/>
  </files>
</sesx>"#;

    #[test]
    fn reads_a_sesx() {
        let imp = sesx::read(SESX, Path::new("."), "Test", sesx::Surrounds::Behind).unwrap();
        let s = &imp.session;
        assert_eq!(s.rate, 44_100);
        assert_eq!(s.tracks.len(), 2);
        assert_eq!(s.tracks[0].name, "First");
        let second = &s.tracks[1];
        assert!(second.mute && second.solo);
        assert!((second.volume_db + 6.02).abs() < 0.01);
        assert!((second.pan + 0.5).abs() < 1e-6);
        let c = &s.tracks[0].clips[0];
        assert!((c.start - 1.0).abs() < 1e-9);
        assert!((c.offset - 0.1).abs() < 1e-9);
        assert!((c.length - 1.0).abs() < 1e-9);
        assert!((c.gain_db - 6.02).abs() < 0.01);
        assert!((c.fade_in - 0.1).abs() < 1e-9);
        assert!((c.fade_out - 0.5).abs() < 1e-9);
        assert_eq!(s.media[0].name, "take");
    }

    #[test]
    fn real_files_on_this_machine_open() {
        // Optional smoke test against real sessions: list them in
        // AF2_SMOKE_FILES, separated by ';'.
        let list = std::env::var("AF2_SMOKE_FILES").unwrap_or_default();
        for p in list.split(';').filter(|p| !p.is_empty()) {
            let p = Path::new(p);
            if p.exists() {
                let imp = import(p).unwrap();
                assert!(!imp.session.tracks.is_empty());
                assert!(imp.session.tracks.iter().any(|t| !t.clips.is_empty()));
            }
        }
    }
}
