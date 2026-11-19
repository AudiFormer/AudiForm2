// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The interface's languages.
//!
//! English is the source language and the English text is the key: code
//! writes `tr("Save As...")` (or `tr!("Saved {0}", name)` when there is
//! something to fill in) and gets the current language's text back, or
//! the English when that language has no entry for it. Strings kept in
//! tables and translated where they are drawn are marked with
//! [`mark`] so the completeness test finds them.
//!
//! Each language is a plain-text file in `app/lang/` (`es.lang`,
//! `ja.lang`, ...), built into the program. A file of the same name in
//! `<settings folder>/lang/` is read over it at start-up, so a
//! translation can be corrected without rebuilding, and a new file there
//! adds a language. See `docs/translating.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

/// Fills a translated text's `{0}`, `{1}`, ... with the values given.
/// The key must be a literal so the completeness test can see it.
macro_rules! tr {
    ($key:literal $(,)?) => {
        $crate::i18n::tr($key)
    };
    ($key:literal, $($arg:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($key),
            &[$(&$arg as &dyn ::std::fmt::Display),+],
        )
    };
}

/// The languages built into the program: (code, file).
pub const BUILT_IN: [(&str, &str); 10] = [
    ("en", ""),
    ("es", include_str!("../lang/es.lang")),
    ("fr", include_str!("../lang/fr.lang")),
    ("de", include_str!("../lang/de.lang")),
    ("pt-BR", include_str!("../lang/pt-BR.lang")),
    ("it", include_str!("../lang/it.lang")),
    ("ja", include_str!("../lang/ja.lang")),
    ("ko", include_str!("../lang/ko.lang")),
    ("zh-CN", include_str!("../lang/zh-CN.lang")),
    ("ru", include_str!("../lang/ru.lang")),
];

/// One language's table.
pub struct Lang {
    pub code: String,
    /// The language's name in itself ("Español", "日本語").
    pub name: String,
    table: HashMap<Box<str>, &'static str>,
}

/// The language now shown; null is English.
static CURRENT: AtomicPtr<Lang> = AtomicPtr::new(std::ptr::null_mut());

/// Languages read so far, by code. Each is read once and kept for the
/// life of the program, so `tr` can hand out `&'static` text.
fn loaded() -> &'static Mutex<HashMap<String, &'static Lang>> {
    static L: OnceLock<Mutex<HashMap<String, &'static Lang>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The text for `english` in the current language.
pub fn tr(english: &str) -> &str {
    let p = CURRENT.load(Ordering::Acquire);
    if p.is_null() {
        return english;
    }
    // SAFETY: CURRENT only ever holds null or a leaked `&'static Lang`.
    let lang: &'static Lang = unsafe { &*p };
    lang.table.get(english).copied().unwrap_or(english)
}

/// Marks a string to be translated where it is drawn (with [`tr`]).
pub const fn mark(english: &str) -> &str {
    english
}

/// Puts `args` in place of `{0}`, `{1}`, ... in `text`.
pub fn fill(text: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let n = after.find('}').and_then(|j| {
            after[..j]
                .parse::<usize>()
                .ok()
                .filter(|&n| n < args.len())
                .map(|n| (n, j))
        });
        match n {
            Some((n, j)) => {
                use std::fmt::Write;
                let _ = write!(out, "{}", args[n]);
                rest = &after[j + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The code of the language now shown ("en" for English).
pub fn current() -> String {
    let p = CURRENT.load(Ordering::Acquire);
    if p.is_null() {
        "en".into()
    } else {
        // SAFETY: as in `tr`.
        unsafe { (*p).code.clone() }
    }
}

/// Shows the interface in `code` (None: the system's language). Codes
/// with no table fall back to English.
pub fn set(code: Option<&str>) {
    let code = match code {
        Some(c) => c.to_string(),
        None => system_language(),
    };
    let lang = if code == "en" { None } else { load(&code) };
    let p = lang.map_or(std::ptr::null_mut(), |l| l as *const Lang as *mut Lang);
    CURRENT.store(p, Ordering::Release);
}

/// Every language there is: (code, name in itself), built-in ones
/// first, then any others found in `<settings folder>/lang/`.
pub fn languages() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = BUILT_IN
        .iter()
        .map(|(code, text)| {
            let name = if code.is_empty() || text.is_empty() {
                "English".to_string()
            } else {
                header(text, "name").unwrap_or_else(|| code.to_string())
            };
            (code.to_string(), name)
        })
        .collect();
    if let Ok(dir) = std::fs::read_dir(user_dir()) {
        let mut extra: Vec<(String, String)> = dir
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension()? != "lang" {
                    return None;
                }
                let code = path.file_stem()?.to_str()?.to_string();
                if BUILT_IN.iter().any(|(c, _)| *c == code) {
                    return None;
                }
                let text = std::fs::read_to_string(&path).ok()?;
                let name = header(&text, "name").unwrap_or_else(|| code.clone());
                Some((code, name))
            })
            .collect();
        extra.sort();
        out.extend(extra);
    }
    out
}

/// Where users put their own language files.
pub fn user_dir() -> std::path::PathBuf {
    crate::state::data_dir().join("lang")
}

/// Reads language `code`: the built-in file, then the user's file of the
/// same name over it. None when neither exists.
fn load(code: &str) -> Option<&'static Lang> {
    let mut map = loaded().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(l) = map.get(code) {
        return Some(l);
    }
    let built_in = BUILT_IN
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, t)| *t);
    let user = std::fs::read_to_string(user_dir().join(format!("{code}.lang"))).ok();
    if built_in.is_none() && user.is_none() {
        return None;
    }
    let mut lang = Lang {
        code: code.to_string(),
        name: code.to_string(),
        table: HashMap::new(),
    };
    for text in built_in.into_iter().chain(user.as_deref()) {
        if let Some(n) = header(text, "name") {
            lang.name = n;
        }
        for (k, v) in parse(text) {
            lang.table.insert(k.into(), Box::leak(v.into_boxed_str()));
        }
    }
    let lang: &'static Lang = Box::leak(Box::new(lang));
    map.insert(code.to_string(), lang);
    Some(lang)
}

