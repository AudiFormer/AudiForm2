// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Checks that every language file has every key the interface asks
//! for, with the same `{0}` placeholders, and that `lang/en.lang` (the
//! template translators start from) is up to date.
//!
//! After adding or changing English text, regenerate the template with
//! `cargo test -p audiform27 write_template -- --ignored`.

use super::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every Rust file under `dir`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Reads the Rust string literal at the start of `s` (just after the
/// opening quote): its text, or None if it does not close.
fn literal(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(ch) = it.next() {
        match ch {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                'u' => {
                    let mut hex = String::new();
                    it.next(); // {
                    for c in it.by_ref() {
                        if c == '}' {
                            break;
                        }
                        hex.push(c);
                    }
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                '\n' | '\r' => {
                    while it.peek().is_some_and(|c| c.is_whitespace()) {
                        it.next();
                    }
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    None
}

/// The keys the source code asks for: the literals in `tr("...")`,
/// `tr!("..."...)` and `mark("...")`.
fn source_keys() -> BTreeSet<String> {
    let mut files = Vec::new();
    sources(&app_dir().join("src"), &mut files);
    let mut keys = BTreeSet::new();
    for f in files {
        if f.ends_with("i18n.rs")
            || f.ends_with("tests.rs") && f.parent().unwrap().ends_with("i18n")
        {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for pat in ["tr(", "tr!(", "mark("] {
            let mut from = 0;
            while let Some(i) = text[from..].find(pat) {
                let at = from + i;
                from = at + pat.len();
                let before = text[..at].chars().next_back();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                // rustfmt may put the literal on the next line.
                let rest = text[from..].trim_start();
                let Some(rest) = rest.strip_prefix('"') else {
                    continue;
                };
                if let Some(k) = literal(rest) {
                    keys.insert(k);
                }
            }
        }
    }
    keys
}

/// Keys that come from the effect crates rather than this one's source:
/// effect names, descriptions, controls, choices, groups and categories.
fn dynamic_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for (group, list) in af27_dsp::effects::GROUPS {
        keys.insert(group.to_string());
        for e in list.iter() {
            keys.insert(e.name().to_string());
            for p in e.params() {
                keys.insert(p.name.to_string());
            }
        }
    }
    for info in af27_fx::registry() {
        keys.insert(info.name.to_string());
        keys.insert(info.category.to_string());
        if !info.description.is_empty() {
            keys.insert(info.description.to_string());
        }
        for p in &info.params {
            keys.insert(p.name.to_string());
            if !p.group.is_empty() {
                keys.insert(p.group.to_string());
            }
            for c in p.choices.iter() {
                keys.insert(c.to_string());
            }
        }
    }
    keys.retain(|k| k.chars().any(|c| c.is_alphabetic()));
    keys
}

fn all_keys() -> BTreeSet<String> {
    let mut k = source_keys();
    k.extend(dynamic_keys());
    k
}

fn placeholders(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) if after[..j].parse::<usize>().is_ok() => {
                out.insert(after[..j].to_string());
                rest = &after[j + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

fn template(keys: &BTreeSet<String>) -> String {
    let mut s = String::from(
        "# AudiForm 2 language file template: every English text the interface\n\
         # shows. Generated by `cargo test -p audiform27 write_template -- --ignored`;\n\
         # do not edit by hand. See docs/translating.md.\n\
         @name English\n\n",
    );
    for k in keys {
        s.push_str(&escape(k));
        s.push_str(" =\n");
    }
    s
}

#[test]
fn every_language_has_every_key() {
    let keys = all_keys();
    assert!(keys.len() > 200, "found only {} keys", keys.len());
    let mut report = String::new();
    for (code, text) in BUILT_IN.iter().skip(1) {
        let entries = parse(text);
        let have: BTreeSet<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        let missing: Vec<&String> = keys.iter().filter(|k| !have.contains(k.as_str())).collect();
        if !missing.is_empty() {
            report.push_str(&format!("\n{code}.lang is missing {}:\n", missing.len()));
            for k in missing {
                report.push_str(&format!("  {}\n", escape(k)));
            }
        }
        for (k, v) in &entries {
            if placeholders(k) != placeholders(v) {
                report.push_str(&format!(
                    "\n{code}.lang: placeholders differ in \"{k}\" = \"{v}\"\n"
                ));
            }
        }
        let stale: Vec<&&str> = have.iter().filter(|k| !keys.contains(**k)).collect();
        if !stale.is_empty() {
            eprintln!(
                "{code}.lang has {} keys no longer used: {stale:?}",
                stale.len()
            );
        }
        assert!(
            header(text, "name").is_some(),
            "{code}.lang has no @name line"
        );
    }
    assert!(report.is_empty(), "{report}");
}

#[test]
fn template_is_up_to_date() {
    let path = app_dir().join("lang/en.lang");
    let now = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        now == template(&all_keys()),
        "lang/en.lang is out of date: run `cargo test -p audiform27 write_template -- --ignored`"
    );
}

#[test]
#[ignore]
fn write_template() {
    std::fs::write(app_dir().join("lang/en.lang"), template(&all_keys())).unwrap();
}

#[test]
fn files_parse() {
    let e = parse("# c\n@name X\nA = B\nx \\= y = z\\nw\nEmpty =\n\\#tag = t\n");
    assert_eq!(
        e,
        vec![
            ("A".into(), "B".into()),
            ("x = y".into(), "z\nw".into()),
            ("#tag".into(), "t".into()),
        ]
    );
    assert_eq!(
        header("@name Español\n", "name").as_deref(),
        Some("Español")
    );
    for s in ["a = b", "#x\ny", "back\\slash", "@at"] {
        assert_eq!(parse(&format!("{} = v", escape(s)))[0].0, s);
    }
}

#[test]
fn fills_placeholders() {
    assert_eq!(fill("{1} then {0}", &[&"a", &2]), "2 then a");
    assert_eq!(fill("{x} {5} {", &[&1]), "{x} {5} {");
}

#[test]
fn picks_the_closest_language() {
    assert_eq!(closest("pt_BR.UTF-8"), "pt-BR");
    assert_eq!(closest("pt-PT"), "pt-BR");
    assert_eq!(closest("zh-Hans-CN"), "zh-CN");
    assert_eq!(closest("de_AT"), "de");
    assert_eq!(closest("nl-NL"), "en");
    assert_eq!(closest("ja"), "ja");
}

#[test]
fn loads_languages() {
    // (Leaves the shown language alone: other tests read English text.)
    let de = load("de").unwrap();
    assert_eq!(de.name, "Deutsch");
    assert!(de.table.get("Cancel").is_some_and(|t| *t != "Cancel"));
    assert!(load("xx-none").is_none());
    assert_eq!(tr("no such key"), "no such key");
    let names: Vec<String> = languages().into_iter().map(|(_, n)| n).collect();
    for n in ["English", "日本語", "Русский", "简体中文", "한국어"] {
        assert!(names.iter().any(|x| x == n), "{n} missing from {names:?}");
    }
}
