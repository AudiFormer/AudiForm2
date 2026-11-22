// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Timing of the Audiform effects (run by hand, release).

#[test]
#[ignore]
fn bench_fx() {
    let rate = 48_000u32;
    let n = rate as usize * 30;
    let src: Vec<f32> = (0..n * 2)
        .map(|i| ((i as f32) * 0.013).sin() * 0.3)
        .collect();
    for (id, pairs) in [
        ("af.reverb", vec![("mix".to_string(), 0.2)]),
        ("af.opto_leveler", vec![]),
    ] {
        let p = af2_fx::params_from(id, &pairs).unwrap();
        let mut fx = af2_fx::build(id, rate, &p).unwrap();
        let mut b = src.clone();
        let t0 = std::time::Instant::now();
        for c in b.chunks_mut(512) {
            fx.process(c);
        }
        let took = t0.elapsed().as_secs_f64();
        eprintln!("{id}: {:.1} ms per 30 s stereo", took * 1000.0);
    }
}
