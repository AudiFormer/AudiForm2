// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Meaning checks on an Audiform session document: things JSON parsing
//! cannot catch, such as a clip pointing at media that is not in the file
//! or buses routed in a circle. A session with problems still opens; the
//! problems are listed so the user knows why something looks wrong.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// One problem: where it is (`tracks[2].clips[0]`) and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub at: String,
    pub what: String,
}

fn issue(out: &mut Vec<Issue>, at: impl Into<String>, what: impl Into<String>) {
    out.push(Issue {
        at: at.into(),
        what: what.into(),
    });
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// Checks a session document (after its header was checked).
pub fn validate(v: &Value) -> Vec<Issue> {
    let mut out = Vec::new();
    match v["sample_rate"].as_u64() {
        Some(r) if r > 0 => {}
        _ => issue(&mut out, "sample_rate", "the sample rate must be above 0"),
    }
    if let Some(b) = v.get("bit_depth").filter(|b| !b.is_null()) {
        if !matches!(b.as_u64(), Some(16 | 24 | 32)) {
            issue(
                &mut out,
                "bit_depth",
                format!("bit depth {b} is not 16, 24 or 32"),
            );
        }
    }

    // Ids are unique across every kind of object.
    let mut seen: HashSet<String> = HashSet::new();
    let mut id_once = |out: &mut Vec<Issue>, at: String, o: &Value| {
        if let Some(id) = o["id"].as_str() {
            if !seen.insert(id.to_string()) {
                issue(out, at, format!("the id {id} is used more than once"));
            }
        }
    };
    for (i, m) in list(v, "media").iter().enumerate() {
        id_once(&mut out, format!("media[{i}]"), m);
    }
    for key in ["tracks", "buses"] {
        for (i, t) in list(v, key).iter().enumerate() {
            id_once(&mut out, format!("{key}[{i}]"), t);
            for (j, c) in list(t, "clips").iter().enumerate() {
                id_once(&mut out, format!("{key}[{i}].clips[{j}]"), c);
            }
        }
    }
    for (i, m) in list(v, "markers").iter().enumerate() {
        id_once(&mut out, format!("markers[{i}]"), m);
    }

    let media: HashSet<&str> = list(v, "media")
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    let buses: HashSet<&str> = list(v, "buses")
        .iter()
        .filter_map(|b| b["id"].as_str())
        .collect();
    for key in ["tracks", "buses"] {
        for (i, t) in list(v, key).iter().enumerate() {
            let at = format!("{key}[{i}]");
            if t["output"]["type"].as_str() == Some("bus") {
                let b = t["output"]["bus_id"].as_str().unwrap_or_default();
                if !buses.contains(b) {
                    issue(
                        &mut out,
                        format!("{at}.output"),
                        "goes to a bus that is not in the session",
                    );
                }
            }
            for (j, s) in list(t, "sends").iter().enumerate() {
                let b = s["bus_id"].as_str().unwrap_or_default();
                if !buses.contains(b) {
                    issue(
                        &mut out,
                        format!("{at}.sends[{j}]"),
                        "sends to a bus that is not in the session",
                    );
                }
            }
            for (j, c) in list(t, "clips").iter().enumerate() {
                let at = format!("{at}.clips[{j}]");
                let m = c["media_id"].as_str().unwrap_or_default();
                if !media.contains(m) {
                    issue(
                        &mut out,
                        at.clone(),
                        "uses media that is not in the session",
                    );
                }
                let (a, b) = (c["source_in"].as_f64(), c["source_out"].as_f64());
                if let (Some(a), Some(b)) = (a, b) {
                    if b <= a {
                        issue(&mut out, at, "ends before it starts in its file");
                    }
                }
            }
        }
    }

    // Bus outputs must not come back round to themselves.
    let next: HashMap<&str, &str> = list(v, "buses")
        .iter()
        .filter_map(|b| {
            let id = b["id"].as_str()?;
            (b["output"]["type"].as_str() == Some("bus"))
                .then(|| b["output"]["bus_id"].as_str())
                .flatten()
                .map(|to| (id, to))
        })
        .collect();
    for (i, b) in list(v, "buses").iter().enumerate() {
        let Some(start) = b["id"].as_str() else {
            continue;
        };
        let mut at = start;
        for _ in 0..=next.len() {
            match next.get(at) {
                Some(&to) if to == start => {
                    issue(
                        &mut out,
                        format!("buses[{i}].output"),
                        "routes back into itself",
                    );
                    break;
                }
                Some(&to) => at = to,
                None => break,
            }
        }
    }

    for (i, m) in list(v, "markers").iter().enumerate() {
        let kind = m["kind"].as_str().unwrap_or("cue");
        if !m["chapter"].is_null() && kind != "chapter" {
            issue(
                &mut out,
                format!("markers[{i}]"),
                "has chapter details but is not a chapter",
            );
        }
        if !m["contracted_duration"].is_null() && kind != "ad_slot" {
            issue(
                &mut out,
                format!("markers[{i}]"),
                "has a contracted length but is not an ad slot",
            );
        }
    }
    out
}

/// One line for the user about what [`validate`] found.
pub fn notice(issues: &[Issue]) -> String {
    let shown: Vec<String> = issues
        .iter()
        .take(3)
        .map(|i| format!("{}: {}", i.at, i.what))
        .collect();
    let more = issues.len().saturating_sub(shown.len());
    let tail = if more > 0 {
        format!(" (and {more} more)")
    } else {
        String::new()
    };
    format!(
        "The session file has {} problem(s); it opened anyway. {}{tail}.",
        issues.len(),
        shown.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc() -> Value {
        json!({
            "format": "altaudio.session", "schema_version": 1, "sample_rate": 48000, "bit_depth": 24,
            "media": [{"id": "m1"}],
            "tracks": [{"id": "t1", "output": {"type": "bus", "bus_id": "b1"},
                "sends": [{"id": "s1", "bus_id": "b1"}],
                "clips": [{"id": "c1", "media_id": "m1", "source_in": 0, "source_out": 10}]}],
            "buses": [{"id": "b1", "output": {"type": "master"}, "sends": []}],
            "markers": [{"id": "k1", "kind": "chapter", "chapter": {"url": "x"}}]
        })
    }

    #[test]
    fn a_sound_session_has_no_issues() {
        assert_eq!(validate(&doc()), Vec::new());
    }

    #[test]
    fn broken_references_loops_and_bad_values_are_listed() {
        let mut v = doc();
        v["sample_rate"] = json!(0);
        v["bit_depth"] = json!(20);
        v["tracks"][0]["clips"][0]["media_id"] = json!("gone");
        v["tracks"][0]["clips"][0]["source_out"] = json!(0);
        v["tracks"][0]["sends"][0]["bus_id"] = json!("nowhere");
        v["buses"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "b2", "output": {"type": "bus", "bus_id": "b1"}}));
        v["buses"][0]["output"] = json!({"type": "bus", "bus_id": "b2"});
        v["markers"][0]["kind"] = json!("cue");
        v["markers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "m1", "kind": "cue", "contracted_duration": 5}));
        let got: Vec<String> = validate(&v).iter().map(|i| i.at.clone()).collect();
        for want in [
            "sample_rate",
            "bit_depth",
            "tracks[0].clips[0]",
            "tracks[0].sends[0]",
            "buses[0].output",
            "buses[1].output",
            "markers[0]",
            "markers[1]",
        ] {
            assert!(got.iter().any(|g| g == want), "{want} missing from {got:?}");
        }
        // The duplicate id ("m1" is media and a marker).
        assert!(validate(&v)
            .iter()
            .any(|i| i.what.contains("more than once")));
        let n = notice(&validate(&v));
        assert!(n.contains("problem(s)") && n.contains("more"), "{n}");
    }
}
