# Audiform: features, settings and options

Version 0.7.1. Compiled from the source code, the decisions log and your earlier Audiform sessions (2026-11-19).

**★** marks a detail you asked for, chose, or reported. Each ★ item is confirmed in the current code unless the item itself says otherwise.

Audiform is a free, open-source multitrack audio workstation built with Tauri 2, Svelte 5 and Rust. It's for spoken word, music and immersive 3D mixing.

- **Platforms:** Windows 10/11 is the main target. Linux (AppImage, .deb) and macOS (Apple silicon .dmg) builds are experimental.
- **Privacy:** no account, no telemetry, and no network calls of its own. Update checks are off unless you turn them on.
- **No AI unless you turn it on.** Any future AI tool stays off by default and runs locally first. A cloud service would only run with your own key.
- ★ **Versioning:** the version goes up with every release.

---

## 1. Performance and stability (★ the 0.7.1 work)

You reported slow zoom, high memory and lag when clicking the header on your 32-track `.sesx` sessions. These are the fixes in the code:

- ★ **Memory:** there's one shared copy of the plug-in catalog instead of 34. Editor memory went from 637 MB to 52 MB, and the whole editor window from about 3.8 GB to about 0.7 GB.
- ★ **Zoom and scroll:**
  - Wheel zoom and scroll are applied once per frame.
  - A zoom or scroll now reaches only the panels that draw the timeline, not every panel.
  - Canvases keep their memory between redraws.
- ★ **Header and ruler clicks:** a click no longer repaints every lane, and waveforms draw as one shape per channel.
- ★ **Plug-in restarts:** plug-ins no longer restart over and over. That restart loop was what kept triggering the UA Connect prompt.
- ★ **One process per plug-in maker** is now the default. You can untick a maker under Effects > Plug-in Manager > Run together to give its plug-ins separate processes.
- **Plug-ins rest when idle:** those of a session set aside for a minute, and those of a pre-rendered track while its render is current.
- ★ **Help > About** shows where memory goes (the app, the editor window and each plug-in host by name) and the build's commit. You'd reported that the numbers didn't show; that's fixed.
- ★ **Waveform-editor freeze fixed:** the "effect update depth" error you hit while recording or playing in empty or edited files is gone.
- ★ **ASIO recording** recovers when the driver asks for a reset (the problem you hit with your SSL interface at 88.2 kHz). You also found ASIO works well at higher quality settings, but only with a larger buffer.

## 2. Editors and views

- **Multitrack Editor:** track lanes, ruler, playhead, overview strip, tempo and metronome row.
- **Waveform Editor:** destructive editing of a single file (see section 3).
- **Spectrum and Pitch views:**
  - Select with Box, Lasso or Brush, then reduce the selection by 12 or 24 dB, remove it, or repair it.
  - Spot Repair fills a painted click or cough from the audio around it.
  - ★ The box selection now stays in its band when you drag past the edges. You'd reported it jumping.
- **Piano roll** (MIDI tracks):
  - 16 MIDI channels, shown as note colors
  - Per-note velocity (1–127) and pan, with velocity and pan tools
  - Erase tool, copy and paste, and Humanize (random timing offset)
  - Notes sound while you drag them; key height and zoom are adjustable
- **Time formats:** minutes and seconds, seconds, bars and beats, samples, timecode (30 fps).
- **Zoom:** in and out, to session, to selection, out full, and vertical (amplitude) zoom.
- **Envelope display:** clip volume, clip pan and clip effect settings.

## 3. The Waveform editor and your files (★ what you asked for)

