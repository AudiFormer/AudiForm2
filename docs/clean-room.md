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
| `app/src/visualizer.rs` | `panels/visualizer/` (`VisualizerPanel.svelte`, `analysis.ts`, `palette.ts`, `presets/*.ts`; Oct 29) | Created clean (added, not copied), no AAE edits, not in Audiform's ATTRIBUTION. The spectrum feed is Audiform27's own (`af27-dsp` FFT), not Audiform's core |
| `crates/af27-dts` | `codecs/dts/` (Audiform 0.4.0) | Created clean; the decoder is an FFmpeg port, so the crate is LGPL |
| `app/src/separation.rs`, `crates/af27-separation` (model grouping and SDR text, engine checksum and update check, removal, GPU fallback, separated tracks) | `panels/separation/SeparationPanel.svelte`, `app/separation-tools.svelte.ts`, `app/separation.ts`, the Audio Separation page of `SettingsDialog.svelte`, `src-tauri/src/separation/{mod,download}.rs`, `commands/separation.rs` (Oct 28) | Each created clean in one commit (`git log -C --find-copies-harder`), no AAE edits, not in Audiform's ATTRIBUTION; behaviour rewritten for egui, no framework code |
| `crates/af27-dsp/src/wide.rs`: speaker grouping (`groups`, pairs by name, lone speakers, numbered channels by order), the ring order (`ring`), the per-pair copy wrapper with `vary(k)`, the loudest-channel key for linked dynamics, the latency-matched pass for channels left alone, and the STEREO_ONLY / LINKED / SKIP_LFE categories | `dsp/effects/wide.rs` (2ba7bd8 and later, all by Claude) | Created clean, no AAE edits, not in Audiform's ATTRIBUTION. Only the algorithm: rewritten against Audiform27's `Live` trait and `EffectSlot` (no `Processor`, `routing` or `build` from Audiform's AAE-derived framework); the speaker positions behind `ring` come from Audiform27's own table; the lists name Audiform27's effects (see docs/parity.md). Echo's bed form (`Echo::wide`, echo.rs) is not ported: Audiform27's own Echo runs through the generic pair wrapper |
| `crates/af27-fx`: the bed forms `Reverb::wide`, `ConvolutionReverb::wide`, `Modulation::wide` (with their `Bed`/`ConvBed`/`Decorrelate`/`Feed` parts) and `Processor::vary` | `dsp/effects/{verb,reverb,modulation}.rs` | Same files as the stereo ports above: created clean, no AAE edits (`git log --author=GeekCast` empty), not in ATTRIBUTION |

| Tempo map sums (`crates/af27-core/src/midi.rs`), piano roll behaviour (`app/src/pianoroll.rs`, `crates/af27-core/src/noteops.rs`) | `src/stores/tempo.ts`, `src/panels/editor/{PianoRollDialog.svelte,midi.ts}` (named by the project lead as the lead's own clean files) | Behaviour and formulas only (segments starting where the previous bars end, bar.beat.tick, the note edit set); rewritten in Rust against AudiForm 2's model, no framework code. `session/ops/midi.rs` was not used: it is written against an AAE-derived model |

## Effect windows ported from Audiform (2026-11-20)

The project lead asked for their own Audiform effect windows in AudiForm 2
("I did those"). Rule 6 normally keeps visual design our own; here the
design is the lead's, so layout, graphs, meters and interactions are
ported, rebuilt in Rust/egui (`app/src/fx_windows/`) and coloured from the
theme. No Svelte, CSS or TypeScript was copied, and no DSP: every window
drives AudiForm 2's own effects. Each Audiform file was checked on
`origin/main` (4b0dce4) with `git log --diff-filter=A --follow` (who created
it), `git log --follow --author=GeekCast` (must be empty) and
`git show --stat -C -C --find-copies-harder <creating commit>` (not copied or
moved), and against Audiform's `ATTRIBUTION.md`. The stray GeekCast header
line those files carry was not taken.

| AudiForm 2 | From Audiform (`src/panels/voice-chain/`) | Created by | Result |
| --- | --- | --- | --- |
| `fx_windows/mod.rs` `columns` (dial columns for every effect without its own face), `widgets::value_box` | `EffectControls.svelte`, `effect-controls.ts`, `ValueField.svelte` | af582cc, Claude | Created clean, no AAE edits, no copy, not in ATTRIBUTION: ported (the dial is AudiForm 2's own; Audiform's `Knob.svelte`/`knob.ts` and `chain.ts` were not used) |
| `fx_windows/phase.rs` (Phase Curve graph) | `PhaseCurveGraph.svelte`, `phase-curve.ts` | b13f804, Claude | Clean: ported. The curve itself is the already-ported `af27_fx` `curve_at`; the frequency ticks are our own (`graph.ts` was not used) |
| `fx_windows/space.rs` `verb` (Reverb faceplate, kinds, decay display) | `VerbPanel.svelte`, `reverb-kinds.ts` | e7ed223, Claude | Clean: ported (the kind tips; faceplate colours come from the theme, not Audiform's fixed colours; the Impulse Response tab is not ported) |
| `fx_windows/space.rs` `convolution` (Convolution Reverb faceplate, IR display, Load/Clear) | `ReverbPanel.svelte` | 81b7779, Claude | Clean: ported |
| `fx_windows/space.rs` `echo` (Delay faceplate driving AudiForm 2's own Echo: Time and Feedback dials round the repeats display) | `DelayPanel.svelte` | e7ed223, Claude | Clean GUI: ported. Audiform's delay DSP (`echo.rs`) is AAE's and is not used; AudiForm 2's Echo has no kinds, tempo or ping-pong, so those parts of the window are left out |
| `fx_windows/dynamics.rs` `opto`, `preamp` (big numbered dials, needle meter, lever switches) and `parts::needle` | `TubePanel.svelte`, `BigKnob.svelte`, `NeedleMeter.svelte` | 08c5c47, Claude | Clean: ported (needle scale and ballistics rebuilt; the brass faceplate is replaced by the theme) |
| `fx_windows/dynamics.rs` `vox` (three modules in signal order with power and style steppers) | `VoxStripPanel.svelte` | 9204329, Claude | Clean: ported (icons and the tone curve not ported) |

Refused or not used:

| Audiform file | Why |
| --- | --- |
| `src/panels/editor/Dial.svelte` | Created in 1b8b098, AAE's initial public release |
| `ResponseGraph.svelte`, `graph.ts`, `VoiceChainPanel.svelte`, `chain.ts` | Created in 850f0bf by GeekCast Radio Network (and edited by it) |
| `effect-window.ts` | Created in 25b189d by GeekCast Radio Network |
| `slot-drag.ts` | Edited by GeekCast (c29c24c) |
| `DynamicsCurveGraph.svelte`, `dynamics-curve.ts`, `curve-presets.ts` | Pass the git checks, but their commit (be4bd61) brought those effects in "from AAE"; left out to be safe (AudiForm 2 has no Dynamics Curve effect) |
| `LimiterPanel.svelte` | Clean, but its controls (Peak / True peak, Link) are not in AudiForm 2's Limiter; the Limiter got AudiForm 2's own unit instead |
| `AmpPanel`, `BandEqPanel`, `CenterPanel`, `MixerPanel`, `ModPanel`, `WidthPanel`, `NotchGraph`, `NoiseShapeGraph`, `EqGraph` | Clean, not ported in this pass (the Parametric EQ editor is separate work); those effects use the dial columns |

The Compressor, Limiter, Noise Gate and De-Esser units, the transfer graph
and the gain-reduction estimate are AudiForm 2's own design. The generator
windows (`app/src/generate_ui.rs`) and their DSP (`af27-dsp/src/generate.rs`)
are written fresh from the feature list of professional editors.

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
- `shaped.rs` (Shaped Noise Reducer): depends on AAE modules. (`wide.rs`
  is now ported as an algorithm only; see above.)
- `dynamics.rs`, `eq.rs`, `filters.rs`: edited by GeekCast commits, so
  nothing from them (including their multichannel handling) is used.
  Audiform27's Compressor, Limiter, Noise Gate and filters get their
  multichannel behaviour from Audiform27's own code (a level key in
  `af27-dsp::live`, the pair wrapper in `af27-dsp::wide`).
- VST3 hosting (`plughost/`): the lead's own work, but its tests were
  touched by AAE's merge. To be ported separately with those tests left out.

## Written fresh from a description

- Audio device I/O (`af27-engine`: `devices.rs`, `asio.rs`, `stream.rs`,
  `record.rs`, `routing.rs`, `health.rs`; the Audio Devices and Channel
  Routing settings pages). Written for Audiform27 from the project lead's
  handoff notes on how Audiform's device I/O behaved on real hardware
  (driver picking, ending streams on silence, stalls, channel maps). No
  code from Audiform or from its patched cpal was used. ASIO goes through
  upstream `cpal` and `asio-sys` (Apache-2.0) unchanged; see `asio.md`.
