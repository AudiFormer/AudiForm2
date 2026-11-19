# Translating AudiForm 2

AudiForm 2's interface is written in English, and each other language is
one plain-text file in [`app/lang/`](../app/lang/). The files are built
into the program; nothing is downloaded and there's no web engine.

Built in: English, Español, Français, Deutsch, Português (Brasil),
Italiano, 日本語, 한국어, 简体中文 and Русский. To pick one, go to
**Edit > Preferences > Appearance > Language**. The change applies
immediately. The default, *System default*, follows the operating
system's display language and falls back to English. To try a language
for one run without changing the setting, start AudiForm 2 with the
environment variable `AF2_LANG` set to its code (for example
`AF2_LANG=ja`).

## The file format

```
# Comments start with #.
@name Deutsch

Cancel = Abbrechen
Save As... = Speichern unter...
Saved {0} = {0} gespeichert
```

- The `@name` line holds the language's name in that language, as the
  Language menu shows it.
- Each entry is `English = Translation`, one per line. The English text
  is the key and must match the program's text exactly, including
  capitals, `...` and punctuation.
- `{0}`, `{1}`, ... are filled in by the program with a file name, a
  number and so on. Keep every one, but move them wherever the
  sentence needs them.
- In either half, write `\n` for a line break, `\=` for an equals sign
  and `\\` for a backslash.
- A missing entry, or one with nothing after the `=`, shows the English.
- Shortcut keys (`Ctrl+S`), units (dB, Hz, ms), theme names and the
  brand name aren't translated.

Use the terms people already know from the leading audio editors in
that language, such as Adobe Audition's localized menus: for example
*Normalisieren*, *Fondu en entrée* and *ノーマライズ*.

## Fixing a translation without rebuilding

Put a file of the same name in the settings folder's `lang` folder:

- Windows: `%APPDATA%\Audiform27\lang\de.lang`
- macOS: `~/Library/Application Support/Audiform27/lang/de.lang`
- Linux: `~/.local/share/Audiform27/lang/de.lang` (or under `$XDG_DATA_HOME`)

It needs only the entries you want to change. They're read over the
built-in ones the first time that language is shown after AudiForm 2
starts, so restart after editing.

## Adding a language

1. Copy [`app/lang/en.lang`](../app/lang/en.lang), the template that
   lists every English text, to `app/lang/<code>.lang`. Use a BCP 47
   code such as `nl`, `pl`, `zh-TW` or `pt-PT`.
2. Set `@name` and fill in the right-hand sides.
3. To try it out, put the file in the settings folder's `lang` folder as
   above. It shows up in the Language menu with no rebuild.
4. To build it in, add it to `BUILT_IN` in
   [`app/src/i18n.rs`](../app/src/i18n.rs). If its script needs a font
   egui lacks (Thai, Arabic, Hebrew, Devanagari, ...), add system font
   paths for it in `install_fallback_fonts` in
   [`app/src/skin.rs`](../app/src/skin.rs). To have it chosen
   automatically, add its Windows LANGID to `os_language` and its code
   to `closest`.

## For developers

- Show text with `tr("English")`. When it has values in it, use
  `tr!("Saved {0} to {1}", name, path)`. For text kept in tables and
  translated where it's drawn, mark the literal with `mark("English")`
  and draw it with `tr(text)`. The key must be a string literal so the
  test can find it.
- Effect names, controls, choices and categories from `af27-dsp` and
  `af27-fx` are collected automatically, so wrap them in `tr()` where
  they're shown.
- Never translate a string that's saved, compared or used as an id.
  Translate it only where it's drawn.
- After adding or changing English text, run

  ```
  cargo test -p audiform27 write_template -- --ignored
  cargo test -p audiform27 every_language_has_every_key
  ```

  The first command regenerates `en.lang`. The second lists, for each
  language, the entries that are missing or whose `{0}` placeholders
  don't match. Add them before committing.