- ★ **Unsaved edits carry into sessions.** A file edited in the Waveform editor plays with its edits in any session that uses it, including a session that only picks it up later.
- ★ **Saving the session keeps those edits.** If the file is already in the session folder, the edits are saved into it. Otherwise the session saves an `<name> (edited).wav` copy into the project and switches to it. The original file isn't touched.
- ★ **Opening a take opens it with its edits,** with no question asked, which is what you chose.
- ★ **Dragging a take into a session brings the edited version,** not the raw recording.
- ★ **Save Audio As (Ctrl+Shift+S):**
  - Actually saves now. The bug behind your missing FLACs and WAVs is fixed.
  - Typing ".flac" in the name switches the format.
  - The new file replaces the old one in the Files list.
- ★ **FLAC compression level 0–8** (default 5, all lossless) in Save Audio As, Export and Batch Convert.
- ★ **Format settings are remembered.** Save Audio As keeps your format, compression, bit depth, sample rate and channels for next time.
- ★ **Repeat Last Command:** Shift+R reapplies the last effect or plug-in with the same settings, and Ctrl+R reopens it so you can adjust it first.
- ★ **Fade In (Linear) and Fade Out (Linear)** sit beside Invert and Reverse in the Effects menu.
- **Also in the editor:**
  - Cut, Silence, Amplify, Normalize, equal-power fades
  - Live effects-chain preview, then Apply to the selection or the whole file (undoable)
  - Save as WAV, FLAC or MP3; WAV cue-point markers
  - Activate the left or right channel (Up and Down arrows)

## 4. Tools

- Move (V), Razor (R), Slip (Y), Time Selection (T)
- Spectrum tools: Box, Lasso, Brush, Spot Repair

## 5. Tracks, mixing and automation

- **Track types:** mono, stereo, MIDI (plays VST3 instruments), bus, mono bus, 3D bus, and a Metronome track.
  - The Add Tracks dialog adds several at once, and new sessions start with six tracks.
- **Track header:**
  - Name (double-click to rename) and color
  - Mute, Solo (Ctrl+click for exclusive solo) and record arm
  - Volume and pan dials, monitor, polarity
  - Input picker, grouped by device, including every ASIO channel
  - Sends, automation dropdown, collapse and resize
- **Track management:** duplicate, delete, delete empty tracks, drag to reorder.
- **Track visibility:** hide the selected track, show all, show only audio tracks, only tracks with clips, only tracks with clips in the time selection, or only tracks with clips at the playhead.
- **Mixer:** channel strips, master volume, pre- or post-fader sends to buses, sidechain sends for keyed compression and ducking, a master effects chain, and plug-in delay compensation.
- **Automation lanes:** volume, pan, any effect setting and every 3D position, drawn under each track. Lanes are resizable.
- **Tempo:** tempo and time-signature changes, bars and beats on the ruler, a metronome, grid snapping.
- **Speaker Balance:** levels every voice track to match the quietest one in one click.
- **Host profiles:** Close Mic (−14 dB gate), Standard Mic (−20 dB gate) and Remote Guest.
- ★ **Space** plays, and stops while playing or recording.

## 6. 3D Space sessions

- You place each track and bus in a room, seen from above, instead of panning it.
- **Per-object controls:** left/right, front/back, height (above or below ear level), size, spread, width, rotation, Spatialize (opens a stereo source into the surrounds and heights) and an LFE send.
- **Speaker layouts:** 5.1, 7.1, 5.1.2, 5.1.4, 7.1.2, 7.1.4.
- **Monitoring:** binaural on headphones, or a real speaker rig. Levels and the Mixer show every speaker channel.
- **Export:** a multichannel WAV or a stereo (binaural) fold-down. Every position can be automated.
- ★ **5.1 `.sesx` sessions** come in as 3D Space sessions. Panners become positions in the room, panner automation carries over, and 5.1 buses become 3D buses. You decided the panner's Center control need not carry over.

## 7. `.sesx` import accuracy (★ problems you found)

