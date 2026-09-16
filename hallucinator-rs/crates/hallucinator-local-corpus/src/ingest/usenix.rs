//! Parse USENIX Security's "technical-sessions" program page into corpus
//! records.
//!
//! Unlike NDSS, USENIX does expose per-paper BibTeX (`/biblio/export/bibtex/
//! {node_id}`) — but the technical-sessions listing itself already embeds
//! both the title and the full author+affiliation text for every paper in
//! one page, so a single fetch/parse covers the whole proceedings without
//! needing to enumerate and fetch ~377 individual BibTeX exports.
//!
//! This structure holds unchanged from 2026 back through 2017 (checked
//! all nine years directly, not sampled). 2016 needs two small tweaks:
//! the paper wrapper is `div.node-paper.node-teaser`, not
//! `article.node-paper`, and the affiliation is `<i>`-wrapped instead of
//! `<em>`-wrapped — both handled below without an era split, since
//! neither changes what's structurally happening. Every year except the
//! current one requires fetching through the Wayback Machine —
//! `usenix.org` 403s direct/bot requests unconditionally, not just for
//! old pages.

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::db::NewPublication;

const USENIX_BASE: &str = "https://www.usenix.org";

/// Parse a USENIX `technical-sessions` program page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"usenix2026"`.
pub fn parse_technical_sessions(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    // Drupal "neat_conference" theme markup — see the fixture captured
    // during investigation (`article.node-paper` wraps one paper's title,
    // author list, and metadata). 2016 uses `div.node-paper` instead of
    // `article.node-paper` for the same content.
    let item_sel = Selector::parse("article.node-paper, div.node-paper").unwrap();
    let title_sel = Selector::parse("h2 a").unwrap();
    let people_sel = Selector::parse(".field-name-field-paper-people-text").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = title_el.value().attr("href").map(resolve_href);

        let people_html: String = item
            .select(&people_sel)
            .next()
            .map(|el| el.html())
            .unwrap_or_default();
        let authors = extract_authors(&people_html);

        out.push(NewPublication {
            title,
            authors,
            url,
            source: source_tag.to_string(),
        });
    }
    out
}

/// Resolve a paper link `href` to an absolute `usenix.org` URL.
///
/// Handles two cases: a plain relative path (`"/conference/.../presentation/x"`,
/// what a live fetch returns) gets `USENIX_BASE` prepended. A Wayback
/// Machine archived page instead wraps the original URL as
/// `"/web/{timestamp}/{original_url}"` (this is what `--from-file` sees
/// when the page was saved via Wayback to work around USENIX's bot-block —
/// see module docs) — unwrap to the real target by taking the last
/// embedded `http(s)://` occurrence, so corpus URLs point at the live site
/// rather than an archive snapshot.
fn resolve_href(href: &str) -> String {
    match href.rfind("http://").or_else(|| href.rfind("https://")) {
        Some(pos) => href[pos..].to_string(),
        None => format!("{USENIX_BASE}{href}"),
    }
}

