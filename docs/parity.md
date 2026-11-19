# Feature parity checklist

This tracks what a mature professional audio editor offers and where
Audiform27 stands. It is compiled from public descriptions of the leading
commercial editor's 2025–2026 feature set, rewritten in our own words, with
our own feature names.

✅ built · 🟡 partly built · ⬜ not yet

## Editors and views
- ✅ Waveform editor (destructive, per file)
- ✅ Multitrack editor (non-destructive)
- ✅ Spectral display under the waveform (stacked per channel)
- ⬜ Pitch contour view
- 🟡 Channel display: all channels stacked ✅; turning single channels on or off for editing ⬜
- 🟡 Time display formats: m:ss.mmm ✅; SMPTE, samples, bars and beats ⬜
- ✅ Zoom at the pointer, zoom in/out/full, overview navigator
- ⬜ Skip-selection preview
- ⬜ Disc assembly view (the reference product dropped its own in 2026)

## Panels
- ✅ Files · ✅ Markers · ✅ Effects (one effect at a time) · ✅ History · ✅ Levels
- ✅ Range (selection and view, editable) · ✅ Level Statistics · ✅ Phase Scope with correlation
- ✅ Spectrum analyzer · ✅ Mixer · ✅ AI Stems (no counterpart in the reference product)
- 🟡 Effect chains: 16 live slots per track, bus and master ✅; per clip ⬜
- ⬜ Loudness meter (live momentary, short-term, LRA) · ⬜ Loudness matcher for many files
- ⬜ Repair scanner (clicks, clipping, silence) · ⬜ Batch processor · ⬜ Quick actions / macros
- ⬜ File browser · ⬜ Metadata editor · ⬜ Track manager · ⬜ Video preview
- ⬜ Quick Mix Assistant (role-based one-click processing)
- 🟡 Docking: drag, tab, split, float, close, saved layout ✅; named workspaces ⬜

## Formats
- ✅ Read WAV/BWF/RF64, AIFF, FLAC, MP3, MP2, Ogg, Opus, AAC/M4A/MP4, CAF, MKV/WebM
- ✅ DTS in WAV and raw `.dts` · ✅ unencrypted `.mogg` · ✅ multichannel kept
- 🟡 Write: WAV 16/24/32f ✅; FLAC, MP3, Ogg, AAC export ⬜
- ⬜ Raw PCM import · ⬜ APE, AU, IFF/8SVX · ⬜ audio from video files
- ✅ `.sesx` import · ✅ `.audfrm`/`.altaudio` read **and write, lossless** · ⬜ `.sesx` export · ⬜ OMF, AAF, FCP XML
- ⬜ Save session with copies of all media (collect files) · ⬜ session templates
- ✅ Mixdown export · ⬜ stems export · ⬜ resample / bit-depth / dither options dialog

## Editing tools
- ✅ Move (clips) · ✅ Razor · ✅ Time selection · ⬜ Slip
- ⬜ Spectral rectangle, freeform and brush selection · ⬜ Spectral spot repair
- ⬜ Zero-crossing snap · 🟡 Snapping to clip edges and playhead ✅; markers and grid ⬜
- ✅ Cut, copy, paste, crop, delete, insert silence
- ⬜ Mix paste (overlap, modulate, crossfade) · ⬜ multiple clipboards · ⬜ copy to new file
- ⬜ On-canvas gain control · 🟡 Clip fades: imported and played ✅; fade handles ⬜
- ⬜ Ripple delete · ✅ Split · ⬜ Merge clips · ⬜ Repeat last command

## Effects
**Amplitude and dynamics**: ✅ Amplify · ✅ Normalize · ✅ Match Loudness · ✅ Fade In/Out ·
✅ Compressor · ✅ Limiter · ✅ Noise Gate · ✅ De-Esser · ⬜ Channel matrix · ⬜ Curve dynamics ·
⬜ Multiband dynamics · ⬜ Voice leveler · ⬜ Warm (tube-style) compressor · ⬜ Gain and fade curves

**Delay**: ✅ Echo · ⬜ Simple L/R delay · ⬜ Tape/analog delay

**Filter and EQ**: ✅ Parametric EQ (3 band) · ✅ High/Low-pass · ✅ Notch · ⬜ 5-band parametric with M/S ·
⬜ Graphic EQ · ⬜ Drawn FFT filter · ⬜ Precision filters (Bessel, Butterworth, Chebyshev, elliptic)

**Modulation**: ✅ Chorus · ✅ Flanger · ✅ Phaser · ✅ Double Track (Audiform's Modulation)

**Restoration**: ✅ Noise Reduction (adaptive estimate) · ✅ Hum Remover · ✅ Remove DC Offset ·
⬜ Noise reduction from a learned noise profile (the code exists in `af27-dsp::noise::learn`; no UI yet) ·
⬜ Declicker · ⬜ Clip restorer · ⬜ Silence remover · ⬜ Auto marker · ⬜ Reverb reducer ·
⬜ Hiss remover · ⬜ Learned sound removal · ⬜ Auto phase align

**Reverb**: ✅ Reverb · ✅ Convolution reverb · ✅ Room, Hall, Plate, Spring and Tape reverb · ⬜ Surround reverb

**Special**: ⬜ Distortion · ⬜ Doppler · ✅ Amp sim (neural `.nam`) · ✅ Tube preamp · ⬜ Mastering suite · ✅ Voice strip (Vox Strip)

**Stereo**: ✅ Stereo Width · ✅ Centre isolate/remove · ✅ Drawn phase shift (Phase Curve) · ✅ Channel matrix

**Time and pitch**: ✅ Speed (varispeed) · ⬜ Pitch shift · ⬜ Time stretch (keep pitch) ·
⬜ Auto pitch tune · ⬜ Pitch draw · ⬜ Pitch curve

**Generate**: ✅ Tone (sine, square, triangle, saw) · ✅ Noise (white, pink, brown) · ✅ Silence ·
⬜ Phone tones · ⬜ Multi-harmonic and sweep tones

**Plug-ins**: ⬜ VST3 hosting · ⬜ CLAP hosting

## Multitrack
- ✅ Audio tracks of any channel count · ✅ buses · ✅ pre/post sends · ✅ track outputs · ✅ MIDI tracks with a built-in synth and piano roll · ✅ 3D Space sessions
- ✅ Mute, solo, arm, volume, pan · ⬜ safe solo · ⬜ monitor input
- ⬜ Automation lanes and modes (read, write, latch, touch)
- 🟡 Clips: gain, fades, mute ✅; loop, colour, lock, group, clip effects ⬜
- ⬜ Clip time stretch · ⬜ auto-length music fit · ⬜ auto duck · ⬜ silence splitter
- ⬜ Bounce to track · ⬜ punch-in · ✅ metronome · 🟡 tempo (one tempo; changes kept in files) · ⬜ latency compensation

## Analysis and metering
- ✅ Integrated loudness, true peak, sample peak, RMS, DC offset
- ✅ Phase Wheel and correlation · ✅ Spectrum at the playhead · ✅ Peak meters with hold
- ⬜ Live loudness meter · ⬜ FFT settings (size, window, scale)

## Recording and the rest
- 🟡 Recording: default input into a new file or onto an armed track ✅; device picker, ASIO, punch-in ⬜
- ⬜ Batch processing · ⬜ macros · ⬜ metadata editing · ⬜ video track
- ✅ Session autosave every 2 minutes · ⬜ crash-recovery prompt
- ✅ Themes (beyond the reference product's light/dark slider)
