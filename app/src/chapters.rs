// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Chapter and ad lists from markers: a plain timestamp list (for show
//! notes and video descriptions), CSV, or the open podcast chapters JSON
//! format (version 1.2.0) that podcast hosts and apps read. Ad slots go
//! in their own list, never among the chapters. Exported FLAC files also
//! carry the chapters inside (see `af2_io::export::chapter_comments`).

use af2_core::session::MarkerKind;
use af2_core::Marker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Timestamps,
    Csv,
    PodcastJson,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Timestamps, Format::Csv, Format::PodcastJson];
    pub fn name(self) -> &'static str {
        match self {
            Format::Timestamps => "Timestamp list (.txt)",
            Format::Csv => "Spreadsheet (.csv)",
            Format::PodcastJson => "Podcast chapters (.json)",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Format::Timestamps => "txt",
            Format::Csv => "csv",
            Format::PodcastJson => "json",
        }
    }
}

/// The markers of `kind`, in time order.
pub fn of_kind<'a>(markers: &'a [Marker], kind: &MarkerKind) -> Vec<&'a Marker> {
    let mut v: Vec<&Marker> = markers.iter().filter(|m| &m.kind == kind).collect();
    v.sort_by(|a, b| a.time.total_cmp(&b.time));
    v
}

/// `m:ss`, or `h:mm:ss` past an hour (as video sites want them).
pub fn stamp(t: f64) -> String {
    let s = t.max(0.0).floor() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn render(list: &[&Marker], format: Format) -> String {
    match format {
        Format::Timestamps => list
            .iter()
            .map(|m| format!("{} {}\n", stamp(m.time), m.name))
            .collect(),
        Format::Csv => {
            let mut s = String::from("Start (s),End (s),Title\n");
            for m in list {
                s.push_str(&format!(
                    "{:.3},{:.3},{}\n",
                    m.time,
                    m.time + m.length,
                    csv_field(&m.name)
                ));
            }
            s
        }
        Format::PodcastJson => {
            let chapters: Vec<serde_json::Value> = list
                .iter()
                .map(|m| {
                    let mut o = serde_json::json!({
                        "startTime": (m.time * 1000.0).round() / 1000.0,
                        "title": m.name,
                    });
                    if m.length > 0.0 {
                        o["endTime"] = serde_json::json!(((m.time + m.length) * 1000.0).round() / 1000.0);
                    }
                    o
                })
                .collect();
            let doc = serde_json::json!({ "version": "1.2.0", "chapters": chapters });
            serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
        }
    }
}

/// Things players and video sites reject or handle badly.
pub fn warnings(list: &[&Marker], duration: f64) -> Vec<String> {
    let mut w = Vec::new();
    if list.is_empty() {
        return vec!["There are no chapter markers.".into()];
    }
    if list[0].time > 0.5 {
        w.push("The first chapter does not start at 0:00; video sites ignore the list without one.".into());
    }
    if list.len() < 3 {
        w.push("Video sites want at least three chapters.".into());
    }
    let mut ends: Vec<f64> = list.iter().skip(1).map(|m| m.time).collect();
    ends.push(duration);
    for (m, end) in list.iter().zip(ends) {
        if end - m.time < 10.0 {
            w.push(format!("\"{}\" is shorter than 10 seconds.", m.name));
        }
        if m.name.chars().count() > 60 {
            w.push(format!("\"{}…\" is a long title; apps may cut it short.", m.name.chars().take(24).collect::<String>()));
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str, time: f64, kind: MarkerKind) -> Marker {
        Marker {
            name: name.into(),
            time,
            length: 0.0,
            ext_id: None,
            kind,
        }
    }

    #[test]
    fn lists_chapters_in_order_and_leaves_ads_out() {
        let ms = vec![
            mk("Interview", 95.0, MarkerKind::Chapter),
            mk("Sponsor", 60.0, MarkerKind::AdSlot),
            mk("Intro, part \"one\"", 0.0, MarkerKind::Chapter),
            mk("Wrap-up", 3725.0, MarkerKind::Chapter),
        ];
        let ch = of_kind(&ms, &MarkerKind::Chapter);
        assert_eq!(render(&ch, Format::Timestamps), "0:00 Intro, part \"one\"\n1:35 Interview\n1:02:05 Wrap-up\n");
        let csv = render(&ch, Format::Csv);
        assert!(csv.contains("0.000,0.000,\"Intro, part \"\"one\"\"\""), "{csv}");
        let json: serde_json::Value = serde_json::from_str(&render(&ch, Format::PodcastJson)).unwrap();
        assert_eq!(json["version"], "1.2.0");
        assert_eq!(json["chapters"][1]["startTime"], 95.0);
        assert_eq!(json["chapters"].as_array().unwrap().len(), 3);
        assert!(warnings(&ch, 4000.0).is_empty(), "{:?}", warnings(&ch, 4000.0));
        let ads = of_kind(&ms, &MarkerKind::AdSlot);
        assert_eq!(render(&ads, Format::Timestamps), "1:00 Sponsor\n");
    }

    #[test]
    fn warns_about_lists_sites_reject() {
        let ms = vec![mk("A", 5.0, MarkerKind::Chapter), mk("B", 9.0, MarkerKind::Chapter)];
        let ch = of_kind(&ms, &MarkerKind::Chapter);
        let w = warnings(&ch, 100.0);
        assert_eq!(w.len(), 3, "{w:?}");
    }
}
