# AudiForm 2

![AudiForm 2](assets/logo/audiform2-logo-dark.svg)

> **⚠️ Highly experimental.** AudiForm 2 is an early alpha. Expect bugs,
> missing pieces and changes between builds. Keep backups of sessions you
> care about. **Please report any problem you hit** through
> [GitHub Issues](https://github.com/nwh212/AudiForm2/issues). Say what you
> did, what you expected and what happened, and attach the session or a
> screenshot if you can. Every report helps.

A fast, themeable audio editor written entirely in Rust. It's the sequel to
[Audiform](https://github.com/nwh212/Audiform), rebuilt from scratch.

AudiForm 2 covers the full editing workflow:
- a destructive **Waveform editor** with a spectral display
- a non-destructive **Multitrack editor** with a mixer, automation and plug-in hosting
- a dockable panel layout
- effects, analysis and local AI stem separation

(Working name until 2026-11-19: Audiform27. The crates, the `.af27` and
`.af27theme` extensions and the settings folder keep that name so nothing
saved is lost.)

## Download

Windows builds are on the
[Releases](https://github.com/nwh212/AudiForm2/releases) page. They're
marked as pre-releases while AudiForm 2 is in alpha.

## Build and run

```bash
cargo run --release -p audiform2
```

Windows is the main target. Linux builds need the ALSA headers (`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora).
You can pass files or sessions on the command line, or drop them on the window.

ASIO is optional. It needs LLVM (`LIBCLANG_PATH`) and builds against
Steinberg's ASIO SDK under its GPLv3 option. See `docs/asio.md`:

```bash
cargo build --release -p audiform2 --features asio
```

## What works today

- **Waveform editor**
  - Every channel stacked (mono through 7.1.4). Click a channel's letter (L, R, C, ...) to mute or turn it off.
  - Zoom at the pointer, markers and time selection.
  - A stacked per-channel **spectral display** with box selections that loop and play only the frequencies you picked.
  - Cut, copy, paste, crop, insert silence, and generate tone or noise.
  - Undo history.
- **Multitrack editor**
  - Audio, **bus** and **MIDI** tracks.
  - Clips: move, trim, split. Clip gain with a dB readout. Fades that can be straight, curved or bell (S-curve).
  - Volume and pan lines on every clip, with node values shown and snapping to their defaults.
  - Track automation, including plug-in parameters.
  - Sends, sidechains and buses. A master with pan, mono and invert.
  - A **Mixer** and a metronome.
  - Mixdown, stem and bounce export.
  - **Pre-render**: renders heavy tracks in the background so their plug-ins rest.
- **Effects rack**
  - 16 slots on every track, bus, clip and the master.
  - Settings change live, with no dropouts.
  - Built-in presets, voice chains among them.
  - **VST3** plug-ins, hosted outside the app so a crashing plug-in can't take AudiForm 2 down.
- **Audiform effects**, with their original tuning:
  - Amp Model (`.nam` captures), Tube Preamp, Tube Master (multichannel), Opto Leveler, Vox Strip
  - Reverbs and Convolution Reverb, Modulation, Center Channel Extractor, Channel Mixer, Stereo Width, Phase Curve
  - Plus EQ, dynamics, restoration and more.
- **3D Space sessions**: place tracks in a 5.1 to 7.1.4 room. Listen binaurally or folded to stereo.
- **MIDI**: a piano roll and a built-in synth.
- **Analysis**:
  - Level Statistics (LUFS, true peak), the Phase Wheel and a spectrum analyzer.
  - Levels meters with adjustable sensitivity.
- **AI Stems**: local stem separation through the AudiForm 2 separation engine, downloaded on demand and SHA-256 checked.
- **Audio devices**: WASAPI and, in ASIO builds, ASIO.
- **Docking and themes**: every panel can be docked, tabbed or floated, and every colour comes from a theme.

## Formats

| Read | Notes |
| --- | --- |
| WAV, BWF, RF64, AIFF/AIFC, FLAC, MP3, MP2, Ogg Vorbis, Opus, AAC/M4A/MP4, CAF, MKV/WebM audio | Every channel is kept |
| DTS in WAV (DTS music CDs) and raw `.dts` | Decoded to 5.1 / 6.1 by `af27-dts` |
| `.mogg` | Unencrypted multitrack Ogg, any number of channels; encrypted files are refused |
| Dolby Digital (AC-3), Dolby Digital Plus (E-AC-3), HE-AAC, AC-4 | Through Windows' own decoders (AC-4 needs Dolby's decoder installed). Dolby Atmos music opens as its 5.1 or 7.1 bed, which carries the whole mix; the height objects need Dolby's renderer |
| Anything else ffmpeg reads | If you have ffmpeg installed (on the PATH, or named by `AF2_FFMPEG`), files nothing else opens go through it, on any system. AudiForm 2 never downloads or ships it |

| Sessions | |
| --- | --- |
| `.audfrm` / `.altaudio` | **The session format.** Read and written losslessly, so sessions go back and forth between Audiform and AudiForm 2. Anything AudiForm 2 doesn't use is saved back untouched. |
| `.sesx` | Imports tracks, clips, fades, clip and track effects, I/O, automation and plug-in automation. Built-in effects are mapped to AudiForm 2's own. Also exports. See `docs/af2/project-files.md` |
| `.af27` | A plain dump of AudiForm 2's own model |

## Themes

Built-in themes:
- **Zero Point** (the default), Neon Abyss, Solar Flare, Synth Sunset and Phosphor
- Prism, Moonlight Aero, Undercurrent, Analog and Grey Desktop '95
- Mall Pastel '95 (Day and Night)
- **Studio Lite**: a flat, lightweight theme for weaker machines

To add your own, put `.af27theme` JSON files in `%APPDATA%\Audiform27\themes\`
(Windows) or `~/.local/share/Audiform27/themes/` (Linux). Then pick one
from **View > Theme**. See `docs/themes/Ice Cave.af27theme`.

On Linux, the **System** theme at the top of **View > Theme** takes your
desktop's colours and follows them as you change them. On KDE Plasma it
uses your whole colour scheme; on GNOME and other desktops, light or dark
and the accent colour.

## Performance

The engine never allocates or locks on the audio thread. Each track's work is
spread over a lock-free pool of worker threads. On the same dense 48-track
session and PC as Audiform:

| Threads | Audiform | AudiForm 2 |
| --- | --- | --- |
| 1 | 6.6x real time | 19.6x |
| 4 | 20.5x | 63.3x |
| 20 | 32.6x | 78.0x |

The UI draws with wgpu, and falls back to OpenGL where wgpu can't start.

## Reporting problems

AudiForm 2 is experimental, so reports matter. Open an issue at
<https://github.com/nwh212/AudiForm2/issues> with:
- the build (Help > About)
- your Windows version and audio device or driver
- the steps that cause the problem, and what happened versus what you expected
- if possible, the session file, a screenshot or a recording

## Licence

AudiForm 2 is free software under the **GNU General Public License,
version 3 or later** (`LICENSE`, full text in `COPYING`).
`crates/af27-dts` is **LGPL-2.1-or-later**, because its decoder is a Rust
port of FFmpeg's DTS decoder.

AudiForm 2 is written clean-room. It takes behaviour from Audiform 1, never
code. Contributions are accepted only under the terms in `CONTRIBUTING.md`.
`ATTRIBUTION.md` and `docs/clean-room.md` record where every part comes from.

ASIO is a trademark and software of Steinberg Media Technologies GmbH. VST is
a trademark of Steinberg Media Technologies GmbH.
