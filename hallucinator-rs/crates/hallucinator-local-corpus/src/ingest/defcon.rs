//! Parse a DEF CON year's main-track speakers page
//! (`defcon.org/html/defcon-{NN}/dc-{NN}-speakers.html`) into corpus
//! records.
//!
//! DEF CON has no formal proceedings, no DOIs, and isn't indexed by
//! DBLP/CrossRef at all — yet its talks are cited constantly in academic
//! security papers. `robots.txt` on `defcon.org` explicitly disallows
//! `ClaudeBot`/`anthropic-ai` by name; per an explicit decision made with
//! the user (see conversation history), this venue is fetched via
//! Wayback Machine snapshots rather than the live site.
//!
//! Structure holds unchanged across DC29/31/32/33 (2021, 2023–2025):
//! `article.talk` per entry, `h3.talk-title`, one `h4.speaker` per
//! speaker (sibling tags for multi-speaker talks), each often containing
//! a nested `span.speaker-title` (company/affiliation) that must be
//! excluded from the name, not just tag-stripped — same trap as ICSE's
//! award-badge span, same fix (direct text-node children only).
//!
//! **DC30 (2022) is intentionally not supported**: that year has no
//! `speakers.html` at all — talk data instead lives on a differently-
//! shaped schedule-grid page with abstracts hosted off-site on DEF CON's
//! community forum, not on defcon.org itself. **DC28 (2020, the virtual
//! "Safe Mode" pandemic year) is also not supported**: it uses camelCase
//! class names (`talkTitle`, `speakerTitle`) from a visibly different,
//! smaller site build, and covers only 35 (mostly Twitch-linked) talks —
//! low value for the extra parser variant.
//!
//! Speaker names are frequently hacker handles rather than (or alongside)
//! real names, e.g. `Jeff "The Dark Tangent" Moss` or bare `richinseattle`
//! with no real name given at all — kept as-is, not split apart: DEF CON
//! citations in papers typically use the same displayed form, handle and
//! all, so preserving it as one string is what actually matches.
//!
//! Main track only (`speakers.html`) — villages/workshops/demo-labs/
//! creator-talks live on separate per-year pages with the same
//! `article.talk` shape and could be added the same way if wanted.

use scraper::{Html, Selector};

use crate::db::NewPublication;

/// Parse a DEF CON year's speakers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"defcon33"`.
pub fn parse_speakers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("article.talk").unwrap();
    let title_sel = Selector::parse("h3.talk-title").unwrap();
    let speaker_sel = Selector::parse("h4.speaker").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let authors: Vec<String> = item
            .select(&speaker_sel)
            .map(|el| direct_text(&el))
            .filter(|s| !s.is_empty())
            .collect();
        if authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            // No per-talk URL on this listing (only an in-page anchor id).
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

/// The text of just `el`'s direct text-node children, skipping nested
/// elements entirely — excludes a `span.speaker-title` affiliation
/// nested inside `h4.speaker` from bleeding into the name (same fix as
/// ICSE's award-badge span; see `icse::direct_text`).
fn direct_text(el: &scraper::ElementRef) -> String {
    el.children()
        .filter_map(|node| node.value().as_text())
        .map(|t| &**t)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
        <article class="talk" id="content_59463">
        <h3 class="talk-title">Welcome to DEF CON 33!</h3>
          <p class="time-room">Friday at 10:00 in LVCC<br />  20 minutes </p>
          <h4 class="speaker">Jeff "The Dark Tangent" Moss
          <span class="speaker-title">DEF CON Communications, Inc.</span>
          </h4>
        <p class="abstract"></p>
        </article>
        <article class="talk" id="content_60293">
        <h3 class="talk-title">BitUnlocker: Leveraging Windows Recovery to Extract BitLocker Secrets</h3>
          <p class="time-room">Friday at 10:00<br />  45 minutes </p>
          <h4 class="speaker">Alon "alon_leviev" Leviev </h4>
          <h4 class="speaker">Netanel Ben Simon </h4>
        <p class="abstract">...</p>
        </article>
    "##;

    #[test]
    fn test_parse_two_talks() {
        let out = parse_speakers(SAMPLE, "defcon33");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Welcome to DEF CON 33!");
        assert_eq!(out[0].source, "defcon33");
        assert_eq!(out[0].url, None);
    }

    #[test]
    fn test_speaker_title_span_excluded_from_name() {
        // Regression: without excluding the nested span, this used to
        // glue the affiliation onto the handle-bearing name.
        let out = parse_speakers(SAMPLE, "defcon33");
        assert_eq!(out[0].authors, vec!["Jeff \"The Dark Tangent\" Moss"]);
        assert!(!out[0].authors[0].contains("DEF CON Communications"));
    }

    #[test]
    fn test_multiple_speakers_as_sibling_tags() {
        let out = parse_speakers(SAMPLE, "defcon33");
        assert_eq!(
            out[1].authors,
            vec!["Alon \"alon_leviev\" Leviev", "Netanel Ben Simon"]
        );
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_speakers("<html></html>", "defcon33").is_empty());
    }
}