- ★ **Level match:** your session played about 18 dB louder than in the program that made it. The cause was compressors listening to an empty sidechain input. Now a sidechain input stays off until a key is routed to it.
- ★ **Pitch:** clip time stretch and pitch now import and export, covering the vocals you found coming in a semitone off.
- ★ **Phase Curve:** imported curves now turn one channel against the other, so big phase shifts actually show. The effect now holds 16 points (up from 8, enough for your 13-point curve) and joins them with straight lines.
- **Also carried over:** tracks, clips, positions, fades, levels, and the effects rack's input, output and mix levels. `.sesx` files also drag in from the Files panel as clips.
- **Other imports:** `.aup3` projects, which are read only and have their audio extracted to WAV. ★ DTS 5.1 audio inside WAV files decodes too (you asked for this twice).

## 8. Clip editing (non-destructive)

- **Split:** at the playhead, or split all clips at the playhead.
- **Combine and copy:** Merge Clips, Make Unique Copy, Split Channels to Tracks, Copy to Track Above or Below (Alt+Up, Alt+Down).
- **Position:** Nudge (Alt+, and Alt+.), Lock Position.
- **Trim:** to the time selection, start to playhead, or end to playhead.
- **Clip settings:** Gain, Color, Mute, Loop, Reverse, Invert.
- **Audio type tags:** Dialogue, Music, Sound Effects, Ambience. You can select clips by type.
- ★ **Fades:** Straight or Curve, bent by dragging (skew −100 to 100). Automatic crossfades.
- **Groups:** Group and Ungroup (Ctrl+G).
- **Time Stretch:** 15, 30 or 60 s presets, fit to the time selection, or 25–400%. Keeps pitch, or tape-style. Remove Stretch undoes it.
- **Stretch and Pitch** per clip.
- **Fit Music to Length:** cuts or repeats whole musical sections, keeps the intro and ending, and offers up to three plans.
- **Phase Align:** lines up two recordings to the sample, flipping polarity when needed.
- **Separate Stems** (quick model).
- **Linked session clips:** Insert Session as Clip, or drop a session onto a track. The clip follows that session's saves; double-click it to open the session.
- **Ripple Delete:** selected clips, the time selection in the selected track or in all tracks, or the gap between clips.
- **Clipboard:** Cut, Copy, Paste, Paste to New File, Mix Paste, Delete, Crop.
- **Selection commands:** all, none, clips in a track, the next clip, clips to the track end or session start and end, view time, all time, time around the clips, playhead to end.
- **Snapping:** to clips, playhead, markers and ruler ticks.
- **Insert:** files at the playhead, files as tracks, and silence at the playhead.

## 9. Built-in effects

Up to 16 effects per clip, track, bus or master chain. Each chain has input and output gain, a dry/wet mix and meters. Each effect opens in its own window with a response graph where relevant. Changes apply live without stopping playback. Every effect has presets you can save and delete, and starting chain presets such as Podcast Voice and Voice Over are included.

| Category | Effects |
|---|---|
| Amplitude | Gain, Amp Model (neural `.nam` with gate, tone, level match and cabinet), Tube Preamp |
| Dynamics | Dynamics, Limiter, Breath Gate, De-Esser, Speech Leveler, Opto Leveler |
| Delay and Reverb | Delay and Echo, Reverb, Convolution Reverb |
| Filter and EQ | ★ Parametric EQ (window laid out the way you asked), Band EQ (10/20/30 bands), Curve EQ, Hum Cut, Notch Filter, Precision Filter (Butterworth, Chebyshev, Bessel, Elliptic), Center Remover |
| Modulation | Modulation (several types) |
| Restoration | Noise Reducer, Noise Print Reducer, Shaped Noise Reducer, Hiss Reducer, Room Reducer, Sound Eraser, Click Repair, Clip Repair, DC Offset Fix |
| Stereo | Center Channel Extractor, Channel Mixer, Stereo Width, Phase Rotate, ★ Phase Curve (window laid out the way you asked) |
| Time and Pitch | Pitch Shift |
| Vocal | Vox Strip |

**Other items in the Effects menu:** Amplify, Normalize, Generate (tone, pink or white noise, silence), Loudness Match, spectral reduce, remove and repair, Capture Noise Print (Shift+P), Hide All Effect Windows (Ctrl+Shift+H).

