# Podcast and spoken-word features

What the leading commercial editor and the upstream open editor offer
for podcast and voice work, and where AudiForm 2 stands. Compiled
2026-11-20 from public descriptions only: the commercial editor's help
pages, the upstream editor's public product pages, and Audiform's own
records (`docs/audiform1-features.md`, Audiform's `docs/decisions.md`).
Feature names here are generic. AudiForm 2's own names are in the last
columns. No code was taken from either product.

These tools are optional. They live in the Effects menu, the Window
menu, Multitrack > Spoken Word, and the **Spoken Word** workspace. The
Default and Music layouts are unchanged.

Key: **C** = the leading commercial editor, **U** = the upstream open
editor (public pages), **A1** = Audiform 1. Status: ✅ have · 🟡 partly ·
⬜ missing.

## Clean-up and repair

| Feature | Who has it | AF2 | Where in AF2 / plan |
|---|---|---|---|
| Live noise reduction that learns as it plays | C, U, A1 | ✅ | **Noise Reducer** (`af.noise_reducer`), Effects > Noise Reduction / Restoration and any rack |
| Noise reduction from a captured noise print | C, U, A1 | ✅ | Effects > **Capture Noise Print** and **Reduce Noise with Print…** (Waveform) |
| Hiss reduction | C, U, A1 | ✅ | **Hiss Reducer** (`af.hiss_reducer`) |
| Reverb / room reduction | C, U, A1 | ✅ | **Room Reducer** (`af.room_reducer`) |
| Automatic click / mouth-noise removal | C, U, A1 | ✅ | **Click Repair** (`af.click_repair`) live; Diagnostics scan |
| Clipping restoration | C, U, A1 | ✅ | **Clip Repair** (`af.clip_repair`) live; Diagnostics scan |
| Hum removal, rumble cut, DC offset | C, U, A1 | ✅ | Hum Remover, High-Pass Filter, Remove DC Offset (were already there) |
| De-essing | C, U, A1 | ✅ | De-Esser (was already there) |
| Spectral spot repair (brush, box, lasso) | C, U | 🟡 | Spectral selection and Spectrum submenu exist; Auto Heal is still greyed out. Plan: fill a spectral selection from its surroundings |
| Learned sound removal (remove a sound model) | C, U | ⬜ | Plan: learn a spectrum from a selection and subtract it, like the noise print but targeted |
| Automatic phase / azimuth correction | C, A1 | ⬜ | Plan: cross-correlate the channels and line them up (A1's Phase Align did this for two clips) |

## Levels and loudness

| Feature | Who has it | AF2 | Where in AF2 / plan |
|---|---|---|---|
| Speech volume leveler | C, U, A1 | ✅ | **Speech Leveler** (`af.speech_leveler`): rides toward a target and holds still in pauses |
| Breath / noise gate for voice | C, U, A1 | ✅ | **Breath Gate** (`af.breath_gate`), keyable by another track; Noise Gate as before |
| Match loudness of a file to a target | C, U, A1 | ✅ | Match Loudness (was already there) |
| Loudness check (integrated, true peak) | C, U, A1 | ✅ | Level Statistics (was already there) |
| Live loudness meter (momentary, short-term, range, true peak) | C | ✅ | **Loudness** panel, with podcast, streaming, broadcast and audiobook targets |
| Export normalised to a loudness target | C (media encoder), U, A1 | ✅ | Export dialog loudness target (was already there) |
| Even out several speakers | U, A1 (Speaker Balance), C (match clip volume) | ✅ | Multitrack > Spoken Word > **Speaker Balance…** |
| One-slider dialogue panel (loudness, repair, clarity sliders) | C | ⬜ | Plan: a "Voice" panel that drives a chain of our effects from a few sliders |
| Match loudness across many files at once | C, U (batch) | ⬜ | Plan: with batch processing |

## Editing

| Feature | Who has it | AF2 | Where in AF2 / plan |
|---|---|---|---|
| Find and delete silence / dead air in a file | C, U, A1 | ✅ | **Diagnostics** panel: Pauses and Dead Air (delete keeping a gap, turn down, or mark), with presets and Find Levels |
| Mark the parts with sound | C, U, A1 | ✅ | Diagnostics: Mark Sound |
| Scan for clicks and clipping, review, fix | C, U, A1 | ✅ | Diagnostics: Clicks and Mouth Noise, Clipped Peaks |
| Strip silence from multitrack clips | C, U | ✅ | Multitrack > Spoken Word > **Strip Silence…** |
| Ripple delete | C, U, A1 | ⬜ | Plan: Multitrack ripple delete of selected clips, gaps and time selection (kept out of the clip drag code under other work) |
| Punch-and-roll recording | C, U, A1 | 🟡 | Recording works; punch-in with pre-roll is in the parity list as still to do |
| Music bed fitted to a length (remix) | C, U, A1 | ⬜ | Plan: cut and repeat whole musical sections to a target length |
| Text-to-speech generator | C | ⬜ | Plan: Windows' speech voices, behind a feature flag |
| Transcription, text-based editing, filler-word removal | A1 (planned, never built), U (planned) | ⬜ | Needs a local speech model; out of scope for now. The commercial editor does not have it either |

## Multitrack and mixing

| Feature | Who has it | AF2 | Where in AF2 / plan |
|---|---|---|---|
| Auto ducking of music under dialogue | C, A1 (sidechain) | ✅ | Multitrack > Spoken Word > **Auto Duck…** writes volume automation; live sidechain ducking still works through keyed effects |
| Podcast session template | C, U (host profiles), A1 | ✅ | Multitrack > Spoken Word > **Add Podcast Tracks** (Host, Guest with a clean-up chain, Music Bed, Sound Effects) |
| Voice chain presets | C, U, A1 | ✅ | Rack presets: **Spoken word** group (Voice Clean-Up, Remote Guest Clean-Up, Audiobook Narration, Breath Gate, Speech Leveler) and new Repair presets, beside the existing Podcast Vocal presets |
| Host / guest mic profiles | U, A1 | 🟡 | The Spoken word presets cover the chains. Plan: per-track profile picker |

## Markers, chapters and delivery

| Feature | Who has it | AF2 | Where in AF2 / plan |
|---|---|---|---|
| Marker types (cue, chapter, ad slot, range) | C (cue, range), U, A1 | ✅ | Markers panel: kind per marker, kind for new markers; `.audfrm` kinds kept |
| Chapter export (timestamps, CSV, podcast chapters JSON) | U, A1 | ✅ | Markers panel > **Export…** > Chapters / Ad Slots, with warnings for lists video sites reject |
| Chapters inside exported files | U, A1 | 🟡 | FLAC ✅ (`CHAPTERnnn` comments). MP3 needs MP3 export first; WAV gets them as cue markers |
| Chapters read when a file opens | U, A1 | ✅ | Opened files' chapters become Chapter markers |
| Export ranges as separate files | C | ⬜ | Plan: Export > Ranges as Files |
| Publish package (episode metadata, notes, MP3) | U, A1 | ⬜ | Plan: needs MP3 export and a metadata editor |

## Clean-room notes

- The new effects use Audiform's `.audfrm` slot ids and saved control keys
  (`altaudio.speech_leveler` and so on), so voice chains in Audiform sessions
  load and play. Only those interface names, with their ranges and
  defaults, are matched. The processing, descriptions and code are new
  (see `docs/clean-room.md`).
- Auto Duck, Diagnostics, Strip Silence, the Loudness meter, chapter export
  and Speaker Balance are written for AudiForm 2 from the public behaviour
  described above.
