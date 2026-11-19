# Where the code comes from

Audiform27 must contain no Altered Audio Editor (AAE) code. Audiform began
as a fork of AAE: its first commit is AAE's own public release, so most of
Audiform's files are AAE derivatives, including parts later written for
Audiform inside those files. Who wrote a commit does not change that. This
file is the record of how every part of Audiform27 was sourced.

## Rules

1. **No AAE code, ever.** Nothing is copied from a file that carries AAE's
   copyright line *because AAE wrote it*, or from a file that grew out of
   one, or from anything Audiform's own `ATTRIBUTION.md` lists as taken
   from AAE.
2. **The project lead's own Audiform work may be ported** (their decision,
   2026-11-18). A file qualifies when all of these hold:
   - an Audiform commit created it from scratch (no copy or move from
     another file, checked with `git log -C --find-copies-harder`);
   - AAE never edited it afterwards (`git log --author=GeekCast`);
   - it is not listed in Audiform's `ATTRIBUTION.md` as taken from AAE.

   Several such files carry a stray GeekCast header line from a template.
   Where the project lead contributed work upstream to AAE, AAE's
   contributor agreement says "You keep your copyright" and grants AAE only
   a non-exclusive licence, so the project lead can relicense it.
   Only the algorithm is ported. Anything tied to Audiform's framework,
   which is AAE-derived, is rewritten against Audiform27's own interfaces.
3. **Third-party code keeps its own licence.** See `ATTRIBUTION.md`.
4. **File formats are read from the files.** The `.sesx` reader was written
   from real session files. The `.audfrm` reader and writer were written
   from real Audiform sessions and Audiform's published JSON Schema
   (`schemas/session.schema.json`), used as a format description. No
   parser code was used. Reading and writing a file format is
   interoperability, not copying.
5. **Algorithms come from public descriptions.** See `ATTRIBUTION.md`.
6. **Behaviour, not appearance.** Features match professional editors in
   what they do. Visual design, icons and feature names are our own.

## Ported from Audiform

| Audiform27 | From Audiform | Checked |
| --- | --- | --- |
| `crates/af27-space` (`pan.rs`, `binaural.rs`) | `dsp/space.rs`, `dsp/binaural.rs` (Oct 25, the lead's sessions) | Created clean, no AAE edits |
| `crates/af27-dsp/src/phase.rs`, `crates/af27-engine/src/scope.rs`, `app/src/phasewheel.rs` | `engine/scope.rs`, `panels/phase/PhaseWheelPanel.svelte`, `app/colors.ts` (Oct 24, nashhill2001) | Created clean, no AAE edits; came in with the lead's `feat/buses-sends-sidechain` |
| `crates/af27-fx`: Amp Model and `.nam` player, Tube Preamp, Opto Leveler, Vox Strip, Reverb, Convolution Reverb, Modulation, Center Channel Extractor, Channel Mixer, Stereo Width, Phase Curve | `dsp/effects/{amp,tube,vocal,verb,reverb,modulation,center,mixer,width,phase_curve}.rs`, `dsp/{nam,convolve,meters}.rs` | Created clean, no AAE edits, not in Audiform's ATTRIBUTION |
| `crates/af27-dts` | `codecs/dts/` (Audiform 0.4.0) | Created clean; the decoder is an FFmpeg port, so the crate is LGPL |
| `app/src/separation.rs`, `crates/af27-separation` (model grouping and SDR text, engine checksum and update check, removal, GPU fallback, separated tracks) | `panels/separation/SeparationPanel.svelte`, `app/separation-tools.svelte.ts`, `app/separation.ts`, the Audio Separation page of `SettingsDialog.svelte`, `src-tauri/src/separation/{mod,download}.rs`, `commands/separation.rs` (Oct 28) | Each created clean in one commit (`git log -C --find-copies-harder`), no AAE edits, not in Audiform's ATTRIBUTION; behaviour rewritten for egui, no framework code |

## Deliberately left out

- **Delay and Echo** (`dsp/effects/echo.rs`): Audiform's `ATTRIBUTION.md`
  lists it as brought in from AAE 0.4.1 and 0.5.0 under the GPL. The Tape
  and Analog modes the lead added later are built on that AAE core, so the
  whole file stays out. Audiform27 has its own Echo in `af27-dsp`.
- The effect framework (`dsp/effects/mod.rs`), `dsp/biquad.rs` and the
  AAE-made effects (Dynamics, Limiter, Breath Gate, De-Esser, Speech
  Leveler, the EQs and filters, the noise, hiss and room reducers, click
  and clip repair, pitch, stretch, spectral). Where Audiform27 has these
  features, they are written fresh.
- `shaped.rs` (Shaped Noise Reducer) and `wide.rs`: both depend on AAE
  modules.
- VST3 hosting (`plughost/`): the lead's own work, but its tests were
  touched by AAE's merge. To be ported separately with those tests left out.
