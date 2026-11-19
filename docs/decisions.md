# Decisions

## 2026-11-18: project start

- **Clean room, MIT OR Apache-2.0.** Decided by the project lead, so that
  Audiform27 is free of Altered Audio Editor's GPL. See `clean-room.md`.
- **Audiform-only files may be ported** (project lead, 2026-11-18). Only
  files Audiform created from scratch, with no AAE copyright line.
- **The DTS decoder stays LGPL-2.1** in its own crate (`af27-dts`), because
  it is a port of FFmpeg's. Everything else is permissive.
- **All Rust UI: egui + egui_dock.** egui gives full control over drawing
  (waveforms, spectrograms, glow), so the theme reaches every pixel.
  egui_dock gives drag, tab, split and float docking for every panel.
- **Themeable at the base.** Every colour, gradient, spectrogram palette,
  corner radius and glow strength comes from `Theme`. Partial theme files
  fill in from the default, so a theme can change just a few colours.
- **Snapshot undo.** Each Waveform edit keeps the previous buffer (shared
  through `Arc`). This is simple and safe. Memory use for very long files
  is a known later item (range-based undo).
- **Positions in seconds** in the multitrack session, so sources of any
  sample rate mix without conversion first. Playback resamples on the fly.
- **Real-time rule.** The audio thread never allocates, locks or frees.
  Programs arrive over `rtrb` and are dropped on the UI thread.
- **AI stems share Audiform's engine and models.** Audiform27 looks in its
  own folder, then in Audiform's (`io.github.nwh212.audiform/separation`),
  so a user who already downloaded gigabytes of models does not do it
  again. Extra models can be described in `models.json` in either folder.
- **Encrypted `.mogg` files are refused**, not decrypted.
- **Plain mouse wheel zooms** over the editors (at the pointer). Shift+wheel
  scrolls in time; wheel over the track headers scrolls tracks.

## 2026-11-19: Audiform features, compatibility and licence

- **`.audfrm` is the session format** (project lead: compatibility comes
  first). It is written losslessly: each object keeps its id from the
  file, and saving writes only Audiform27's fields over the original
  object, so automation, stretch, routing and future fields survive.
  This is better than copying Audiform's internal model into Audiform27:
  the new engine stays free of old limits, and nothing AAE-derived comes
  in. Checked against all 75 of the lead's sessions: byte-identical
  after a load and save.
- **Audiform27-only data in `.audfrm`**: its own effects are saved as
  `af27.<id>` slots (Audiform skips them when playing but keeps them when
  it saves). The synth settings go in `af27_synth` on the track.
- **The project lead's 3D Space, Phase Wheel and effects are ported.**
  Provenance was checked file by file (`clean-room.md`). Delay and Echo
  was dropped: Audiform's own records list it as AAE's.
- **Live edits.** Fader, pan, mute and solo, effect settings, bypass, 3D
  positions and instrument settings go to the audio thread as fixed-size
  commands. Only structural changes rebuild the mix. Each drag is one
  undo step.
- **Licence changed to the Audiform27 License 1.0** (project lead): MIT
  freedoms for everyone except GeekCast Radio Network, LLC and anyone
  acting for it. It is visible, not hidden, because a hidden term is not
  enforceable. `af27-dts` stays LGPL, which forbids added restrictions.
  The lead's earlier upstream contributions were licensed to AAE under its
  agreement; this licence does not and cannot change that.

## 2026-11-19: AudiForm 2, performance

- **Renamed AudiForm 2** (project lead). Visible name, logo and licence
  title. Crate names, file extensions and the settings folder keep
  `af27`/`Audiform27` so nothing saved is lost.
- **Parallel engine.** A pool of worker threads (one fewer than the
  cores, at most 15) driven by the audio thread with atomics only: no
  locks or allocation, unlike rayon (which Audiform uses), whose
  injection from a foreign thread takes a lock. Workers spin 300 us after
  a block and then park, so an idle or lightly loaded engine does not
  burn cores. Parallel and serial mixes are bit-identical (tested).
- **Internal block raised to 2048 frames** (as Audiform): fewer handoffs
  per second; a device callback of any size still works.
- **wgpu renderer with OpenGL fallback.** OpenGL showed a white window on
  the lead's PC.
- **Visual Effects Auto/Full/Lite.** Auto watches real frame pacing and
  drops to Lite on slow devices.