/// Extract author names from the `field-paper-people-text` fragment.
///
/// The field's HTML looks like:
/// `"Name, <em>Affiliation;</em> Name and Name, <em>Affiliation</em>"` —
/// affiliations are `<em>`-wrapped (`<i>`-wrapped in the 2016 snapshot)
/// and semicolon-separated per group; a group can list multiple authors
/// sharing one affiliation, joined by `", "` and/or `" and "` (with an
/// Oxford comma for 3+: `"A, B, and C"`).
///
/// Strategy: strip every `<em>...</em>`/`<i>...</i>` span (affiliations)
/// and any remaining tags, normalize `" and "` to `", "`, then split on
/// `,`/`;`.
fn extract_authors(people_html: &str) -> Vec<String> {
    static EM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<em>.*?</em>|<i>.*?</i>").unwrap());
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

    let without_affiliations = EM_RE.replace_all(people_html, "");
    let plain = TAG_RE.replace_all(&without_affiliations, "");
    let plain = plain.replace("&amp;", "&").replace(" and ", ", ");

    plain
        .split([',', ';'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("and"))
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <article id="node-318002" class="node node-paper view-mode-schedule">
            <h2><a href="/conference/usenixsecurity26/presentation/hu-zhenkai">Ajax: Fast Threshold Fully Homomorphic Encryption without Noise Flooding</a></h2>
            <div class="content">
                <div class="field field-name-field-paper-people-text field-type-text-long field-label-hidden">
                    <div class="field-items field-items"><div class="field-item odd">
                        <p>Zhenkai Hu, <em>Shanghai Jiao Tong University and State Key Laboratory of Cryptology;</em> Haofei Liang, <em>Shanghai Jiao Tong University;</em> Xiao Wang, <em>Northwestern University</em></p>
                    </div></div>
                </div>
            </div>
        </article>
        <article id="node-318028" class="node node-paper view-mode-schedule">
            <h2><a href="/conference/usenixsecurity26/presentation/li-zhihao">BatchBoot: Fast Batched Bootstrapping for TFHE scheme</a></h2>
            <div class="content">
                <div class="field field-name-field-paper-people-text field-type-text-long field-label-hidden">
                    <div class="field-items field-items"><div class="field-item odd">
                        <p>Zhihao Li, <em>Ant Digital Technologies, Ant Group;</em> Yuan Zhao and Lichun Li, <em>Ant Digital Technologies, Ant Group;</em> Jiaxing He, Changzheng Wei, and Ying Yan, <em>Ant Digital Technologies, Ant Group;</em> Lifeng Guo, <em>Shanxi University</em></p>
                    </div></div>
                </div>
            </div>
        </article>
    "#;

    #[test]
    fn test_parse_simple_paper() {
        let out = parse_technical_sessions(SAMPLE, "usenix2026");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Ajax: Fast Threshold Fully Homomorphic Encryption without Noise Flooding"
        );
        assert_eq!(
            out[0].url.as_deref(),
            Some("https://www.usenix.org/conference/usenixsecurity26/presentation/hu-zhenkai")
        );
        assert_eq!(out[0].source, "usenix2026");
        assert_eq!(
            out[0].authors,
            vec!["Zhenkai Hu", "Haofei Liang", "Xiao Wang"]
        );
    }

    #[test]
    fn test_parse_2016_variant_div_wrapper_and_italic_affiliation() {
        // 2016 uses `div.node-paper` (not `article.node-paper`) and
        // `<i>` (not `<em>`) for the affiliation.
        let html = r#"
            <div id="node-197264" class="node node-paper node-teaser paper-type-0 clearfix">
              <h2 class="node-title clearfix"><a href="/conference/usenixsecurity16/technical-sessions/presentation/razavi">Flip Feng Shui: Hammering a Needle in the Software Stack</a></h2>
              <div class="content">
                <div class="field field-name-field-paper-people-text field-type-text-long field-label-hidden"><div class="field-items"><div class="field-item odd"><p class="p1">Kaveh Razavi, Ben Gras, and Erik Bosman, <i>Vrije Universiteit Amsterdam</i></p></div></div></div>
              </div>
            </div>
        "#;
        let out = parse_technical_sessions(html, "usenix2016");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "Flip Feng Shui: Hammering a Needle in the Software Stack"
        );
        assert_eq!(
            out[0].authors,
            vec!["Kaveh Razavi", "Ben Gras", "Erik Bosman"]
        );
    }

    #[test]
    fn test_parse_multi_author_affiliation_groups_and_oxford_comma() {
        let out = parse_technical_sessions(SAMPLE, "usenix2026");
        assert_eq!(
            out[1].authors,
            vec![
                "Zhihao Li",
                "Yuan Zhao",
                "Lichun Li",
                "Jiaxing He",
                "Changzheng Wei",
                "Ying Yan",
                "Lifeng Guo",
            ]
        );
    }

    #[test]
    fn test_absolute_url_left_untouched() {
        let html = r#"<article class="node-paper"><h2><a href="https://www.usenix.org/conference/usenixsecurity26/presentation/x">Title Here For Test</a></h2></article>"#;
        let out = parse_technical_sessions(html, "usenix2026");
        assert_eq!(
            out[0].url.as_deref(),
            Some("https://www.usenix.org/conference/usenixsecurity26/presentation/x")
        );
    }

    #[test]
    fn test_wayback_wrapped_url_unwrapped_to_live_site() {
        // A page saved via Wayback Machine (the fallback for USENIX's
        // bot-block) wraps the original URL; the corpus should still
        // store the real usenix.org link, not the archive snapshot URL.
        let html = r#"<article class="node-paper"><h2><a href="/web/20260811181606/https://www.usenix.org/conference/usenixsecurity26/presentation/hu-zhenkai">Title Here For Test</a></h2></article>"#;
        let out = parse_technical_sessions(html, "usenix2026");
        assert_eq!(
            out[0].url.as_deref(),
            Some("https://www.usenix.org/conference/usenixsecurity26/presentation/hu-zhenkai")
        );
    }

    #[test]
    fn test_extract_authors_directly() {
        let authors = extract_authors(
            "<p>Zikai Zhou, <em>Tsinghua University;</em> William Seo and Edward Chen, <em>Carnegie Mellon University;</em> Alex Ozdemir, <em>Max Planck Institute for Security and Privacy;</em> Fraser Brown and Wenting Zheng, <em>Carnegie Mellon University</em></p>",
        );
        assert_eq!(
            authors,
            vec![
                "Zikai Zhou",
                "William Seo",
                "Edward Chen",
                "Alex Ozdemir",
                "Fraser Brown",
                "Wenting Zheng",
            ]
        );
    }
}
