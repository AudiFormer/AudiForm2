# Porting notes: af2-fx

These effects come from Audiform (`Audiform/src-tauri/src/dsp/`), from files
written by Audiform's author (AudiFormer) and relicensed by them for Audiform2
under MIT OR Apache-2.0. Every algorithm, constant, default, range, curve,
parameter key and comment was kept, apart from the points listed below.
Each file's header was replaced with the Audiform2 SPDX header and a
"Ported from" line.

Nothing was copied from Altered Audio Editor's own modules. The framework
(`src/framework.rs`), the filter sections (`src/biquad.rs`), the registry
(`src/lib.rs`) and `src/effects/mod.rs` were written new for Audiform2.

## Files ported

| Audiform file | af2-fx module | Effect(s) and id |
| --- | --- | --- |
| `effects/amp.rs` | `effects::amp` | Amp Model, `af.amp_model` |
| `nam.rs` | `nam` | `.nam` capture player (WaveNet, LSTM) used by the Amp Model |
| `effects/tube.rs` | `effects::tube` | Tube Preamp, `af.tube_preamp`; Opto Leveler, `af.opto_leveler` (same file) |
| `effects/echo.rs` | `effects::echo` | Delay and Echo, `af.echo` (see the provenance note below) |
| `effects/verb.rs` | `effects::verb` | Reverb (Room, Hall, Plate, Spring, Tape), `af.reverb` |
| `effects/reverb.rs` | `effects::reverb` | Convolution Reverb, `af.convolution_reverb` |
| `convolve.rs` | `convolve` | Partitioned FFT convolution and Lanczos resampling of a response |
| `effects/modulation.rs` | `effects::modulation` | Chorus, Flanger, Phaser, Double Track: `af.modulation` |
| `effects/center.rs` | `effects::center` | Center Channel Extractor, `af.center_extractor` |
| `effects/mixer.rs` | `effects::mixer` | Channel Mixer, `af.channel_mixer` |
| `effects/width.rs` | `effects::width` | Stereo Width, `af.stereo_width` |
| `effects/phase_curve.rs` | `effects::phase_curve` | Phase Curve, `af.phase_curve` |
| `effects/vocal.rs` | `effects::vocal` | Vox Strip, `af.vox_strip` |
| `meters.rs` | `meters` | Slot meters (gain reduction, output, aux) |

Each effect's own tests were ported and pass. `src/lib.rs` adds one test
per effect (`*_runs_clean`). Each runs the effect on a stereo sine at its
defaults and at every option of every pick-one control and switch, and
checks there are no NaNs. Another test runs all effects at 44.1, 48 and
96 kHz. The two `nam` reference tests stay `#[ignore]`, as they were in
Audiform: they need the Neural Amp Modeler example captures (`NAM_REF`).

## Left out, and why

- **Speaker beds and surround.** Audiform2 effects are stereo only, so
  the following were removed:
  - `Echo::wide`, `Modulation::wide`, `Reverb::wide` and
    `ConvolutionReverb::wide`;
  - their `Bed` / `ConvBed` structs and the `process_bed` paths;
  - the Convolution Reverb's `Feed` upmix and `Decorrelate`;
  - the Reverb's Hadamard `sign` helper, which only the bed used;
  - the Echo's `MAX_APART`, and the "On a bed..." branches in Echo's
    Simple kind.

  Each module's header now has one line saying the bed version was not
  ported.
- **Spread controls.** The `spread` ("Room spread") controls of Delay and
  Echo and of Modulation are kept, with the same key, range and default,
  so saved settings still load. They only ever acted on a bed, so in
  stereo they change nothing, just as in Audiform.
- **`Processor::channels`, `is_plugin`, `vary` and the chain machinery.**
  These belonged to the host framework: `ChainProcessor`, routing,
  plug-ins, `chain_latency` and `chain_hash`. `Modulation::vary` is kept
  as a plain public method.
- **`ParamInfo::retimes`.** Hosts can compare `latency()` before and after
  `update` instead. `hidden` is kept.