**VST3 plug-ins:**
- Each runs in a separate process (shared per maker), so a crashing plug-in can't take the session down.
- Plug-ins use their own windows and the standard preset folders, and get the song position.
- **Plug-in Manager** handles scanning; **Stop Network Prompts From Plug-ins** blocks their network access.
- Mixdown can run in real time for sessions that use plug-ins. VST2 isn't supported.
- ★ You also asked whether Audiform could rebuild your iZotope plug-ins as lighter built-ins. The fix that came out of that was one process per maker; no built-in replacements were added.

## 10. Recording and playback

- **Recording:** arm tracks and record each one's input to its own file, with a live waveform and input meters.
- **Recording options:**
  - Latency offset (0–1000 ms trimmed from each take)
  - Punch-in limited to the time selection, with 0–10 s pre-roll, optionally replacing what was there
  - Count-in: off, 3 s or 5 s
- **System audio** input on Windows records what the computer plays. ASIO is in release builds.
- **Transport:** play/stop (Space), pause, record (Shift+Space), rewind and fast-forward 5 s (1 s with Shift), go to start or end, previous or next marker or edge.
- **Playback options:** Loop Playback (follows a new selection), Skip Selection, Return to Start on Stop.
- **Meters:** per-track stereo meters with peak hold and clip lights.

## 11. Markers and chapters

- **Marker types:** Cue (M), Subclip, ★ CD Track (Shift+M) and ★ Cart Timer (C) (you kept broadcast and CD features in the plan), Range, Chapter, Ad Slot, Social Clip.
- Markers have colors and notes, and the Markers panel lists them.
- **Chapter export:** a timestamp list, CSV, podcast chapters JSON, and chapters embedded in MP3 and FLAC files.

## 12. Import, export and publishing

- **Opens:** WAV (including 8-bit and ADPCM), AIFF, FLAC, MP3, AAC/M4A (stereo, 5.1, 7.1), ALAC, Ogg Vorbis, Ogg FLAC, Opus and DTS. On Windows, also HE-AAC, Dolby Digital and Dolby Digital Plus.
- **Export:** session mixdown, time selection, track stems, markers and chapters, and a multitrack `.sesx` copy.
  - Formats: WAV (4-bit ADPCM, 8, 16, 24 or 32-bit float), FLAC (★ levels 0–8), MP3 (constant or variable bit rate), and AAC/M4A on Windows only.
  - Any sample rate, mono or stereo, with a loudness option and a check after saving.
- **Mix Down to New File:** whole session, time selection or selected clips.
- **Bounce to New Track:** selected track, time selection, selected clips in the time selection, or selected clips only.
- **Check Loudness:** LUFS and true peak.
- **Loudness Match:** podcast, platform or broadcast targets, following your region.
- **Publish Package:** title, description, season and episode, type (Full, Trailer, Bonus), Explicit flag, folder and loudness.
- **Batch Convert:**
  - Converts to WAV, FLAC, MP3 or AAC
  - Optional effects-chain preset, peak normalize and end fades
  - Naming, destination, sample rate, channels and dither
  - Tag copying or setting, and per-file progress

## 13. Panels and workspaces

- **Panels:** Files, Audio FX, Batch Convert, Editor, Mixer, Levels, Libraries, Time Info, History, Drive Manager, Markers, Playlist, Loudness Match, Properties, Phase Wheel, Visualizer, 3D Space, Audio Separation, Track Details.
- **Built-in workspaces:** Default, Focused Edit, Mixing, Music, Voiceover, Review and Export. You can save, reset and delete your own.
- Tabs have no close button, so a stray click never closes a panel.
- **Libraries:** a folder with collections, metadata and search. You drag items onto tracks.
- **Audio Separation** (optional download): splits audio into vocals and instrumental using public models, listed with their SDR scores. Runs locally on the GPU or CPU.

