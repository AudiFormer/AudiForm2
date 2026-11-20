# Attribution

## Code and data from other projects

| Work | Licence | Used in |
| --- | --- | --- |
| FFmpeg's DTS Coherent Acoustics core decoder (`dca_core.c`, `dcadsp.c`, `dcadct.c`, `synth_filter.c`, `dcadata.c`, `dcahuff.c`; foo86, Gildas Bazin and others) | LGPL-2.1-or-later | `crates/af27-dts` (a Rust port; the crate stays under the LGPL) |
| NeuralAmpModelerCore (Steven Atkinson) | MIT | `crates/af27-fx/src/nam.rs` follows its model layout and uses its fast tanh approximation, to play `.nam` amp captures |
| flacenc (Yotaro Kubo) | Apache-2.0 | The FLAC encoder behind `crates/af27-io/src/export/flac.rs` (a Cargo dependency, not copied code) |
| Freeverb (Jezar at Dreampoint) | Public domain | The comb and allpass delay tunings in `crates/af27-dsp/src/reverb.rs` |
| `asio-sys` (cpal project) | Apache-2.0 | ASIO driver access, only in builds with the `asio` feature |
| Steinberg ASIO SDK | GPL-3.0 (Steinberg's GPLv3 option) | Compiled in only with the `asio` feature. ASIO is a trademark and software of Steinberg Media Technologies GmbH. |
| `midly` (Negamartin) | Unlicense | Reading and writing `.mid` files (`crates/af27-interop/src/smf.rs`), as a crate dependency |
| `rustysynth` (Nobuaki Tanaka) | MIT | SoundFont instruments (`crates/af27-engine/src/soundfont.rs`), as a crate dependency; its MIT notice must travel with release builds |
| `embed-resource` (nabijaczleweli and others) | MIT | Builds the icon and version text into the Windows exe (`app/build.rs`); a build-time dependency only, nothing of it is in the program |
| `vst3` and `com-scrape-types` (Micah Johnston, coupler-rs) | MIT or Apache-2.0 | Hosting VST3 plug-ins (`crates/af27-vst3`), as crate dependencies. The bindings are generated from Steinberg's VST 3 SDK headers, which are MIT licensed since SDK 3.8. VST is a trademark of Steinberg Media Technologies GmbH; the VST logo is not used. VST2 is not supported |
| The `vst3` crate's `examples/gain.rs` (Micah Johnston) | MIT or Apache-2.0 | The skeleton of the test plug-in `crates/af27-vst3-testplug` (factory, string helpers, bus answers). Used by tests only, never shipped |
| `windows-sys` (Microsoft) | MIT or Apache-2.0 | Loading plug-in libraries and their windows (`crates/af27-vst3/src/host`), as a crate dependency |

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

## System fonts

Grey Desktop '95 sets its text in Windows' own Microsoft Sans Serif (or
Tahoma, with Tahoma Bold for window captions), read from the system's font
folder at run time. No font file is copied into AudiForm 2 or shipped with
it; elsewhere the theme falls back to egui's built-in fonts.