- **`shaped.rs`, `wide.rs`, and anything VST3 or plug-in.** These were out
  of scope, as asked.

## Changes, and behaviour that may not be exactly the same

- **Ids.** The old `altaudio.*` ids are now `af.*` ids (see the table
  above). Echo and Phase Curve now have an `ID` constant.
- **Filter sections.** `biquad.rs` is new code built on the public RBJ
  Audio EQ Cookbook formulas, so the curves are the ones the effects were
  tuned for. Two implementation details may differ from what Audiform
  ran on:
  - the state is transposed direct form II in f64;
  - the frequency is kept below 0.4999 of the rate.

  Rounding may differ in the last bits. Every ported test still passes
  with the same limits.
- **Convolution Reverb file reading.** The file is now read with
  `af2_io::decode_file` and resampled with `af2_dsp::resample`. Audiform
  used its own streaming reader and resampler, so a response at another
  rate can differ by a tiny amount. Files with 3 channels or with 5 or
  more channels use their first two channels. Mono, stereo and
  four-channel (true stereo) files work exactly as before. The first 12 s
  (`MAX_SECONDS`) are kept, trailing silence is trimmed and leveling is unchanged.
- **Slot data and meters.** Audiform passed these from its session slot.
  Here they go to `build_with(id, rate, &params, data, meter)`:
  - `data` is the JSON text: the Amp Model's capture and cabinet, or the
    Convolution Reverb's file path;
  - `meter` is an `Arc<SlotMeter>`, from `meters::for_slot(id)` with any
    `u64` slot id.

  `build` passes neither: no data, and a private meter.
- **`Params`.** `Params::resolve` fills in the defaults, clamps saved
  values to each control's range (rounding pick-one controls and
  switches), and ignores keys it does not know.
- **The Amp Model's `update`** still returns false, so the host rebuilds
  it, as before.

## Provenance note: Delay and Echo (`effects/echo.rs`)

**Check this before committing.** Audiform's own `ATTRIBUTION.md`, in the
section "From later Altered Audio Editor releases", lists **"Delay and Echo
effect, and its four presets", from Altered Audio Editor 0.4.1 and 0.5.0, as
brought in under GPL v3.0 or later**, with `src-tauri/src/dsp/effects/echo.rs`
as the file. Commit 9204329 ("Audiform 0.4.0") added that row and the file
together. In that commit the file carried the GeekCast copyright line.
Later commits by the author (e7ed223, 7790ccd, b68651b, 4c74039) added the
following:

- the Tape, Analog and Simple kinds;
- the machine noise;
- tempo sync;
- the bed spread.

The core (ring, one-pole darkened feedback, gliding read point, ping-pong)
dates from that first version. Unless the author confirms that this file is
their own and the ATTRIBUTION row is wrong, `effects/echo.rs` should be
dropped from af2-fx or rewritten clean-room. To drop it, remove its module
line, its registry and `build_with` entries, and its `clean!` test line.
No other module uses it.

## Third-party notice

`nam.rs` follows the weight layout and forward passes of
NeuralAmpModelerCore (sdatkinson/NeuralAmpModelerCore, Copyright (c) 2023
Steven Atkinson, MIT licence), written fresh in Rust. It also uses that
project's quick tanh formula, so captures sound as they do in its plug-in.
The files consulted were `NAM/wavenet/model.cpp`, `NAM/lstm.cpp`,
`NAM/conv1d.cpp`, `NAM/dsp.cpp` and `NAM/activations.h`. Audiform credited
this in its `ATTRIBUTION.md`. Audiform2 should carry the same notice.


## Removed after porting: Delay and Echo

Audiform's ATTRIBUTION.md lists `effects/echo.rs` (Delay and Echo and its presets) as brought in from Altered Audio Editor 0.4.1 and 0.5.0 under the GPL, so it is not the lead's own code and was removed. Audiform2 keeps its own clean-room Echo in `af2-dsp`.
