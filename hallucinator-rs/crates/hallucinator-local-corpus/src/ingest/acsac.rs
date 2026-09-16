//! Parse ACSAC's accepted-papers page into corpus records.
//!
//! `acsac.org` publishes no BibTeX/CSV/JSON export. Its page format has
//! changed several times over recent years (parenthetical affiliations →
//! semicolon-separated → OpenConf-schedule-embedded → ACM-DL-embedded) —
//! this module covers only the current format
//! (`div.acsac-prose p > b`, `<br/>`-separated from `"Name (Affiliation),
//! ..."` authors on the same line), matching the scope of every other
//! venue here: current edition only, historical backfill is a separate
//! follow-up if wanted.
//!
//! Note: `acsac.org`'s `robots.txt` disallows generic bots (`Disallow: /`
//! for `User-agent: *`, allowlisting only major search engines) — no
//! technical enforcement was found, but per an explicit decision made
//! with the user (see conversation history, same call as DEF CON's),
//! this is scraped anyway.

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse an ACSAC accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"acsac2025"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    static TAG_RE: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"<[^>]+>").unwrap());

    let document = Html::parse_document(html);
    let p_sel = Selector::parse("p").unwrap();
    let bold_sel = Selector::parse("b, strong").unwrap();

    let mut out = Vec::new();
    for p in document.select(&p_sel) {
        let Some(title_el) = p.select(&bold_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        // Authors are whatever comes after the title's closing bold tag
        // in this paragraph.
        let p_html = p.html();
        let Some(idx) = p_html.find("</b>").or_else(|| p_html.find("</strong>")) else {
            continue;
        };
        let close_len = if p_html[idx..].starts_with("</b>") {
            4
        } else {
            8
        };
        let after_title = &p_html[idx + close_len..];
        let plain = TAG_RE.replace_all(after_title, " ");
        let authors = parse_paren_grouped_names(&plain);
        if authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <div class="acsac-prose">
        <h1>Accepted Papers</h1>
        <p><b>MoEvil: Poisoning Expert to Compromise the Safety of Mixture-of-Experts LLMs</b><br /> Jaehan Kim (KAIST), Seung Ho Na (KAIST), Sooel Son (KAIST)</p>
        <p><b>FLAME: Flexible and Lightweight Biometric Authentication Scheme in Malicious Environments</b><br /> Fuyi Wang (SUTD), Jianying Zhou (SUTD)</p>
        </div>
    "#;

    #[test]
    fn test_parse_two_papers() {
        let out = parse_accepted_papers(SAMPLE, "acsac2025");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "MoEvil: Poisoning Expert to Compromise the Safety of Mixture-of-Experts LLMs"
        );
        assert_eq!(
            out[0].authors,
            vec!["Jaehan Kim", "Seung Ho Na", "Sooel Son"]
        );
        assert_eq!(out[0].source, "acsac2025");
        assert_eq!(out[1].authors, vec!["Fuyi Wang", "Jianying Zhou"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "acsac2025").is_empty());
    }
}
