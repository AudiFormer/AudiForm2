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
