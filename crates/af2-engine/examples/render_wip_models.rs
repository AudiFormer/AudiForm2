//! Renders a short demo of each work-in-progress synth model to WAV
//! (dry and with Analog at 70%): `cargo run -p af2-engine --example
//! render_wip_models -- <out dir>`.
use af2_core::synth_model::SynthModel;
use af2_engine::synth::{note, Synth};

const RATE: u32 = 48_000;

fn wav(path: &std::path::Path, l: &[f32], r: &[f32]) {
    let n = l.len() as u32;
    let mut b = Vec::with_capacity(44 + l.len() * 4);
    let g = 2.0;
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
    // (start, end, pitch, velocity) per model: a phrase in the register
    // it was heard in, played with some light and heavy notes.
    type Ph = Vec<(f64, f64, u8, u8)>;
    let lead: Ph = vec![(0.0, 0.45, 71, 100), (0.5, 1.4, 66, 80), (1.5, 1.9, 69, 110), (2.0, 3.0, 64, 70), (3.2, 3.6, 61, 95), (3.7, 4.6, 61, 85)];
    let high: Ph = vec![(0.0, 0.6, 83, 90), (0.75, 1.5, 85, 110), (1.6, 2.4, 81, 70), (2.6, 4.4, 83, 100)];
    let mut drone: Ph = Vec::new();
    for (a, b, chord) in [(0.0, 2.3, [57, 61, 64, 69, 73, 76, 80]), (2.4, 4.7, [54, 61, 64, 66, 69, 73, 76])] {
        for p in chord {
            drone.push((a, b, p, 90));
        }
    }
    let pad: Ph = vec![(0.0, 0.6, 71, 90), (0.6, 2.4, 73, 100), (0.6, 2.4, 66, 75), (2.6, 5.0, 69, 95), (2.6, 5.0, 64, 70)];
    let ice: Ph = vec![(0.0, 0.3, 83, 100), (0.35, 0.6, 78, 70), (0.7, 1.0, 81, 115), (1.1, 1.6, 76, 80), (1.8, 2.1, 78, 95), (2.2, 2.5, 73, 60), (2.6, 3.4, 73, 105)];
    let jobs: [(SynthModel, &Ph); 5] = [
        (SynthModel::AtlantisVerseLead, &lead),
        (SynthModel::WildHorsesDemoPad, &pad),
        (SynthModel::WildHorsesIntroLead, &high),
        (SynthModel::WildHorsesCrushDrone, &drone),
        (SynthModel::IceQueen, &ice),
    ];
    let mut reference = None;
    for (m, ph) in jobs {
        // As it comes, and with Analog toggled (on at 70%, or off where
        // the model has it on).
        for toggled in [false, true] {
            let mut inst = m.preset();
            let built_in = inst.ext.analog > 0.0;
            if toggled {
                inst.ext.analog = if built_in { 0.0 } else { 0.7 };
            }
            let notes = ph.iter().map(|&(a, b, p, v)| note(a, b, p, v, 0.0)).collect();
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
                match (toggled, built_in) {
                    (false, _) => "",
                    (true, false) => "-analog",
                    (true, true) => "-no-analog",
                }
            );
            // Loudness while notes sound, against a stock model's.
            let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            let db = 20.0 * rms(&l[..(4.5 * RATE as f64) as usize]).log10();
            let refdb = *reference.get_or_insert_with(|| {
                let notes = lead.iter().map(|&(a, b, p, v)| note(a, b, p, v, 0.0)).collect();
                let mut s = Synth::new(SynthModel::PolyChorus.preset(), notes, RATE);
                let mut l = vec![0f32; (4.5 * RATE as f64) as usize];
                let mut r = l.clone();
                let n = l.len();
                let mut t = 0;
                while t < n {
                    let k = 256.min(n - t);
                    s.render(t as f64 * dt, dt, &mut l[t..t + k], &mut r[t..t + k]);
                    t += k;
                }
                20.0 * rms(&l).log10()
            });
            println!("{name}: {:+.1} dB against Poly Chorus", db - refdb);
            wav(&out.join(name), &l, &r);
        }
    }
}