## 14. Sessions

- Several sessions can be open at once: New, Open, Open Recent (10), Save, Save As, Save All, Close, Close All.
- Save as Session Template, Remove Unused Files, Collect Session Media, Show in File Manager.
- Autosave, crash recovery and rolling backups.
- **Session Properties:** name, sample rate, bit depth, counts, files used, length, saved time.
- Undo and redo appear in the History panel.

## 15. Settings (Edit > Settings, 18 pages)

| Page | Options |
|---|---|
| General | Wheel zoom step; zoom centers on the pointer, between pointer and middle, or the playhead; new fades start straight or curved; re-show dismissed warnings; back up and restore settings |
| Appearance | ★ Themes as `.audtheme` files in a folder (13 ship: Dark, Prism, Moonlight, Aqua, Meadow, Analog, Grey95, Mall Night, Mall Pastel, Marigold, Phosphor, Undercurrent, plus custom); Open Themes Folder, Look Again |
| Audio Devices | Input and output devices, ASIO or system driver, Open System Sound Settings, Refresh |
| Channel Routing | Stereo mix and 3D Space speaker mapping |
| Autosave | Recovery copy every N minutes; keep the newest N backups |
| Session Media | On import: copy into the session folder, ask, or leave in place; subfolder name; copy in outside audio when saving; leave library files in place |
| Effects | Show the chain when adding from the menu; real-time mixdown for VST3 sessions |
| Audio Separation | GPU or CPU; quality 1, 2, 4 or 8 |
| Cache and Files | Cache folder, Clear Waveform Drawings, Change Folder, Use Default |
| Markers | Default name for new markers |
| Multitrack | Solo: several tracks at once, or exclusive |
| Clips | Crossfade overlapping clips |
| Playback and Recording | Follow the playhead; meter yellow and red thresholds; meter response fast, medium or slow; latency offset; pre-roll; record only into the selection; count-in |
| Time Display | Time format |
| Loudness | Region, target, custom LUFS and true-peak ceiling |
| Privacy | Weekly update check (off by default) |

Some settings are shown but greyed out with a reason given: edge crossfades in the Waveform editor, batch parallelism, resampler quality, pan law, scrub audio, spectrum detail, range and gridlines, and tempo for bars and beats.

## 16. Keyboard shortcuts

- Edit > Keyboard Shortcuts rebinds any menu command, and you can bind several keys to one command. ★ You approved the common industry defaults, with every approved shortcut assigned, even on items that aren't built yet.
- **Notable defaults:**
  - Ctrl+K split; 1, 0 and 9 switch editors; Shift+D spectrum
  - `\` zoom to session; G clear the time selection
  - Shift+Backspace ripple delete
  - Alt+A stereo track; Alt+B bus; Shift+A Add Tracks
  - Shift+E delete empty tracks; Shift+X Return to Start on Stop

## 17. Help menu

User Guide (not built yet), Keyboard Shortcuts, Report a Problem, Check for Updates, Source Code on GitHub, Discord, About.

---

## ★ Asked for, but not in this code

On 2026-11-02 you asked for four more fixes, then rolled back to 0.7.1 (commit 79c1e3f) after the test build broke wheel zoom over the tracks:

1. Wheel zoom over the tracks should weigh the same as over the time ruler.
2. A saved session shouldn't reference files that weren't in it.
3. New Audio File should always add the file to the Files list.
4. Closing a file, or the last open file, should clear its waveform from the editor.

## Still greyed out in the menus

Convert Sample Type, Meter Input Signal, ripple delete of the time selection in selected clips, Show Only Bus Tracks, visibility presets, Guest Track, Transcribe Session, Remove Filler Words, Remove Music, Suspend Groups, Remove Focus Clip from Group, Zoom Reset (All Axes), Show HUD, Podcast Mode, Voiceover Mode, Pitch Tune, Pitch Curve, User Guide.
