// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Files the open decoders refuse, read through the system's decoders
//! (Windows). Each test file (made with ffmpeg from sine tones) has a
//! different tone on each speaker; the expected order is ffmpeg's own
//! decode of the same file.

#![cfg(windows)]

use std::path::PathBuf;

fn media(f: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/media").join(f)
}

/// Each channel's tone in Hz (by zero crossings, edges left out), and
/// who decoded it.
fn tones_by(f: &str) -> (u32, Vec<u32>, String) {
    let o = af2_io::open_file(&media(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let b = o.buf;
    let hz = b
        .channels
        .iter()
        .map(|c| {
            let s = &c[4800..c.len() - 4800];
            let z = s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
            (z as f64 / 2.0 / (s.len() as f64 / b.rate as f64) / 50.0).round() as u32 * 50
        })
        .collect();
    (b.rate, hz, o.notes.join(" "))
}

fn tones(f: &str) -> (u32, Vec<u32>) {
    let (rate, hz, by) = tones_by(f);
    assert!(by.contains("decoded by Windows"), "{by}");
    (rate, hz)
}

#[test]
fn dolby_digital_51_opens_with_every_speaker() {
    assert_eq!(tones("ac3_51.m4a"), (48_000, vec![300, 400, 200, 500, 600, 700]));
}

#[test]
fn dolby_digital_plus_51_opens_with_every_speaker() {
    // How Dolby Atmos music comes: its bed decodes like this.
    assert_eq!(tones("eac3_51.m4a"), (48_000, vec![300, 400, 200, 500, 600, 700]));
}

/// Windows' AAC decoder refuses these surround layouts, so they need an
/// installed ffmpeg (skipped without one).
#[test]
fn surround_aac_opens_through_ffmpeg_when_there_is_one() {
    let Ok(o) = af2_io::open_file(&media("aac_51.m4a")) else {
        eprintln!("no ffmpeg here: skipped");
        return;
    };
    assert!(o.notes.join(" ").contains("decoded by ffmpeg"));
    assert_eq!(o.buf.num_channels(), 6);
    let (rate, hz, _) = tones_by("aac_71.m4a");
    assert_eq!(rate, 48_000);
    // The LFE (index 3) is low-passed by the encoder; the rest are clean.
    assert_eq!([&hz[..3], &hz[4..]].concat(), vec![300, 400, 200, 600, 700, 800, 900]);
}

#[test]
fn the_codec_is_named() {
    use af2_io::fallback::codec_name;
    assert_eq!(codec_name(&media("eac3_51.m4a")), Some("Dolby Digital Plus (E-AC-3)"));
    assert_eq!(codec_name(&media("ac3_51.m4a")), Some("Dolby Digital (AC-3)"));
    assert_eq!(codec_name(&media("aac_71.m4a")), Some("AAC"));
}
