# Audiform27

A fast, themeable audio editor written entirely in Rust. It's the sequel to
[Audiform](https://github.com/nwh212/Audiform), rebuilt from scratch.

Audiform27 covers the full professional editing workflow: a destructive
**Waveform editor** with a spectral display, a non-destructive **Multitrack
editor** with a mixer, a dockable panel layout, effects, analysis and local
AI stem separation. The look, names and code are its own.

## Build and run

```bash
cargo run --release -p audiform27
```

Windows is the main target. Linux needs the ALSA headers (`libasound2-dev`).
You can pass files or sessions on the command line, or drop them on the window.

## What works today

- **Waveform editor**: every channel stacked (mono through 7.1), gradient
  waveforms with glow, an overview bar, a ruler, markers, time selection,
  zoom at the pointer, and a stacked per-channel **spectral display**.
  Cut, copy, paste, crop, delete, insert silence, generate tone, noise or
  silence. Undo and redo with history.
- **Multitrack editor**: tracks of clips. You can move clips in time and
  between tracks (with snapping), trim either end, use the razor to split,
  split at the playhead, drag files in from the Files panel, use clip gain
  and fades, mute, solo, arm, volume and pan, add markers, select time, and
  undo everything. There's a **Mixer** with channel strips and a master
  strip. Mixdown export is WAV.
- **Effects** (23): Amplify, Normalize, Match Loudness, Fade In and Out,
  Invert, Reverse, Silence, Remove DC Offset, Parametric EQ, High-Pass and
  Low-Pass filters, Notch Filter, Hum Remover, Compressor, Limiter (with
  look-ahead), Noise Gate, De-Esser, Noise Reduction (spectral), Echo,
  Reverb, Stereo Width and Speed. Every effect has preview.
- **Analysis**: Level Statistics (integrated LUFS per BS.1770, true peak,
  sample peak, RMS, DC offset, delivery target checks), a Phase Scope with a
  correlation meter, a Spectrum analyzer, and Levels meters with peak hold.
- **AI Stems**: local stem separation through the separation engine. It
  reuses the engine and models an Audiform install already downloaded, so
  nothing is fetched twice. Downloads are on demand and SHA-256 checked.
- **Recording**: record into a new file, or onto an armed track at the playhead.
- **Docking**: every panel can be dragged, tabbed, split, floated or closed
  (Window menu to reopen; Window > Reset Layout). The layout is saved
  between runs.
- **Themes**: every colour, gradient, spectrogram palette, corner radius
  and glow comes from a theme (see below).

## Formats

| Read | Notes |
| --- | --- |
| WAV, BWF, RF64, AIFF/AIFC, FLAC, MP3, MP2, Ogg Vorbis, Opus, AAC/M4A/MP4, CAF, MKV/WebM audio | Every channel is kept |
| DTS in WAV (DTS music CDs) and raw `.dts` | Decoded to 5.1 / 6.1 by `af27-dts` |
| `.mogg` | Unencrypted multitrack Ogg; encrypted files are refused |
| Multichannel | Stays multichannel; playback folds to stereo (-3 dB centre and surrounds, LFE out) |

| Write | |
| --- | --- |
| WAV | 16-bit and 24-bit integer (dithered), 32-bit float |

| Sessions | |
| --- | --- |
| `.af27` | Native: versioned JSON |
| `.sesx` | Imports tracks, clips, source in/out, clip gain and mute, fades, track volume, pan, mute, solo, markers and file paths (absolute or relative). Effects and automation are reported as skipped |
| `.audfrm` / `.altaudio` | Audiform sessions: the same set |

## Themes

Themes are JSON files with the `.af27theme` extension. Put them in
`%APPDATA%\Audiform27\themes\` (Windows), `~/.local/share/Audiform27/themes/`
(Linux) or `~/Library/Application Support/Audiform27/themes/` (macOS), then
pick one from **View > Theme**. **Save Current Theme As** writes the active
theme out as a starting point, and any field you leave out falls back to the
default. See `docs/themes/Ice Cave.af27theme`.

Built-in themes: **Neon Abyss** (the default), **Synth Sunset**, **Phosphor**
and **Daylight**.

## Keyboard

Space play/stop · Shift+Space record · Ctrl+Z / Ctrl+Y undo/redo · Ctrl+O
open · Ctrl+S save · Ctrl+N new file · Ctrl+A select all · Ctrl+X/C/V ·
Ctrl+T crop · Delete · Ctrl+K split at playhead · Ctrl+L loop · Home / End ·
M add marker · 9 Waveform · 0 Multitrack · V Move · R Razor · T Time
Selection · = / - zoom · Esc clear selection.

## Workspace

| Crate | |
| --- | --- |
| `af27-core` | Audio buffers, the session model, undo history |
| `af27-io` | Decoding (symphonia + DTS + MOGG) and WAV writing |
| `af27-dsp` | Effects, filters, loudness, FFT, spectrogram, resampling, noise reduction |
| `af27-engine` | Real-time playback and recording (cpal; lock-free, no allocation on the audio thread) |
| `af27-interop` | `.sesx` and `.audfrm` import |
| `af27-separation` | AI stem separation client |
| `af27-dts` | DTS Coherent Acoustics core decoder |
| `app` | The egui application |

## Licence

Audiform27 is dual-licensed **MIT OR Apache-2.0** (`LICENSE-MIT`,
`LICENSE-APACHE`). There is one exception: `crates/af27-dts` is
**LGPL-2.1-or-later**, because its decoder is a Rust port of FFmpeg's DTS
decoder. Because the whole project ships as source, anyone can rebuild it
with a modified decoder. See `docs/clean-room.md` for where every part comes
from.
