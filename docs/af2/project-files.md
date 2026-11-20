# Project files: what carries over

What AudiForm 2 reads from, and writes back to, other project files. The
longer background guide (Audiform 1's own design, the `.sesx` structure,
the history of each mapping) is the project lead's `af2-notes/project-files.md`;
this page is the current state of AudiForm 2's code
(`crates/af27-interop`).

Legend: **yes** carries over; **est.** carries over as an estimate (the
other program's knob scale was inferred from real sessions, not from a
readout); **kept** is not used here but is saved back unchanged; **no**
is lost (and named in the import notice or export report).

## `.audfrm` / `.altaudio` (Audiform 1)

Saving starts from the document that was opened and writes only what
changed, so everything AudiForm 2 does not use passes through, and an
untouched session saves byte for byte the same (checked on all 77 of the
lead's sessions, `real_sessions_round_trip_exactly`).

| What | Open | Save |
| --- | --- | --- |
| Clip gain, mute, fades and fade curves | yes | yes |
| Clip volume and pan envelopes | yes (clip lanes, times from the clip edge) | yes |
| Clip effect envelopes (effect parameters) | yes | yes |
| Clip effects rack (built-in effects AudiForm 2 has, VST3 slots with `params` and `plugin_state`), rack power, input/output gain, mix | yes | yes |
| Audiform effects AudiForm 2 lacks (e.g. Dynamics) | kept (silent slot, in place) | kept |
| AudiForm 2-only effects | written as `af27.<id>` slots (Audiform keeps but skips them), with their `data` (impulse file, kept `.sesx` component) | yes |
| Clip Source Channels (`channel_mix.sends`, clip "input") | yes | yes |
| Clip stretch, pitch, reverse, invert, lock, colour | yes | yes |
| Looped clips | played once (notice) | kept |
| Clip group, audio type, invert channel | kept | kept |
| Track volume, pan, mute, solo, mono, polarity, solo safe, height, colour | yes | yes |
| Track automation: volume, pan, effect parameters, plug-in controls (`p<id>`) | yes | yes |
| Mute and send automation (from a `.sesx`) | yes, from `af27_lanes` | yes, as `af27_lanes` (Audiform has no such envelopes) |
| Track and bus effects racks, sidechain keys | yes | yes |
| Track output (master or bus), sends (level, pan, pre/post, on) | yes | yes |
| Track hardware input (`input`) | yes (`Track.input`) | yes |
| Record arm, MIDI input, automation mode, lanes shown | yes | yes |
| Pre-render switch (`prerender`) | yes | yes (only when on) |
| Monitor input / monitor dry, role, host profile, hidden, collapsed | kept | kept |
| Slot channel routing (`routing`), slot preset names | kept | kept |
| MIDI clips, tempo map, SoundFont, built-in synth | yes | yes |
| VST3 instrument on a MIDI track | kept; the built-in synth plays (notice) | kept |
| Buses, bus position, 3D Bus, 3D spots and spot automation | yes | yes |
| Master volume, effects, automation | yes | yes |
| Markers (name, position, length) | yes | yes; kind, colour, chapters kept |
| View, libraries, playlist, episode metadata, later fields | kept | kept |

## `.sesx` (Multitrack session XML)

| What | Open | Save as `.sesx` |
| --- | --- | --- |
| Clip gain, mute, fades (curve amount; S-curves as straight: est.) | yes | yes |
| Clip volume and pan lines | yes (cut to the clip; a line flat within the clip becomes the clip gain) | yes |
| Clip mute lines | yes (clip Mute lane) | yes |
| Clip stretch and pitch, lock, colour | yes | yes |
| Clip Source Channels (`channelMap`) | yes | yes (full on/off sends only) |
| Reversed / inverted clips | — | written plain (reported) |
| Looped clips, clip groups | no (reported) | — |
| Track volume, pan, mute, solo, solo safe, arm, mono, height, colour | yes | yes |
| Track volume and pan lines | yes | yes |
| Track mute lines | yes (Mute lane; kept, not played yet) | yes |
| Send level and pan lines | yes (Send Level / Send Pan lanes; kept, not played yet) | yes |
| Send on/off lines | no (reported) | — |
| Track output to master or bus, sends | yes | yes |
| Direct hardware output of a track | est. (sent to the master) | — |
| Track hardware input (`trackInput` + `audioDevice` port names) | yes (`Track.input`; "Default…" ports read as the default) | yes (ports written by name) |
| Pre-render switch (`preRendered`) | yes | yes |
| VST3 plug-ins (track, bus, clip, master racks) with their settings chunk | yes | yes |
| Plug-in control lines | yes (lanes by the control's number and name, 0..1) | yes; lanes named by plug-in id from Audiform (`p<id>`) cannot be placed (reported) |
| Built-in effects with a counterpart (table below), with their parameter lines | yes / est. | yes (written back as the built-in effect) |
| Built-in effects without one (Mastering, Multiband Compressor, FFT Filter, Graphic Equalizer, Dynamics Processing, Dynamics, Automatic Pitch Correction, DeNoise, DeReverb, Automatic Phase Correction, Surround Reverb, anything else) | kept (silent slot holding the component XML, lines and all; named in the notice) | yes, unchanged |
| Rack input/output/mix | yes | yes |
| Rack level lines, sidechain inputs, lines on settings with no counterpart | no (reported) | — |
| AudiForm 2 effects with no counterpart (Vox Strip, Amp Model, Tube Master, …) | — | no (reported) |
| Space lanes, 3D Space (5.1 sessions open as 3D: spots from panners) | yes (5.1) | stereo (reported) |
| Master volume and its line, master effects | yes | yes |
| Markers (XMP block) | yes | no (reported) |
| Video tracks | no (reported) | — |

### Built-in effects

| `.sesx` effect | AudiForm 2 effect | Settings | Lines | Back out as |
| --- | --- | --- | --- | --- |
| Parametric Equalizer | Parametric EQ (`af.parametric_eq`) | 5 bands (on, frequency, gain ±24 dB, Q), shelves, HP/LP (on, frequency, slope), output gain, bypass | yes | Parametric Equalizer |
| Graphic Phase Shifter | Phase Curve | channel, every point (thinned to 16) | — | Graphic Phase Shifter |
| Hard Limiter | Limiter | ceiling, input boost (est.) | yes | Hard Limiter |
| Stereo Expander | Stereo Width | centre pan, width (a third of the knob is 100 %) | yes | Stereo Expander |
| Channel Mixer | Channel Mixer | the L/R part of the matrix, polarity flips | yes | Channel Mixer |
| Studio Reverb | Reverb (`af.reverb`) | est.: size, decay, early reflections, width, diffusion, damping, low/high cut, dry, wet | yes | Studio Reverb |
| Chorus | Modulation (Chorus) | est.: voices, delay, feedback, rate, depth, spread, width, dry, wet | yes | Chorus |
| Center Channel Extractor | Center Channel Extractor | est.: extract, phase, pan, delay, band (start/end frequency), bleed, discrimination, decay, levels, FFT size | yes | Center Channel Extractor |
| Amplify | Amplify | est.: the two sides' average gain | yes | Amplify |
| Single-band Compressor | Compressor | threshold, ratio, attack, release, output gain (hard knee) | — | Single-band Compressor |
| Tube-modeled Compressor | Compressor | as above, without the tube colour (est.) | — | Single-band Compressor |
| DeEsser | De-esser | est.: threshold, frequency | — | DeEsser |
| Delay, Echo, Analog Delay | Echo | est.: time, feedback, mix | Delay/Echo mix line | Analog Delay |
| Convolution Reverb | Convolution Reverb | est.: dry/wet from mix and gain, pre-delay, width, length; the impulse file's path | — | Convolution Reverb |
| Adaptive Noise Reduction | Noise Reduction | est.: amount | yes | Adaptive Noise Reduction |
| Notch Filter | Parametric EQ cuts | est.: up to five notches as narrow −24 dB bands | — | Parametric Equalizer |
| xvox (plug-in) | Vox Strip | its knobs | — | no |

Not in any of the lead's sessions, so their storage could not be read
and they are kept as they are: Noise Gate, Speech Volume Leveler, Guitar
Suite, Flanger, Phaser, Chorus/Flanger, FFT Filter (stored as a drawn
curve; no fair EQ fit), Mastering.

Checked on the lead's sessions (`real_sesx_sessions_import`, 1744
sessions): every session opens, and an export reads back with the same
clips, the same effects in every rack and the same lanes.

## `.aup3` (Audacity)

Import only.

| What | Open |
| --- | --- |
| Tracks (stereo pairs joined), name, gain, pan, mute, solo | yes |
| Clips, trims, stretch ratio, cent shift | yes |
| Clip gain envelopes | yes (flat: clip gain; moving: clip volume lane) |
| Labels | yes (markers; a label with an end is a range) |
| Real-time effects | no (named in the notice; empty effect lists are not counted) |
| Note, time tracks | no (reported) |

## Not carried yet, and why

* **Playback of mute, send and plug-in control automation.** The lanes
  are read, kept and saved, but the engine (outside this work) only
  plays volume, pan and built-in effect lanes.
* **Track hardware input** is kept in the session (`Track.input`) but
  the recorder still uses its own per-track choice.
* **Sesx plug-in control numbers** are the plug-in's parameter index;
  Audiform names controls by parameter id. Without the plug-in loaded the
  two cannot be matched, so lanes keep the form they came in.
* **Built-in effects stored only as a settings chunk** that did not turn
  up in the lead's sessions, and multi-band or curve effects with no fair
  counterpart, are kept rather than guessed.
* **Effect estimates** (marked est.) come from the values found in real
  sessions, not from the other program's readouts.
