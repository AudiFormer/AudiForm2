# Where the code comes from

Audiform27 is meant to be free of Altered Audio Editor (AAE) code, so that
AAE's GPL does not apply to it. Audiform began as a fork of AAE: its first
commit is AAE's own public release. That means nearly every Audiform file
is a derivative of AAE, including the parts later written for Audiform. Who
authored a commit does not change that.

## Rules

1. **No AAE code, ever.** Nothing is copied from a file that carries AAE's
   copyright line, or from a file that grew out of one.
2. **Audiform-only files may be ported** at the project lead's direction
   (2026-11-18). These are files that Audiform commits created from
   scratch, that carry no GeekCast Radio Network copyright line, and that
   git shows were not copied or moved from another file. The project lead
   is their author and can relicense them. When ported, only the algorithm
   is kept; anything tied to Audiform's framework (which is AAE-derived) is
   rewritten against Audiform27's own interfaces.
3. **Third-party code keeps its own licence.** Vendored copies of cpal and
   symphonia in Audiform are not reused. Audiform27 depends on the published
   crates.
4. **File formats are read from the files.** The `.sesx` and `.audfrm`
   readers were written from real session files on the project lead's
   machine and from their visible structure, not from any parser.
5. **Algorithms come from public descriptions**: RBJ biquads, ITU-R BS.1770
   loudness, Freeverb's public-domain tunings, windowed-sinc resampling,
   STFT spectral gating, and Paul Kellet's pink-noise filter.
6. **Behaviour, not appearance.** Features match professional editors in
   what they do. Visual design, icons and feature names are our own.

## What was ported from Audiform

| Audiform27 | From | Licence | Notes |
| --- | --- | --- | --- |
| `crates/af27-dts` | Audiform `src-tauri/src/codecs/dts/` (created in Audiform 0.4.0 by the project lead's sessions) | LGPL-2.1-or-later | The decoder core and tables are a port of FFmpeg's `dca_core.c` and its tables (LGPL-2.1-or-later, foo86, Gildas Bazin and others), so this crate stays LGPL. The test fixture `dts_51_14bit.wav` came from the same commit |
| `crates/af27-separation` (interface only) | The separation engine's job-file interface | Interface | No code copied. The engine is a separate program, and Audiform27 speaks its documented interface. The model list is factual data (URLs, hashes, specs) |

## Eligible for porting later

These Audiform files pass rule 2. They would each need their framework glue
rewritten:

- `dsp/effects/modulation.rs`: Chorus, Flanger, Phaser, Double Track
- `dsp/effects/verb.rs`: algorithmic reverb (Room, Hall, Plate, Spring, Tape)
- `dsp/effects/reverb.rs`: convolution reverb
- `dsp/effects/center.rs`: centre channel extractor
- `dsp/effects/vocal.rs`: one-knob vocal strip
- `dsp/effects/width.rs`, `dsp/effects/wide.rs`: stereo width and surround handling
- `codecs/source.rs`, `aup3/mod.rs` (Audacity project import), `engine/reverse.rs`

`dsp/biquad.rs` and the effect framework in `dsp/effects/mod.rs` carry
AAE's copyright, so they must not be ported.
