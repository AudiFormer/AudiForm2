// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Audiform27's effects, ported from the effects their author wrote for
//! Audiform (see PORTING.md): Amp Model, Opto Leveler, Tube Preamp, Delay
//! and Echo, Reverb, Convolution Reverb, Modulation, Center Channel
//! Extractor, Channel Mixer, Stereo Width, Phase Curve and Vox Strip.
//!
//! [`registry`] lists them, [`build`] makes a running [`Processor`] for
//! one, and [`params_from`] turns saved values into [`Params`].

use std::collections::BTreeMap;
use std::sync::Arc;

pub mod biquad;
pub mod convolve;
pub mod effects;
pub mod framework;
pub mod meters;
pub mod nam;

pub use framework::{choice, db_to_gain, num, toggle, EffectInfo, ParamInfo, Params, Processor};
pub use meters::SlotMeter;

use effects::{amp, center, mixer, modulation, phase_curve, reverb, tube, verb, vocal, width};

/// Every effect, in menu order.
pub fn registry() -> Vec<EffectInfo> {
    vec![
        amp::amp_info(),
        tube::preamp_info(),
        tube::opto_info(),
        vocal::vocal_info(),
        verb::verb_info(),
        reverb::reverb_info(),
        modulation::modulation_info(),
        center::center_extractor_info(),
        mixer::mixer_info(),
        width::width_info(),
        phase_curve::phase_curve_info(),
    ]
}

/// The description of one effect.
pub fn info(id: &str) -> Option<EffectInfo> {
    registry().into_iter().find(|i| i.id == id)
}

/// An effect's values: its defaults, with `saved` in their place where
/// given (each kept inside its range). None for an unknown effect.
pub fn params_from(id: &str, saved: &[(String, f64)]) -> Option<Params> {
    info(id).map(|i| Params::from_pairs(&i, saved))
}

/// The same, from a saved map.
pub fn params_from_map(id: &str, saved: &BTreeMap<String, f64>) -> Option<Params> {
    info(id).map(|i| Params::resolve(&i, saved))
}

/// A running processor for effect `id` at `rate`, with no slot data
/// (no amp capture or impulse response loaded) and a meter nobody reads.
pub fn build(id: &str, rate: u32, params: &Params) -> Option<Box<dyn Processor>> {
    build_with(id, rate, params, None, None)
}

/// A running processor for effect `id` at `rate`.
///
/// `data` is the slot's data text (JSON): the Amp Model's capture and
/// cabinet, the Convolution Reverb's impulse response. `meter` is where
/// effects with a meter in their window (Opto Leveler, Tube Preamp, Vox
/// Strip) report it; see [`meters::for_slot`]. Build off the audio thread.
pub fn build_with(
    id: &str,
    rate: u32,
    params: &Params,
    data: Option<&str>,
    meter: Option<Arc<SlotMeter>>,
) -> Option<Box<dyn Processor>> {
    let rate = rate.max(1);
    let p = params;
    let meter = || meter.clone().unwrap_or_default();
    Some(match id {
        amp::AMP => Box::new(amp::AmpModel::new(p, rate, data)),
        tube::OPTO => Box::new(tube::Opto::new(p, rate, meter())),
        tube::PREAMP => Box::new(tube::Preamp::new(p, rate, meter())),
        vocal::VOCAL => Box::new(vocal::VoxStrip::new(p, rate, meter())),
        verb::ID => Box::new(verb::Reverb::new(p, rate)),
        reverb::ID => Box::new(reverb::ConvolutionReverb::new(p, rate, data)),
        modulation::ID => Box::new(modulation::Modulation::new(p, rate)),
        center::ID => Box::new(center::CenterExtractor::new(p, rate)),
        mixer::ID => Box::new(mixer::ChannelMixer::new(p)),
        width::WIDTH => Box::new(width::StereoWidth::new(p, rate)),
        phase_curve::ID => Box::new(phase_curve::PhaseCurve::new(p, rate)),
        _ => return None,
    })
}