/// A header line's value: `@name Español`.
fn header(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix('@')?.strip_prefix(key)?;
        rest.starts_with(char::is_whitespace)
            .then(|| rest.trim().to_string())
    })
}

/// A language file's entries: `English = Translation`, one per line.
/// Lines starting with `#` are comments and `@` lines are headers. In
/// both halves `\n` is a line break, `\=` an equals sign kept from
/// splitting the line, and `\\` a backslash. An entry with nothing after
/// the `=` is left out (falls back to English).
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
            continue;
        }
        let Some(split) = find_separator(line) else {
            continue;
        };
        let key = unescape(line[..split].trim_end());
        let value = unescape(line[split + 1..].trim_start());
        if !key.is_empty() && !value.is_empty() {
            out.push((key, value));
        }
    }
    out
}

/// The first `=` not escaped with a backslash.
fn find_separator(line: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '\\' if !escaped => escaped = true,
            '=' if !escaped => return Some(i),
            _ => escaped = false,
        }
    }
    None
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(ch) = it.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Writes `s` as one half of a language file line.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '=' => out.push_str("\\="),
            '#' if out.is_empty() => out.push_str("\\#"),
            '@' if out.is_empty() => out.push_str("\\@"),
            c => out.push(c),
        }
    }
    out
}

// ------------------------------------------------------------ the system's language

/// The built-in language closest to the system's, or "en".
pub fn system_language() -> String {
    os_language()
        .map(|tag| closest(&tag))
        .unwrap_or_else(|| "en".into())
}

/// Picks a built-in language for a BCP 47 / POSIX tag ("pt_BR.UTF-8",
/// "zh-Hans-CN", "de-AT").
pub fn closest(tag: &str) -> String {
    let tag = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    let lower = tag.to_ascii_lowercase();
    let primary = lower.split('-').next().unwrap_or("");
    let code = match primary {
        "pt" => "pt-BR",
        "zh" => "zh-CN",
        p => p,
    };
    if BUILT_IN.iter().any(|(c, _)| *c == code) {
        code.to_string()
    } else {
        "en".to_string()
    }
}

#[cfg(windows)]
fn os_language() -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: takes nothing, returns the display language's LANGID.
    let id = unsafe { GetUserDefaultUILanguage() };
    let code = match id & 0x3ff {
        0x09 => "en",
        0x0a => "es",
        0x0c => "fr",
        0x07 => "de",
        0x16 => "pt",
        0x10 => "it",
        0x11 => "ja",
        0x12 => "ko",
        0x04 => "zh",
        0x19 => "ru",
        _ => return None,
    };
    Some(code.into())
}

#[cfg(not(windows))]
fn os_language() -> Option<String> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            let first = v.split(':').next().unwrap_or("").to_string();
            if !first.is_empty() && first != "C" && first != "POSIX" {
                return Some(first);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Apps opened from the Finder get no LANG; ask the system.
        let out = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let tag = text
            .split(|c: char| c == '"' || c.is_whitespace() || c == ',' || c == '(')
            .find(|s| s.len() >= 2 && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))?;
        return Some(tag.to_string());
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests;
