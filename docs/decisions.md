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
  *(Superseded later the same day: AudiForm 2 is GPL-3.0-or-later, so
  ASIO can use Steinberg's GPLv3 option. See `LICENSE`.)*

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

## 2026-11-19: Tempo map, MIDI files, piano roll, quantize, SoundFonts

- **Tempo map.** `Tempo` now carries `beat_unit` and `changes` (bar,
  tempo, top, bottom), the model Audiform's `.audfrm` already stores:
  changes on bar lines (bars from 0, so 1 is the second bar), no ramps,
  beats are quarter notes, each stretch starts where the previous one's
  bars end, out-of-order changes are ignored as Audiform does. It drives
  note timing, the synth, the click (accent on each bar's first beat, a
  click per beat of the time signature: eighths in 6/8), bar.beat.tick
  (960 ticks per beat of the time signature), the piano roll's bar and
  beat lines, a bar grid and bar numbers in the Multitrack (shown when
  there are MIDI tracks, tempo changes or the metronome), new clips (on
  bars, four bars long) and the session's length. Multitrack > Tempo and
  Time Signature... lists the start and every change (bar, tempo, time
  signature, where it starts) with Add at Playhead and Delete; the
  transport's tempo field and time signature show and edit the stretch
  under the playhead.
- **`.audfrm` compatibility.** The map is read from and written to
  `tempo` in place. A value read and not edited is written back exactly
  (same number form), each change keeps its id and any fields we do not
  use, and a session without changes keeps the file's own `changes`, so
  unedited sessions save byte-for-byte as before. The "only the first
  tempo plays" notice is gone.
- **MIDI files** via `midly` (Unlicense). File > Import > MIDI File...
  makes one MIDI track per SMF track and channel with notes, each with one
  clip from the bar of its first note; velocity, channel and pan
  (controller 10) come across; tempo and time signature events become the
  map (an event inside a bar moves to the nearest bar line, and a toast
  says how many). If the session already has tempo changes the app asks
  before replacing them. Multitrack > Export > MIDI File... writes format
  1 at 960 PPQ, the map in track 0, one track per MIDI track (unmuted
  clips as they play, cut at clip ends, pan as controller 10 when it
  changes). Export then import gives the same notes and map (tested).
- **Piano roll parity with Audiform.** Draw, Select and Erase tools;
  Shift-click and box select; moving or stretching one picked note moves
  or stretches them all (both ends stretch); right-drag erases; Cut, Copy,
  Paste (at the playhead, or the last spot clicked when the playhead is
  outside the clip; the clip grows to the next bar line if needed),
  Delete and Select All on the usual keys while the pointer is over the
  roll; Tools: velocity set, scale %, randomize; pan set, randomize;
  channel; Humanize in ms; triplet grids and a one-bar grid; Alt leaves
  the grid; key height (Alt+wheel or buttons), middle-button pan. Notes
  sound through the track's instrument when pressed, drawn and moved to
  another pitch, and the keys play. Every gesture is one undo step.
- **Quantize** (new): grid (with triplets), strength, swing (100% puts
  the off-beat a third of a step late, the 2:1 feel), starts on or off,
  ends kept, pulled to the grid, or lengths to whole steps; on the picked
  notes or the whole clip. The grid counts from the session start.
- **SoundFont instruments** via `rustysynth` (MIT, pure Rust). A MIDI
  track's Inspector picks Built-in synth or SoundFont (.sf2)..., with a
  file picker, preset list (bank:program name) and level. Fonts load once
  on the UI thread and are shared; each track's player has its own
  synthesizer made when the mix is built, plays a pre-sorted event list
  and renders into buffers it already has, so nothing allocates on the
  audio thread. Jumps let every note go and restart the notes sounding
  there. Notes take channels in turn (not 10) so per-note pan holds; the
  preset is set on all of them, and drum kits are bank 128. Reverb and
  chorus in the font player are off (use the track's effects). A missing
  font leaves the track silent and says so in the Inspector.
- **Where the SoundFont is saved.** `.audfrm` has no place for it, so it
  goes in AudiForm 2's own track field `af27_soundfont` (path, bank,
  program, preset name, level), next to the existing `af27_synth`; the
  writer only adds it when a track uses a SoundFont and removes it when
  the track goes back to the built-in synth, so other sessions are
  untouched and Audiform keeps the field. `.af27` saves it as
  `soundfont` on the track.
- **Hand-played notes** go through an audition voice in the engine, made
  on the UI thread for the open track's instrument and swapped in over
  the command queue (the old one comes back to be freed), heard whether
  or not the transport runs and never in exports.

## 2026-11-19: Sessions, the Files panel and importing many files

Behaviour follows Audiform 1's decisions log (dates are its entries).

- **Several files dropped on the Multitrack** (Audiform 1, 2026-10-27,
  which replaced the 2026-10-25 setting): one per track, all from the drop
  time; the first on the track under the pointer, the rest on the audio
  tracks below it, then on new tracks named after the files. **Holding Alt
  puts them one after another on one track.** The drop preview follows Alt
  as it goes down and up. While files come from the file manager the window
  hears no keys or mouse moves, so on Windows Alt and the pointer are read
  from the system. A drop below the last track, or on a bus or MIDI track,
  makes new tracks. Drops close to the start land on it (2026-10-25).
  Ours: dropped files are taken in natural name order ("2-guest" before
  "10-guest") rather than the order the file manager hands them over, and
  folders are searched (8 levels, 5,000 files) as Audiform 1 did for the
  Files panel (2026-10-13). The whole drop is one undoable step.
- **Insert Files as Tracks** (2026-10-14): each file on its own new track at
  the start, named without a leading speaker number. Ours: only one to three
  digits count as a speaker number, so "2026-11-19 call" keeps its date.
  Zips are not extracted yet.
- **Several sessions open at once** (2026-10-25): New, Open and Open Recent
  set the session in front aside with its changes, undo history and view;
  the Files panel lists them (double-click to switch, × to close, asking to
  save). Closing the front one brings the last one set aside forward. An
  untouched blank session is dropped, not listed. Opening one already open
  brings it forward. Quitting asks about unsaved sessions.
- **Session commands** (2026-10-14): Open Recent (10), Save, Save As (names
  the session after its file), Save All, Close, Close All, Save as Session
  Template (an `.audfrm` in the app's templates folder; a new session from
  it gets a new id and records the template's), Remove Unused Files (keeps
  every file a clip in any open session plays; asks before dropping unsaved
  Waveform edits, 2026-10-31), Collect Session Media (one undoable step,
  2026-10-25), Session Properties, Show in File Manager.
- **Session Media** (2026-10-25): copy imported audio into the session's
  `Audio` folder (default), ask, or leave it; the folder name; copy in
  outside audio when saving. Copies are found again by name, size and the
  first and last 64 KB; a different file with the same name is numbered;
  copies are written under a temporary name first. Ours, to keep `.audfrm`
  files unchanged: copy-on-save only moves files that came into the session
  in this run, never paths the file already had, and opening a file for the
  Waveform editor does not copy it (only imports into the session do).
- **Autosave, recovery, backups** (2026-10-14, 2026-10-19, 2026-10-21): a
  recovery copy per open session with unsaved changes; saving or closing
  deletes it; copies whose run has no fresh lock (150 s) are offered at
  start. Each save first copies the previous file into `Backups` beside it,
  keeping the newest 1 to 50 (default 10); only our own time-stamped names
  are pruned. Stamps are UTC.
- **Waveform edits on session save** (2026-11-01): unsaved edits to a file
  the session uses are written into it when it is a WAV in the session
  folder, else as `<name> (edited).wav` in the media folder, the session and
  the Files panel moving to it.
- **The four fixes asked for on 2026-11-02:** wheel zoom over the tracks
  (Ctrl+wheel) already weighs the same as over the ruler here (tested in
  `zoom.rs`); a saved session leaves out media no clip uses that was never
  saved with it; New Audio File always lists the file; closing a file stops
  it playing and no longer leaves its spectrogram (the drawing cache is now
  keyed by document).
- **`.audfrm` stays byte-exact for unedited sessions:** none of the above
  touches media the file already had. New media inside the session folder
  gets a `relative_path`, as Audiform writes it.

## 2026-11-19: GPLv3

- **Licence changed to GPL-3.0-or-later** (project lead), so release builds
  can include ASIO under Steinberg's GPLv3 option for the ASIO SDK. The
  earlier AudiForm 2 License 1.0 is withdrawn; its exclusion clause cannot
  stand under the GPL, which forbids added restrictions.
- **The clean-room rule stays.** No code is taken from Audiform 1 or from
  the editor it was forked from: every Audiform 1 file carries that
  editor's copyright line, and the project lead does not want AudiForm 2 to
  owe it credit. Behaviour only, as before.
- **Contributions** need the relicensing grant in `CONTRIBUTING.md`, so the
  project lead keeps the freedom to license AudiForm 2 differently later.