/// Helpers for the effects' tests: a slot like a host keeps one.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct Slot {
        pub id: meters::Id,
        pub effect: String,
        pub params: BTreeMap<String, f64>,
        pub data: Option<String>,
    }

    pub fn new_id() -> meters::Id {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    pub fn slot(effect: &str, params: &[(&str, f64)]) -> Slot {
        Slot {
            id: new_id(),
            effect: effect.into(),
            params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            data: None,
        }
    }

    pub fn build(s: &Slot, rate: u32) -> Option<Box<dyn Processor>> {
        let p = params_from_map(&s.effect, &s.params)?;
        build_with(
            &s.effect,
            rate,
            &p,
            s.data.as_deref(),
            Some(meters::for_slot(s.id)),
        )
    }

    /// Two seconds of a stereo sine (left 440 Hz, right 660 Hz) at -6 dBFS.
    pub fn stereo_sine(rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f32 / rate as f32;
                [
                    0.5 * (std::f32::consts::TAU * 440.0 * t).sin(),
                    0.5 * (std::f32::consts::TAU * 660.0 * t).sin(),
                ]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_well_formed_and_every_effect_builds() {
        let all = registry();
        let mut ids: Vec<_> = all.iter().map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len(), "ids are unique");
        for i in &all {
            assert!(i.id.starts_with("af."), "{}", i.id);
            let mut keys: Vec<_> = i.params.iter().map(|p| p.key).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), i.params.len(), "{}: keys are unique", i.id);
            for p in &i.params {
                assert!(
                    p.min <= p.default && p.default <= p.max,
                    "{} {}",
                    i.id,
                    p.key
                );
            }
            let p = params_from(i.id, &[]).unwrap();
            assert!(build(i.id, 48_000, &p).is_some(), "{}", i.id);
        }
        assert!(build("af.nope", 48_000, &Params::default()).is_none());
    }

    /// One test per effect: its defaults and every option of each of its
    /// pick-one controls and switches run on a stereo sine without NaNs.
    fn runs_clean(id: &str) {
        let info = info(id).unwrap();
        let mut settings: Vec<Vec<(&str, f64)>> = vec![vec![]];
        for p in &info.params {
            if p.toggle || !p.choices.is_empty() {
                for v in 0..=(p.max as usize) {
                    settings.push(vec![(p.key, v as f64)]);
                }
            }
        }
        for set in settings {
            let s = testing::slot(id, &set);
            let mut fx = testing::build(&s, 48_000).unwrap();
            let mut buf = testing::stereo_sine(48_000, 12_000);
            for block in buf.chunks_mut(512) {
                fx.process(block);
            }
            assert!(
                buf.iter().all(|v| v.is_finite() && v.abs() < 100.0),
                "{id} with {set:?}"
            );
        }
    }

    macro_rules! clean {
        ($($name:ident => $id:expr,)*) => {
            $(
                #[test]
                fn $name() {
                    runs_clean($id);
                }
            )*
        };
    }

    clean! {
        amp_model_runs_clean => amp::AMP,
        tube_preamp_runs_clean => tube::PREAMP,
        opto_leveler_runs_clean => tube::OPTO,
        vox_strip_runs_clean => vocal::VOCAL,
        reverb_runs_clean => verb::ID,
        convolution_reverb_runs_clean => reverb::ID,
        modulation_runs_clean => modulation::ID,
        center_extractor_runs_clean => center::ID,
        channel_mixer_runs_clean => mixer::ID,
        stereo_width_runs_clean => width::WIDTH,
        phase_curve_runs_clean => phase_curve::ID,
    }

    #[test]
    fn every_effect_runs_on_a_stereo_sine_without_nans() {
        for i in registry() {
            for rate in [44_100, 48_000, 96_000] {
                let p = params_from(i.id, &[]).unwrap();
                let mut fx = build(i.id, rate, &p).unwrap();
                let mut buf = testing::stereo_sine(rate, rate as usize);
                for block in buf.chunks_mut(1024) {
                    fx.process(block);
                }
                assert!(
                    buf.iter().all(|v| v.is_finite() && v.abs() < 100.0),
                    "{} at {rate}",
                    i.id
                );
                fx.reset();
                assert!(fx.update(&p) || !fx.update(&p));
            }
        }
    }
}
