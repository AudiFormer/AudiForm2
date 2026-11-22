# Attribution

## Code and data from other projects

| Work | Licence | Used in |
| --- | --- | --- |
| FFmpeg's DTS Coherent Acoustics core decoder (`dca_core.c`, `dcadsp.c`, `dcadct.c`, `synth_filter.c`, `dcadata.c`, `dcahuff.c`; foo86, Gildas Bazin and others) | LGPL-2.1-or-later | `crates/af2-dts` (a Rust port; the crate stays under the LGPL) |
| NeuralAmpModelerCore (Steven Atkinson) | MIT | `crates/af2-fx/src/nam.rs` follows its model layout and uses its fast tanh approximation, to play `.nam` amp captures |
| flacenc (Yotaro Kubo) | Apache-2.0 | The FLAC encoder behind `crates/af2-io/src/export/flac.rs` (a Cargo dependency, not copied code) |
| Freeverb (Jezar at Dreampoint) | Public domain | The comb and allpass delay tunings in `crates/af2-dsp/src/reverb.rs` |
| `asio-sys` (cpal project) | Apache-2.0 | ASIO driver access, only in builds with the `asio` feature |
| Steinberg ASIO SDK | GPL-3.0 (Steinberg's GPLv3 option) | Compiled in only with the `asio` feature. ASIO is a trademark and software of Steinberg Media Technologies GmbH. |
| `midly` (Negamartin) | Unlicense | Reading and writing `.mid` files (`crates/af2-interop/src/smf.rs`), as a crate dependency |
| `rustysynth` (Nobuaki Tanaka) | MIT | SoundFont instruments (`crates/af2-engine/src/soundfont.rs`), as a crate dependency; its MIT notice must travel with release builds |
| `embed-resource` (nabijaczleweli and others) | MIT | Builds the icon and version text into the Windows exe (`app/build.rs`); a build-time dependency only, nothing of it is in the program |
| `vst3` and `com-scrape-types` (Micah Johnston, coupler-rs) | MIT or Apache-2.0 | Hosting VST3 plug-ins (`crates/af2-vst3`), as crate dependencies. The bindings are generated from Steinberg's VST 3 SDK headers, which are MIT licensed since SDK 3.8. VST is a trademark of Steinberg Media Technologies GmbH; the VST logo is not used. VST2 is not supported |
| The `vst3` crate's `examples/gain.rs` (Micah Johnston) | MIT or Apache-2.0 | The skeleton of the test plug-in `crates/af2-vst3-testplug` (factory, string helpers, bus answers). Used by tests only, never shipped |
| `windows-sys` (Microsoft) | MIT or Apache-2.0 | Loading plug-in libraries and their windows (`crates/af2-vst3/src/host`), as a crate dependency |

**To do before any release:** copy NeuralAmpModelerCore's MIT notice verbatim from its repository's LICENSE file into this section. The MIT licence requires that notice to travel with the code.

## Published methods

| Source | Used for |
| --- | --- |
| Robert Bristow-Johnson, "Cookbook formulae for audio EQ biquad filter coefficients" | Biquad coefficient formulas (`af2-dsp`, `af2-fx`) |
| ITU-R BS.1770-4 | Integrated loudness and K-weighting |
| Paul Kellet's economy pink-noise filter | Pink noise generator |
| Howard Hinnant, "chrono-compatible low-level date algorithms" | Date formatting in `.audfrm` saves |

## Ported from Audiform by its author

The 3D Space panning and binaural model, the Phase Wheel, and the effects in
`af2-fx` were written by Audiform's author (AudiFormer) for Audiform and are
relicensed here by them. See `docs/clean-room.md`.

## System fonts

Grey Desktop '95 sets its text in Windows' own Microsoft Sans Serif (or
Tahoma, with Tahoma Bold for window captions), read from the system's font
folder at run time. No font file is copied into AudiForm 2 or shipped with
it; elsewhere the theme falls back to egui's built-in fonts.

## Decoding help

| Part | Licence | Use |
| --- | --- | --- |
| `symphonia-codec-vorbis` 0.5.5 (Project Symphonia), in `vendor/` | MPL-2.0 | One change: Ogg Vorbis with more than 8 channels (multitrack `.mogg`) decodes instead of being refused. See `vendor/README.md` |
| Windows Media Foundation | Part of Windows | `crates/af2-osdecode` asks Windows' own decoders for Dolby Digital, Dolby Digital Plus, HE-AAC and AC-4. Nothing of theirs is shipped. Dolby, Dolby Atmos and AC-4 are trademarks of Dolby Laboratories |
| ffmpeg | Not shipped | Used only if the user has it installed, as a separate program, for files nothing else opens |
