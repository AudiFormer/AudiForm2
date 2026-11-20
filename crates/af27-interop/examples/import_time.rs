// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Times importing one session file (read only):
//! `cargo run --release -p af27-interop --example import_time -- <file>`.
fn main() {
    let path = std::env::args().nth(1).expect("a session file");
    let t = std::time::Instant::now();
    match af27_interop::import(std::path::Path::new(&path)) {
        Ok(i) => {
            let clips: usize = i.session.tracks.iter().map(|t| t.clips.len()).sum();
            println!(
                "imported in {:.2}s: {} tracks, {} clips, {} media, {} notices",
                t.elapsed().as_secs_f64(),
                i.session.tracks.len(),
                clips,
                i.session.media.len(),
                i.notices.len()
            );
            for n in &i.notices {
                println!("- {n}");
            }
            let stretched = i
                .session
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .filter(|c| c.props.stretch.is_on())
                .count();
            println!("stretched clips: {stretched}; space: {:?}", i.session.space);
        }
        Err(e) => println!("failed after {:.2}s: {e}", t.elapsed().as_secs_f64()),
    }
}
