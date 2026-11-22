# Where the code comes from

Audiform2 must contain no Altered Audio Editor (AAE) code. Audiform began
as a fork of AAE: its first commit is AAE's own public release, so most of
Audiform's files are AAE derivatives, including parts later written for
Audiform inside those files. Who wrote a commit does not change that. This
file is the record of how every part of Audiform2 was sourced.

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
   which is AAE-derived, is rewritten against Audiform2's own interfaces.
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

| Audiform2 | From Audiform | Checked |
| --- | --- | --- |
| `crates/af2-space` (`pan.rs`, `binaural.rs`) | `dsp/space.rs`, `dsp/binaural.rs` (Oct 25, the lead's sessions) | Created clean, no AAE edits |
| `crates/af2-dsp/src/phase.rs`, `crates/af2-engine/src/scope.rs`, `app/src/phasewheel.rs` | `engine/scope.rs`, `panels/phase/PhaseWheelPanel.svelte`, `app/colors.ts` (Oct 24, nashhill2001) | Created clean, no AAE edits; came in with the lead's `feat/buses-sends-sidechain` |
| `crates/af2-fx`: Amp Model and `.nam` player, Tube Preamp, Opto Leveler, Vox Strip, Reverb, Convolution Reverb, Modulation, Center Channel Extractor, Channel Mixer, Stereo Width, Phase Curve | `dsp/effects/{amp,tube,vocal,verb,reverb,modulation,center,mixer,width,phase_curve}.rs`, `dsp/{nam,convolve,meters}.rs` | Created clean, no AAE edits, not in Audiform's ATTRIBUTION |
| `app/src/visualizer.rs` | `panels/visualizer/` (`VisualizerPanel.svelte`, `analysis.ts`, `palette.ts`, `presets/*.ts`; Oct 29) | Created clean (added, not copied), no AAE edits, not in Audiform's ATTRIBUTION. The spectrum feed is Audiform2's own (`af2-dsp` FFT), not Audiform's core |
| `crates/af2-dts` | `codecs/dts/` (Audiform 0.4.0) | Created clean; the decoder is an FFmpeg port, so the crate is LGPL |
| `app/src/separation.rs`, `crates/af2-separation` (model grouping and SDR text, engine checksum and update check, removal, GPU fallback, separated tracks) | `panels/separation/SeparationPanel.svelte`, `app/separation-tools.svelte.ts`, `app/separation.ts`, the Audio Separation page of `SettingsDialog.svelte`, `src-tauri/src/separation/{mod,download}.rs`, `commands/separation.rs` (Oct 28) | Each created clean in one commit (`git log -C --find-copies-harder`), no AAE edits, not in Audiform's ATTRIBUTION; behaviour rewritten for egui, no framework code |
| `crates/af2-dsp/src/wide.rs`: speaker grouping (`groups`, pairs by name, lone speakers, numbered channels by order), the ring order (`ring`), the per-pair copy wrapper with `vary(k)`, the loudest-channel key for linked dynamics, the latency-matched pass for channels left alone, and the STEREO_ONLY / LINKED / SKIP_LFE categories | `dsp/effects/wide.rs` (2ba7bd8 and later, all by Claude) | Created clean, no AAE edits, not in Audiform's ATTRIBUTION. Only the algorithm: rewritten against Audiform2's `Live` trait and `EffectSlot` (no `Processor`, `routing` or `build` from Audiform's AAE-derived framework); the speaker positions behind `ring` come from Audiform2's own table; the lists name Audiform2's effects (see docs/parity.md). Echo's bed form (`Echo::wide`, Audiform's echo.rs) is not ported: the built-in Echo runs through the generic pair wrapper, and `af.echo`'s own bed form is written fresh |
| `app/src/prerender.rs` (Pre-render: fingerprint, background render, cached renders, playing the render in place of the clips and effects) | `src-tauri/src/commands/prerender.rs` (9204329 and 44e6472, Oct 27-28) | Created clean in the lead's own commits, no AAE edits (`git log --author=GeekCast` empty), not in Audiform's ATTRIBUTION. Behaviour only: rewritten against Audiform2's session model and engine |
| `crates/af2-fx`: the bed forms `Reverb::wide`, `ConvolutionReverb::wide`, `Modulation::wide` (with their `Bed`/`ConvBed`/`Decorrelate`/`Feed` parts) and `Processor::vary` | `dsp/effects/{verb,reverb,modulation}.rs` | Same files as the stereo ports above: created clean, no AAE edits (`git log --author=GeekCast` empty), not in ATTRIBUTION |

| Tempo map sums (`crates/af2-core/src/midi.rs`), piano roll behaviour (`app/src/pianoroll.rs`, `crates/af2-core/src/noteops.rs`) | `src/stores/tempo.ts`, `src/panels/editor/{PianoRollDialog.svelte,midi.ts}` (named by the project lead as the lead's own clean files) | Behaviour and formulas only (segments starting where the previous bars end, bar.beat.tick, the note edit set); rewritten in Rust against AudiForm 2's model, no framework code. `session/ops/midi.rs` was not used: it is written against an AAE-derived model |

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
| `fx_windows/phase.rs` (Phase Curve graph) | `PhaseCurveGraph.svelte`, `phase-curve.ts` | b13f804, Claude | Clean: ported. The curve itself is the already-ported `af2_fx` `curve_at`; the frequency ticks are our own (`graph.ts` was not used) |
| `fx_windows/space.rs` `verb` (Reverb faceplate: kind tabs as lit keys with Impulse Response last, Size and Decay dials round the decay display, the groups two to a row), `to_impulse`/`from_impulse`, and the slot switch (`Fx::new_effect`, taken by `fx_windows/mod.rs` `finish` as one undo step) | `VerbPanel.svelte`, `reverb-kinds.ts`, `faceplate.css` | e7ed223 (`faceplate.css` 7790ccd), Claude; changed only in the lead's commits e4c5330, 7790ccd and b68651b | Clean: created clean, `git log --author=GeekCast` empty for all three. Ported: the layout, the kind tips, the settings carried between the Reverb and the Convolution Reverb, and each kind's face, face2, ink and glow colours, which are blended into the theme (mostly the kind's on a dark theme, a tint on a light one) rather than used as fixed colours |
| `fx_windows/space.rs` `convolution` (Convolution Reverb faceplate: the same kind tabs, Impulse Response lit, to switch back to a made kind; Load and Clear keys; the IR display with Length and Pre-delay marks; the dial groups) | `ReverbPanel.svelte`, `reverb-kinds.ts`, `faceplate.css` | 81b7779, Claude; changed only in the lead's commits e7ed223, e4c5330, 7790ccd and b68651b | Clean: `git log --author=GeekCast` empty. Ported, with its own faceplate colours blended into the theme as for the Reverb's kinds. The IR's shape is read by `af2_fx` (`reverb::read`), not by Audiform's `reverbLoad` |
| `fx_windows/space.rs` `echo` (Delay faceplate driving AudiForm 2's own Echo: Time and Feedback dials round the repeats display) | `DelayPanel.svelte` | e7ed223, Claude | Clean GUI: ported. Audiform's delay DSP (`echo.rs`) is AAE's and is not used; AudiForm 2's Echo has no kinds, tempo or ping-pong, so those parts of the window are left out |
| `fx_windows/delay.rs` (Delay and Echo faceplate for `af.echo`: kind tabs, Tempo and Ping-pong, Time and Feedback round the repeats display, Use session BPM, Modulation / Filters / Machine / Room / Output groups, Simple's Left and Right groups, the kinds' faceplate colours from `faceplate.css`) | `DelayPanel.svelte`, `faceplate.css` | e7ed223, 8b8cedf, bea4d08, 7790ccd, b68651b, 4c74039 (the lead's commits) | Clean GUI, no AAE edits: layout and behaviour ported (no Svelte or CSS copied) |
| `crates/af2-fx/src/effects/echo.rs` (`af.echo`, Delay and Echo: Digital, Tape, Analog, Simple; its bed form) | `src-tauri/src/dsp/effects/echo.rs`, its control list only (lines 31-48 and 69-100: keys, ranges, defaults, pick lists) | GeekCast header | The control list was read as a format description so `.audfrm` `altaudio.echo` slots load and save; the note lengths come from the note names. No DSP was read or used: the sound code (and the description text) was written fresh for AF2 from a behaviour description |
| `fx_windows/dynamics.rs` `opto`, `preamp` (big numbered dials, needle meter, lever switches) and `parts::needle` | `TubePanel.svelte`, `BigKnob.svelte`, `NeedleMeter.svelte` | 08c5c47, Claude | Clean: ported (needle scale and ballistics rebuilt; the brass faceplate is replaced by the theme) |
| `fx_windows/dynamics.rs` `vox` (the heading with the Low Cut switch; three numbered modules in signal order, each with a power light, a style stepper with the style's line icon and place dots, its main controls and a trim; GR and DS bars, the Tone curve and its four faders with value boxes; the Input and Output dials round the output ladder) and `parts::icon`/`path_lines` | `VoxStripPanel.svelte` | 9204329, Claude; changed only in the lead's commits 2362872 and feb960f | Clean: `git log --author=GeekCast` empty. Ported, including the style icons' path data (drawn by our own path reader). The Tone curve is AudiForm 2's own `af2_fx` `vocal::tone_curve`, not Audiform's `voxToneCurve`; the GR and DS bars are estimated from the rack's input level and the style's curve, as Audiform's `effectMeter` reading is not plumbed to the app. Colours come from the theme |
| `crates/af2-fx/src/effects/dynamics.rs` (`af.dynamics`, `af.limiter`, `af.deesser`, `af.gain`: Dynamics, Limiter, De-Esser, Gain) | `src-tauri/src/dsp/effects/dynamics.rs` lines 20-112 and the Gain entry in `dsp/effects/mod.rs`, control lists only (keys, names, units, ranges, defaults, descriptions) | GeekCast header; AAE-made effects, edited by GeekCast commits | The control lists were read as a format description so `.audfrm` `altaudio.dynamics`, `altaudio.limiter`, `altaudio.deesser` and `altaudio.gain` slots load and save. No DSP was read or used: the sound was written fresh from textbook methods (a soft-knee compressor with attack and release on the reduction; a look-ahead limiter with a windowed minimum and a box average, cubic true-peak estimate; a Linkwitz-Riley split de-esser; an eased gain) |
| `fx_windows/units.rs` `amp`, `preamp`, `opto`, `limiter` (the Amp Model faceplate with its grille band, capture and cabinet rows, LED switches and dials; the Tube Preamp and Opto Leveler faceplates with big numbered dials, the cream needle meter and lever switches; the Limiter's Peak/True peak choice, numbered slider rows and Link channels) and `af2_fx` `amp::summary`/`with_model`/`with_ir`/`without` | `AmpPanel.svelte`, `amp.ts`, `TubePanel.svelte`, `NeedleMeter.svelte`, `BigKnob.svelte`, `LimiterPanel.svelte`, `SliderRow.svelte` | Created by Claude in the lead's commits (08c5c47, da441a6, e7ed223 and later), `git log --follow --author=GeekCast` empty for each; the GeekCast line in some headers is the stray template line | Ported: layout, wording, notes and the faceplates' colours, which are blended into the theme. The needle is fed by the level worked out from the rack's meters, as Audiform's `effectMeter` reading is not plumbed to the app. Loading reads files with `af2_io` and `nam::parse_value`, not Audiform's commands |
| `fx_windows/modulation.rs` (Modulation: kind tabs in their faceplate colours, Rate and Depth round the movement display, each kind's own dials, the Double Track strips, copy-position display, Offset and LFO, kind start settings) | `ModPanel.svelte`, `modulation-kinds.ts`, `faceplate.css` | Claude (no GeekCast commits) | Clean GUI: layout and behaviour ported (2026-11-21); AF2's own knobs (BigKnob is AAE-headed and was not used) |
| `fx_windows/center.rs` (Center Channel Extractor: the top-down stage with its draggable light, Center and Side faders, Extraction and Discrimination tabs, Advanced) | `CenterPanel.svelte`, `faceplate.css` | Claude (no GeekCast commits) | Clean GUI: ported (2026-11-21); AF2's own faders (VSlider is AAE-headed and was not used) |
| `fx_windows/mixer.rs` (Channel Mixer: L and R output tabs, Input Channel / Invert rows) | `MixerPanel.svelte` | Claude (no GeekCast commits) | Clean GUI: ported (2026-11-21) |

Refused or not used:

| Audiform file | Why |
| --- | --- |
| `src/panels/editor/Dial.svelte` | Created in 1b8b098, AAE's initial public release |
| `ResponseGraph.svelte`, `graph.ts`, `VoiceChainPanel.svelte`, `chain.ts` | Created in 850f0bf by GeekCast Radio Network (and edited by it) |
| `effect-window.ts` | Created in 25b189d by GeekCast Radio Network |
| `slot-drag.ts` | Edited by GeekCast (c29c24c) |
| `DynamicsCurveGraph.svelte`, `dynamics-curve.ts`, `curve-presets.ts` | Pass the git checks, but their commit (be4bd61) brought those effects in "from AAE"; left out to be safe (AudiForm 2 has no Dynamics Curve effect) |
| `LimiterPanel.svelte` | Clean, but its controls (Peak / True peak, Link) are not in AudiForm 2's Limiter; the Limiter got AudiForm 2's own unit instead |
| `AmpPanel`, `BandEqPanel`, `WidthPanel`, `NotchGraph`, `NoiseShapeGraph`, `EqGraph` | Clean, not ported in this pass (the Parametric EQ editor is separate work); those effects use the dial columns |

The Compressor, Limiter, Noise Gate and De-Esser units, the transfer graph
and the gain-reduction estimate are AudiForm 2's own design. The generator
windows (`app/src/generate_ui.rs`) and their DSP (`af2-dsp/src/generate.rs`)
are written fresh from the feature list of professional editors.
| `fx_windows/band_eq.rs` (Band EQ window: 10 / 20 / 30 band tabs, a new count keeping the curve's shape, one upright fader per band with its level above and centre below, Output on the right, double-click for 0 dB, Flat, draw a shape across the bands) and `af2_fx` `band_eq::carry` | `BandEqPanel.svelte`, `band-eq.ts` | 8d7b0c8, b904e89 and later, all Claude | Clean: created clean (`git show -C -C --find-copies-harder`), `git log --follow --author=GeekCast` empty, not in ATTRIBUTION. Behaviour ported; the response curve above the faders is new. The sound is not ported: Audiform's Band EQ DSP (`dsp/effects/filters.rs`) carries AAE's header, so `crates/af2-fx/src/effects/band_eq.rs` was written fresh from the behaviour (band counts, ISO centres, ±15 dB, Output) with cookbook peaking sections and the standard bandwidth-to-Q formula |

## Deliberately left out

- **Delay and Echo** (`dsp/effects/echo.rs`): Audiform's `ATTRIBUTION.md`
  lists it as brought in from AAE 0.4.1 and 0.5.0 under the GPL. The Tape
  and Analog modes the lead added later are built on that AAE core, so the
  whole file stays out. Audiform2 has its own Echo in `af2-dsp`.
- The effect framework (`dsp/effects/mod.rs`), `dsp/biquad.rs` and the
  AAE-made effects (Dynamics, Limiter, Breath Gate, De-Esser, Speech
  Leveler, the EQs and filters, the noise, hiss and room reducers, click
  and clip repair, pitch, stretch, spectral). Where Audiform2 has these
  features, they are written fresh.
- `shaped.rs` (Shaped Noise Reducer): depends on AAE modules. (`wide.rs`
  is now ported as an algorithm only; see above.)
- `dynamics.rs`, `eq.rs`, `filters.rs`: edited by GeekCast commits, so
  nothing from them (including their multichannel handling) is used.
  Audiform2's Compressor, Limiter, Noise Gate and filters get their
  multichannel behaviour from Audiform2's own code (a level key in
  `af2-dsp::live`, the pair wrapper in `af2-dsp::wide`).
- VST3 hosting (`plughost/`): the lead's own work, but its tests were
  touched by AAE's merge. To be ported separately with those tests left out.

## Written fresh from a description

- Audio device I/O (`af2-engine`: `devices.rs`, `asio.rs`, `stream.rs`,
  `record.rs`, `routing.rs`, `health.rs`; the Audio Devices and Channel
  Routing settings pages). Written for Audiform2 from the project lead's
  handoff notes on how Audiform's device I/O behaved on real hardware
  (driver picking, ending streams on silence, stalls, channel maps). No
  code from Audiform or from its patched cpal was used. ASIO goes through
  upstream `cpal` and `asio-sys` (Apache-2.0) unchanged; see `asio.md`.
- Parametric EQ (`af2-fx/src/effects/parametric_eq.rs`): the filters are
  written from the public Audio EQ Cookbook formulas and the Butterworth
  pole angles, on Audiform2's own `biquad.rs`. Audiform's `eq.rs` was not
  read. Only its saved parameter keys (`altaudio.parametric_eq`, as the
  `.sesx` importer and the session schema name them) are matched, so
  sessions carry over.
- The Parametric EQ's graph editor (`app/src/eq_editor.rs`): the design
  and maths (node and width handle behaviour, the Q/octave formulas, the
  frequency ruler, the range switch, the band table) follow Audiform's
  `eq-graph.ts`, `EqGraph.svelte` and `graph.ts`, files the lead's own
  sessions created (commits 36d74db and 8a61a53) and AAE never edited.
  They are rewritten in Rust/egui, with no code or headers copied.
- Speech and restoration effects (`af2-fx/src/effects/speech.rs`,
  `restore.rs`; finders in `af2-dsp/src/repair.rs`), 2026-11-20: Speech
  Leveler, Breath Gate, Click Repair, Clip Repair, Noise Reducer, Hiss
  Reducer and Room Reducer. Their processing, descriptions and code are
  new, written from textbook methods: a second-difference click detector
  with a median scale, cubic Hermite fills, minimum-tracking spectral
  noise estimation, and late-reverberation prediction from a decayed
  past spectrum. So that Audiform sessions' voice chains load, only the
  interface was matched: the slot ids (`altaudio.<name>`) with each
  control's key, range and default. These were read from the effect
  description blocks in Audiform's `dynamics.rs` and `restore.rs`. None of
  their processing code was read or used. The lead should confirm that
  matching these names and numbers is acceptable, as was done for the
  Parametric EQ's keys.
- Diagnostics, Loudness panel, Auto Duck, Strip Silence, Speaker Balance,
  marker kinds and chapter export (2026-11-20): written for AudiForm 2 from
  public descriptions of how these tools behave (see
  `docs/podcast-features.md`). The `.audfrm` marker `kind` names (`cue`,
  `chapter`, `ad_slot`, `social_clip`, `range`) are file-format names. The
  podcast chapters JSON is the open 1.2.0 format.
- Pitch Shift, Phase Rotate and Center Remover (`af2-fx/src/effects/`
  `pitch.rs`, `phase_rotate.rs`, `center_remover.rs`, 2026-11-21): Audiform's
  versions live in AAE-headed files (`pitch.rs`, `phase.rs`, `filters.rs`),
  so they were re-implemented from behaviour only. One agent read
  Audiform's files and wrote a plain-language description: the controls
  (keys, ranges, defaults, so `altaudio.*` slots load), what each does,
  and its frame sizes and smoothing times. A separate author who had not
  read those files wrote the code from that description, using standard
  public techniques: a phase vocoder with identity phase locking and
  per-frame cubic read-back for the pitch, a square-root-Hann STFT phase
  turn, and the mid minus its Butterworth low-pass. Phase Rotate and the
  Center Remover are kept for sessions but not offered, as in Audiform.
  The lead should confirm this is acceptable.
- The Reverb's Studio kind (2026-11-21): the Studio Reverb (`studio.rs`,
  tuned from the lead's recordings, nothing taken from anywhere) is now
  the Reverb's sixth kind and default, sounding sample for sample as
  before (tested). Its window is `fx_windows/studio.rs` under the Reverb's
  kind tabs.
- Delay and Echo was not changed. Its sound differs from Audiform's, and
  Audiform's `echo.rs` is AAE's (see "Deliberately left out"). On
  2026-11-21 one author read that file's processing while comparing the
  two, then reverted their own change to `af2-fx` `echo.rs`, so it is
  exactly as it was. Any future move toward Audiform's sound must come
  from an author who has not read that file, working from measurements
  or a description.
- The filters (2026-11-21): Curve EQ, Precision Filter, Notch Filter, Hum
  Cut and DC Offset Fix (`af2-fx/src/effects/curve_eq.rs`, `filters.rs`).
  Audiform's versions live in AAE-headed files (`dsp/effects/eq.rs`,
  `filters.rs`, `restore.rs`, `dsp/elliptic.rs`), so they were
  re-implemented from behaviour only. The filters session read those
  files and wrote a plain-language description (controls as a file
  format so `altaudio.*` slots load, and what each does in public
  textbook terms: RBJ cookbook sections, Butterworth/Chebyshev I/Bessel
  sections, Orfanidis' elliptic method, a smoothstep curve applied in a
  square-root-Hann STFT, the one-pole DC blocker). A first draft that the
  same session had written after reading those files was set aside
  unused; a separate author who had not read them wrote the code from the
  description. Their windows (`app/src/fx_windows/filters.rs`): the
  Notch Filter's graph and table port the lead's `NotchGraph.svelte`
  (ba16c79, Claude; `git log --author=GeekCast` empty, not copied); the
  response graph over the Curve EQ, Precision Filter and Hum Cut controls
  recreates what Audiform's `ResponseGraph.svelte`/`graph.ts` (AAE files)
  show and do: no code taken, but written by the filters session, which
  had read those two files (UI behaviour only: a log axis 20 Hz-20 kHz,
  lines at 100/1k/10k, a +-24 dB range opened out to the dB controls and
  deep cuts, draggable points). If the lead wants the two-author rule
  here too, that graph should be redone from this description. The Parametric EQ's
  shelf Q now starts at 1, Audiform's fixed shelf Q, so its sessions'
  shelves sound as they did. The lead should confirm this is acceptable.
