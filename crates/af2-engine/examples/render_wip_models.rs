//! Renders a short demo of each work-in-progress synth model to WAV
//! (dry and with Analog at 70%): `cargo run -p af2-engine --example
//! render_wip_models -- <out dir>`.
use af2_core::synth_model::SynthModel;
use af2_engine::synth::{note, Synth};

const RATE: u32 = 48_000;

fn wav(path: &std::path::Path, l: &[f32], r: &[f32]) {
    let n = l.len() as u32;
    let mut b = Vec::with_capacity(44 + l.len() * 4);
    let peak = l.iter().chain(r).fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let g = (0.89 / peak).min(4.0);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n * 4).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n * 4).to_le_bytes());
    for i in 0..l.len() {
        for v in [l[i], r[i]] {
            b.extend_from_slice(&(((v * g).clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
    }
    std::fs::write(path, b).unwrap();
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or("wip".into()));
    std::fs::create_dir_all(&out).unwrap();
    // (start, end, pitch) per model: a phrase in the register it was heard in.
    let lead = [(0.0, 0.45, 71), (0.5, 1.4, 66), (1.5, 1.9, 69), (2.0, 3.2, 64), (3.4, 5.0, 66)];
    let chords = [(0.0, 2.2, 62), (0.0, 2.2, 66), (0.0, 2.2, 69), (2.4, 4.8, 64), (2.4, 4.8, 68), (2.4, 4.8, 71)];
    let high = [(0.0, 0.6, 83), (0.75, 1.5, 85), (1.6, 2.4, 81), (2.6, 4.4, 83)];
    let drone = [(0.0, 5.0, 33), (0.0, 5.0, 45)];
    let pad = [(0.0, 0.6, 71), (0.6, 2.4, 73), (0.6, 2.4, 66), (2.6, 5.0, 69), (2.6, 5.0, 64)];
    let jobs: [(SynthModel, &[(f64, f64, u8)]); 6] = [
        (SynthModel::AtlantisVerseLead, &lead),
        (SynthModel::AtlantisChorusPoly, &chords),
        (SynthModel::WildHorsesDemoPad, &pad),
        (SynthModel::WildHorsesIntroLead, &high),
        (SynthModel::WildHorsesCrushDrone, &drone),
        (SynthModel::IceQueen, &lead.map(|(a, b, p)| (a, b, p + 12))),
    ];
    for (m, ph) in jobs {
        for analog in [0.0f32, 0.7] {
            let mut inst = m.preset();
            inst.ext.analog = analog;
            let notes = ph.iter().map(|&(a, b, p)| note(a, b, p, 100, 0.0)).collect();
            let mut s = Synth::new(inst, notes, RATE);
            let total = (6.0 * RATE as f64) as usize;
            let (mut l, mut r) = (vec![0f32; total], vec![0f32; total]);
            let dt = 1.0 / RATE as f64;
            let mut t = 0;
            while t < total {
                let n = 256.min(total - t);
                s.render(t as f64 * dt, dt, &mut l[t..t + n], &mut r[t..t + n]);
                t += n;
            }
            let name = format!(
                "{}{}.wav",
                m.name().replace(" (WIP)", "").replace(' ', "_"),
                if analog > 0.0 { "-analog" } else { "" }
            );
            wav(&out.join(name), &l, &r);
        }
    }
}
