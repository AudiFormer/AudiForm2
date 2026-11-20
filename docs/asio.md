# ASIO

AudiForm 2 can play and record through ASIO drivers on Windows. It is off
by default: a normal build uses the system driver (WASAPI) only, and
Settings > Audio Devices shows ASIO as "not in this build", listing any
ASIO drivers installed so the user can see why they are not offered.

## Licence: the GPLv3 route

ASIO is Steinberg's. Building with it compiles part of Steinberg's ASIO
SDK into the program. Steinberg offers the SDK under GPLv3 or under its
own proprietary licence. AudiForm 2 is GPL-3.0-or-later, so release builds
use the **GPLv3 option**: no separate agreement with Steinberg is needed,
and a build with ASIO is distributed under GPLv3 as a whole (check the
SDK's licence text for whether it allows later GPL versions; until then,
treat such a binary as GPLv3). What that asks of a
release:

- Ship the complete corresponding source with the build, or a written
  offer for it, as GPLv3 requires. The ASIO SDK is not committed to this
  repository; state which SDK version the build used (and its SHA-256),
  so anyone can fetch the same files from Steinberg and rebuild.
- Keep the credit "ASIO is a trademark and software of Steinberg Media
  Technologies GmbH." The About box shows it in builds with the feature
  (`af27_engine::ASIO_CREDIT`); use the name ASIO only to say what is
  supported, and never the ASIO logo.
- Add `asio-sys` (Apache-2.0, part of the cpal project, compatible with
  GPLv3) and the ASIO SDK (GPLv3 option) to `ATTRIBUTION.md`; `asio-sys`
  is a Cargo dependency only when the feature is on.

Builds without the feature contain nothing of the SDK.

## Building with ASIO

Needed on the build machine (none of this is needed for a normal build):

1. Visual Studio Build Tools with the MSVC C++ compiler.
2. LLVM, for `bindgen` (it needs `libclang`). Set `LIBCLANG_PATH` to
   LLVM's `bin` folder, e.g. `C:\Program Files\LLVM\bin`. GitHub's Windows
   runners have it already.
3. The ASIO SDK (used under its GPLv3 option). `asio-sys`'s build script
   downloads it from Steinberg when it is not given one; better, download
   it yourself, check its SHA-256, unzip it and set `CPAL_ASIO_DIR` to the
   folder that holds `common/` and `host/`, so the build is pinned and
   needs no network.

Then:

```
cargo build --release -p audiform2 --features asio
```

The `asio` feature of the app turns on `af27-engine/asio`, which turns on
cpal's `asio` feature and the `asio-sys` crate. Nothing else changes: the
same code paths compile with the feature off (`ASIO_BUILT` is false and
the ASIO choice is greyed out).

For CI, a separate Windows job with `--features asio`, `LIBCLANG_PATH` and
a pinned `CPAL_ASIO_DIR` is the only check that the ASIO code compiles;
the ordinary jobs never build it.

## How it behaves

- **Only the chosen driver is loaded.** Driver names come from the
  registry (`HKLM\SOFTWARE\ASIO`), which loads nothing. cpal's own ASIO
  device listing loads every installed driver in turn (slow, and a crash
  risk with many drivers), so AudiForm 2 loads one driver by name through
  `asio-sys` and wraps it with cpal's public types (`asio.rs`); cpal is not
  patched.
- **The driver guess** (when none is chosen) passes over wrappers and
  virtual cables (ASIO4ALL, FlexASIO, Voicemeeter, VB-Audio, generic and
  Realtek drivers) in favour of the interface's own driver.
- **One output stream.** Playback, hand-played notes and the metronome are
  mixed into one callback, so there is never a second stream on the driver
  to replay stale buffers or ask for a different channel count. Recording
  opens the driver's input with the same device value.
- **Busy driver.** Most ASIO drivers serve one program at a time. If the
  driver will not open, the system driver plays instead and a message
  says so.
- **Buffer size and rate.** Settings offers the buffer sizes the driver
  accepts and the rates it can run at. A size it refuses falls back to the
  driver's own, with a note. Changing either reloads the driver.
- **Resets and stalls.** A driver that asks for a reset, fails, or stops
  calling back for two seconds has its stream opened again (inputs pad the
  gap with silence so takes stay in time; a rate change ends the take with
  a note). Each stream ends on a few silent buffers before it closes.
- **Log.** Every stream opened, every error and restart goes to
  `audio.log` in the data folder (path in Settings > Audio Devices).

## Not verified

The ASIO code paths have never been compiled here (no LLVM on the
development machine) or run on hardware. Before a release with ASIO, run
the hardware checklist in the handoff notes: Play/Stop repeatedly while
recording; a 64-sample buffer then Record then Play; 88.2 and 96 kHz at 64
and 128 samples; unplug and replug during playback; another program
holding the driver, then Play; a 12-output rig with speakers to outputs.
