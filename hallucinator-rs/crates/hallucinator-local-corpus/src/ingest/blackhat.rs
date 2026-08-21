//! Parse a Black Hat edition's `sessions.json` (Briefings schedule data)
//! into corpus records.
//!
//! Unlike every other source in this crate, this one is JSON, not HTML —
//! `blackhat.com`'s schedule page (`{edition}-{YY}/briefings/schedule/
//! index.html`) is a client-rendered Handlebars shell; the actual talk
//! data loads from a sibling `sessions.json` the page fetches at
//! runtime. That JSON is itself only reachable through the Wayback
//! Machine: the live site 403s every request behind a Cloudflare WAF
//! challenge (confirmed during investigation — this is a bot-management
//! block, not a `robots.txt` policy; `robots.txt` there says `Allow: /`).
//! No BibTeX/CSV export or documented public API exists; this reads the
//! same undocumented internal data file the front-end does.
//!
//! Schema: `{"sessions": {id: {...}}, "speakers": {id: {...}}}` —
//! sessions reference speakers by `person_id`, joined against the
//! top-level `speakers` map (whose keys are the string form of that same
//! id). A session with no speakers or marked `cancelled` is logistics
//! (e.g. "Briefings Breakfast") or didn't happen — skipped, not
//! inserted.

use std::collections::HashMap;

use serde::Deserialize;

use crate::db::NewPublication;

#[derive(Debug, Deserialize)]
struct SessionsDoc {
    #[serde(default)]
    sessions: HashMap<String, Session>,
    #[serde(default)]
    speakers: HashMap<String, Speaker>,
}

#[derive(Debug, Deserialize)]
struct Session {
    #[serde(default)]
    title: String,
    #[serde(default)]
    speakers: Vec<SpeakerRef>,
    #[serde(default)]
    cancelled: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SpeakerRef {
    person_id: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct Speaker {
    #[serde(default)]
    first_name: String,
    #[serde(default)]
    last_name: String,
}

/// Parse a Black Hat `sessions.json` document into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"blackhatus2025"`.
pub fn parse_sessions(json: &str, source_tag: &str) -> Vec<NewPublication> {
    let Ok(doc) = serde_json::from_str::<SessionsDoc>(json) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for session in doc.sessions.values() {
        let title = session.title.trim();
        if title.is_empty() {
            continue;
        }
        if session.cancelled.as_ref().is_some_and(is_truthy) {
            continue;
        }

        let authors: Vec<String> = session
            .speakers
            .iter()
            .filter_map(|s| {
                let key = value_as_key(&s.person_id)?;
                let speaker = doc.speakers.get(&key)?;
                let name = format!("{} {}", speaker.first_name.trim(), speaker.last_name.trim());
                let name = name.trim().to_string();
                (!name.is_empty()).then_some(name)
            })
            .collect();
        // No speakers at all means this is a logistics entry (breakfast,
        // networking break, ...), not a citable talk.
        if authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title: title.to_string(),
            authors,
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

/// A JSON value truthy enough to mean "cancelled": `1`, `true`, or the
/// strings `"1"`/`"true"`. Anything else (including `0`, `false`,
/// missing) means not cancelled.
fn is_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_i64() == Some(1) || n.as_u64() == Some(1),
        serde_json::Value::String(s) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// `person_id` may be serialized as a JSON number or a string; the
/// top-level `speakers` map is keyed by its string form either way.
fn value_as_key(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "sessions": {
            "48195": {
                "id": 48195,
                "title": "Keynote: Three Decades in Cybersecurity",
                "speakers": [{"person_id": 31896, "role": "Speaker"}],
                "cancelled": 0
            },
            "48200": {
                "id": 48200,
                "title": "A Two-Speaker Panel",
                "speakers": [{"person_id": 31896, "role": "Speaker"}, {"person_id": 31897, "role": "Speaker"}],
                "cancelled": "0"
            },
            "48210": {
                "id": 48210,
                "title": "Briefings Breakfast",
                "speakers": [],
                "cancelled": 0
            },
            "48220": {
                "id": 48220,
                "title": "A Cancelled Talk",
                "speakers": [{"person_id": 31896, "role": "Speaker"}],
                "cancelled": 1
            }
        },
        "speakers": {
            "31896": {"person_id": 31896, "first_name": "Mikko", "last_name": "Hypponen", "company": "WithSecure"},
            "31897": {"person_id": 31897, "first_name": "Jane", "last_name": "Doe"}
        }
    }"#;

    #[test]
    fn test_parse_keynote_with_one_speaker() {
        let out = parse_sessions(SAMPLE, "blackhatus2025");
        let keynote = out.iter().find(|r| r.title.starts_with("Keynote")).unwrap();
        assert_eq!(keynote.authors, vec!["Mikko Hypponen"]);
        assert_eq!(keynote.source, "blackhatus2025");
    }

    #[test]
    fn test_multi_speaker_panel_joined_from_speakers_map() {
        let out = parse_sessions(SAMPLE, "blackhatus2025");
        let panel = out.iter().find(|r| r.title.contains("Panel")).unwrap();
        assert_eq!(panel.authors, vec!["Mikko Hypponen", "Jane Doe"]);
    }

    #[test]
    fn test_logistics_entry_with_no_speakers_skipped() {
        let out = parse_sessions(SAMPLE, "blackhatus2025");
        assert!(!out.iter().any(|r| r.title.contains("Breakfast")));
    }

    #[test]
    fn test_cancelled_talk_skipped() {
        let out = parse_sessions(SAMPLE, "blackhatus2025");
        assert!(!out.iter().any(|r| r.title.contains("Cancelled")));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_parse_invalid_json_returns_empty() {
        assert!(parse_sessions("not json", "blackhatus2025").is_empty());
    }

    #[test]
    fn test_parse_empty_doc() {
        assert!(parse_sessions(r#"{"sessions": {}, "speakers": {}}"#, "blackhatus2025").is_empty());
    }
}
