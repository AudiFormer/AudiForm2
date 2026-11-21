// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Multitrack Ogg (`.mogg`): more than 8 Vorbis channels decode with our
//! own decoder (the patched Vorbis decoder in vendor/), in file order,
//! with or without the MOGG header in front.

use std::path::PathBuf;

fn media(f: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/media").join(f)
}

fn tones(b: &af27_core::AudioBuffer) -> Vec<u32> {
    b.channels
        .iter()
        .map(|c| {
            let s = &c[2400..c.len() - 2400];
            let z = s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
            (z as f64 / 2.0 / (s.len() as f64 / b.rate as f64) / 50.0).round() as u32 * 50
        })
        .collect()
}

const WANT: [u32; 10] = [200, 300, 400, 500, 600, 700, 800, 900, 1000, 1100];

#[test]
fn ten_channels_decode_in_file_order_without_help() {
    let o = af27_io::open_file(&media("tenchannel.mogg")).unwrap();
    assert!(o.notes.is_empty(), "our own decoder, no fallback: {:?}", o.notes);
    assert_eq!(o.buf.rate, 48_000);
    assert_eq!(tones(&o.buf), WANT);
}

#[test]
fn the_mogg_header_is_skipped() {
    let ogg = std::fs::read(media("tenchannel.mogg")).unwrap();
    let mut b = 10u32.to_le_bytes().to_vec();
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&[0u8; 8]);
    b.extend_from_slice(&ogg);
    let dir = std::env::temp_dir().join(format!("af2-mogg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("headed.mogg");
    std::fs::write(&p, &b).unwrap();
    let o = af27_io::open_file(&p).unwrap();
    assert_eq!(tones(&o.buf), WANT);
    // An encrypted one is refused and says so.
    b[0] = 11;
    std::fs::write(&p, &b).unwrap();
    let e = af27_io::open_file(&p).err().unwrap();
    assert!(e.contains("encrypted"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
