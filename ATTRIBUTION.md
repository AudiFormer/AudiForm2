# Attribution

## Code and data from other projects

| Work | Licence | Used in |
| --- | --- | --- |
| FFmpeg's DTS Coherent Acoustics core decoder (`dca_core.c`, `dcadsp.c`, `dcadct.c`, `synth_filter.c`, `dcadata.c`, `dcahuff.c`; foo86, Gildas Bazin and others) | LGPL-2.1-or-later | `crates/af27-dts` (a Rust port; the crate stays under the LGPL) |
| NeuralAmpModelerCore (Steven Atkinson) | MIT | `crates/af27-fx/src/nam.rs` follows its model layout and uses its fast tanh approximation, to play `.nam` amp captures |
| flacenc (Yotaro Kubo) | Apache-2.0 | The FLAC encoder behind `crates/af27-io/src/export/flac.rs` (a Cargo dependency, not copied code) |
| Freeverb (Jezar at Dreampoint) | Public domain | The comb and allpass delay tunings in `crates/af27-dsp/src/reverb.rs` |
| `midly` (Negamartin) | Unlicense | Reading and writing `.mid` files (`crates/af27-interop/src/smf.rs`), as a crate dependency |
| `rustysynth` (Nobuaki Tanaka) | MIT | SoundFont instruments (`crates/af27-engine/src/soundfont.rs`), as a crate dependency; its MIT notice must travel with release builds |
| `embed-resource` (nabijaczleweli and others) | MIT | Builds the icon and version text into the Windows exe (`app/build.rs`); a build-time dependency only, nothing of it is in the program |

**To do before any release:** copy NeuralAmpModelerCore's MIT notice verbatim from its repository's LICENSE file into this section. The MIT licence requires that notice to travel with the code.

## Published methods

| Source | Used for |
| --- | --- |
| Robert Bristow-Johnson, "Cookbook formulae for audio EQ biquad filter coefficients" | Biquad coefficient formulas (`af27-dsp`, `af27-fx`) |
| ITU-R BS.1770-4 | Integrated loudness and K-weighting |
| Paul Kellet's economy pink-noise filter | Pink noise generator |
| Howard Hinnant, "chrono-compatible low-level date algorithms" | Date formatting in `.audfrm` saves |

## Ported from Audiform by its author

The 3D Space panning and binaural model, the Phase Wheel, and the effects in
`af27-fx` were written by Audiform's author (nwh212) for Audiform and are
relicensed here by them. See `docs/clean-room.md`.
