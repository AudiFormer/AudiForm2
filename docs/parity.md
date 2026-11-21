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
- ✅ Loudness meter (live momentary, short-term, integrated, LRA, true peak) · ⬜ Loudness matcher for many files
- ✅ Repair scanner (Diagnostics: pauses, clicks, clipping, sound) · ⬜ Batch processor · ⬜ Quick actions / macros
- ⬜ File browser · ⬜ Metadata editor · ⬜ Track manager · ⬜ Video preview
- ⬜ Quick Mix Assistant (role-based one-click processing)
- 🟡 Docking: drag, tab, split, float, close, saved layout ✅; named workspaces ⬜

## Formats
- ✅ Read WAV/BWF/RF64, AIFF, FLAC, MP3, MP2, Ogg, Opus, AAC/M4A/MP4, CAF, MKV/WebM
- ✅ DTS in WAV and raw `.dts` · ✅ unencrypted `.mogg` · ✅ multichannel kept
- 🟡 Write: WAV 8/16/24/32f with RF64, extensible masks, cue markers and INFO tags ✅; FLAC 16/24 (levels 0–8, Vorbis tags) ✅; MP3 (licence decision), Ogg, AAC export ⬜
- ⬜ Raw PCM import · ⬜ APE, AU, IFF/8SVX · ⬜ audio from video files
- ✅ `.sesx` import · ✅ `.audfrm`/`.altaudio` read **and write, lossless** · ⬜ `.sesx` export · ⬜ OMF, AAF, FCP XML
- ⬜ Save session with copies of all media (collect files) · ⬜ session templates
- ✅ Mixdown export (session, time selection, selected clip) · ✅ stems export · ✅ bounce track to new track · ✅ export dialog: rate, channels, depth, dither, FLAC level, loudness target with true-peak ceiling, read-back check

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
⬜ Multiband dynamics · ✅ Voice leveler (Speech Leveler) · ✅ Breath Gate · ⬜ Warm (tube-style) compressor · ⬜ Gain and fade curves

**Delay**: ✅ Echo · ⬜ Simple L/R delay · ⬜ Tape/analog delay

**Filter and EQ**: ✅ Parametric EQ (3 band) · ✅ Parametric EQ (7 bands + HP/LP, graph editor; Audiform's `parametric_eq` keys) · ✅ High/Low-pass · ✅ Notch · ⬜ 5-band parametric with M/S ·
⬜ Graphic EQ · ⬜ Drawn FFT filter · ⬜ Precision filters (Bessel, Butterworth, Chebyshev, elliptic)

**Modulation**: ✅ Chorus · ✅ Flanger · ✅ Phaser · ✅ Double Track (Audiform's Modulation)

**Restoration**: ✅ Noise Reduction (adaptive estimate) · ✅ Hum Remover · ✅ Remove DC Offset ·
✅ Noise reduction from a captured noise print · ✅ Live noise reducer (learns as it plays) ·
✅ Declicker (Click Repair) · ✅ Clip restorer (Clip Repair) · ✅ Silence remover (Diagnostics, Strip Silence) · ✅ Auto marker (Diagnostics: Mark Sound) · ✅ Reverb reducer (Room Reducer) ·
✅ Hiss remover (Hiss Reducer) · ⬜ Learned sound removal · ⬜ Auto phase align

**Reverb**: ✅ Reverb · ✅ Convolution reverb · ✅ Room, Hall, Plate, Spring and Tape reverb · ✅ Surround reverb (the Reverb and Convolution Reverb fill a speaker bed, a different room in every speaker, none in the LFE)

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
- 🟡 Clips: gain, fades, mute, colour, lock ✅; clip effects racks, clip automation (volume, pan, effect values), Source Channels ✅; loop, group ⬜
- ⬜ Clip time stretch · ⬜ auto-length music fit · ✅ auto duck (Spoken Word > Auto Duck) · ✅ silence splitter (Strip Silence) · ✅ speaker balance
- ⬜ Bounce to track · ⬜ punch-in · ✅ metronome · 🟡 tempo (one tempo; changes kept in files) · ⬜ latency compensation

## Analysis and metering
- ✅ Integrated loudness, true peak, sample peak, RMS, DC offset
- ✅ Phase Wheel and correlation · ✅ Spectrum at the playhead · ✅ Peak meters with hold
- ✅ Live loudness meter · ⬜ FFT settings (size, window, scale)

## Recording and the rest
- 🟡 Recording: default input into a new file or onto an armed track ✅; device and driver picker, per-channel track inputs, output channel map ✅; ASIO behind the off-by-default `asio` feature (docs/asio.md) 🟡; punch-in ⬜
- ⬜ Batch processing · ⬜ macros · ⬜ metadata editing · ⬜ video track
- ✅ Session autosave every 2 minutes · ⬜ crash-recovery prompt
- ✅ Themes (beyond the reference product's light/dark slider)

## Effects on surround and 3D Space

Every effect works on a speaker bed (a 3D Space session's master chain,
run on the speakers before binaural or fold monitoring and before a
multichannel bed export) and on a surround file in the Waveform editor
(3.0 up to 7.1.4), as in Audiform. Bus tracks stay stereo.

| How | Effects |
| --- | --- |
| Own bed form | Reverb (`af.reverb`), Convolution Reverb (`af.convolution_reverb`), Modulation (`af.modulation`) |
| Every channel as it is | Amplify, Invert |
| Copies per speaker pair, gain linked to the loudest channel | Compressor, Limiter, Noise Gate |
| Copies per speaker pair, LFE left alone (held in time) | Echo, Reverb (built-in), Phase Curve, Amp Model, Tube Preamp, Opto Leveler, Vox Strip (and Modulation, in its bed form) |
| L and R only | Stereo Width (built-in and `af.stereo_width`), Channel Mixer, Center Channel Extractor |
| Copies per speaker pair, LFE too | Parametric EQ, High-Pass, Low-Pass, Notch, Hum Remover, Remove DC Offset, De-Esser |

Pairs go by speaker name (L/R, Ls/Rs, Lrs/Rrs, Lss/Rss, Ltf/Rtf, Ltr/Rtr,
Ltm/Rtm); a lone speaker (C) is fed to both sides and averaged back;
numbered channels pair by order. Each copy is set apart (`vary`) so
sweeps and tails do not move in lockstep.

## Spoken word

See `docs/podcast-features.md` for the full podcast and voice inventory:
Diagnostics, the Loudness panel, Auto Duck, Strip Silence, Speaker
Balance, marker kinds and chapter export, the speech and restoration
effects, and what is still to do.
