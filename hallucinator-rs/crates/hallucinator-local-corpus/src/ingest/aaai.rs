//! Parse an AAAI proceedings issue's table of contents (`ojs.aaai.org`)
//! into corpus records.
//!
//! AAAI's formal proceedings run on an OJS (Open Journal Systems)
//! instance, organized as one *volume* per conference year, split into
//! many *issues* (~94 papers each, one per track/session) — there's no
//! single page listing an entire year, so importing a full edition means
//! one fetch per issue (`https://ojs.aaai.org/index.php/AAAI/issue/view/
//! {issue_id}`; enumerate issue ids for a year from
//! `.../issue/archive`).
//!
//! Unlike the other venues here, AAAI's current gap is *prospective, not
//! retrospective*: as of 2026-08-21, AAAI-26 (concluded Jan 2026) is
//! already fully published here, and AAAI-27 hasn't had accept/reject
//! decisions yet (due ~Nov 2026). This module exists so the corpus is
//! ready the moment that changes — AAAI publishes a PDF program schedule
//! shortly before each conference (title+authors, no affiliations,
//! parseable but out of scope for this HTML-only ingest module) followed
//! months later by these OJS issue pages.
//!
//! No BibTeX/CSV export was reliably reachable (the site's citation
//! download endpoints returned connection resets during investigation,
//! unlike the plain TOC/article pages) — this scrapes the same TOC HTML
//! that AAAI's own users normally read.

use scraper::{Html, Selector};

use crate::db::NewPublication;

/// Parse one AAAI OJS issue's table-of-contents page into publication
/// records. No affiliations are present on this page (only on each
/// article's own page, one extra fetch per paper — not worth it for the
/// same reason NDSS 2019's title de-truncation wasn't: see that module).
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"aaai26"`.
pub fn parse_issue_toc(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("div.obj_article_summary").unwrap();
    let title_sel = Selector::parse("h3.title a").unwrap();
    let authors_sel = Selector::parse("div.authors").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = title_el.value().attr("href").map(|s| s.to_string());

        let authors: Vec<String> = item
            .select(&authors_sel)
            .next()
            .map(|el| el.text().collect::<String>())
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        out.push(NewPublication {
            title,
            authors,
            url,
            source: source_tag.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <li>
        <div class="obj_article_summary">
            <h3 class="title"><a id="article-36958" href="https://ojs.aaai.org/index.php/AAAI/article/view/36958">Resource Efficient Sleep Staging via Multi-Level Masking and Prompt Learning</a></h3>
            <div class="meta">
                <div class="authors">Lejun Ai, Yulong Li, Haodong Yi, Jixuan Xie, Yue Wang, Jia Liu, Min Chen, Rui Wang</div>
                <div class="pages">3-11</div>
            </div>
        </div>
        </li>
        <li>
        <div class="obj_article_summary">
            <h3 class="title"><a id="article-36959" href="https://ojs.aaai.org/index.php/AAAI/article/view/36959">AutoMalDesc: Large-Scale Script Analysis for Cyber Threat Research</a></h3>
            <div class="meta">
                <div class="authors">Alexandru-Mihai Apostu, Andrei Preda, Alexandra Daniela Damir</div>
                <div class="pages">12-20</div>
            </div>
        </div>
        </li>
    "#;

    #[test]
    fn test_parse_two_papers() {
        let out = parse_issue_toc(SAMPLE, "aaai26");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Resource Efficient Sleep Staging via Multi-Level Masking and Prompt Learning"
        );
        assert_eq!(
            out[0].url.as_deref(),
            Some("https://ojs.aaai.org/index.php/AAAI/article/view/36958")
        );
        assert_eq!(out[0].source, "aaai26");
        assert_eq!(
            out[0].authors,
            vec![
                "Lejun Ai",
                "Yulong Li",
                "Haodong Yi",
                "Jixuan Xie",
                "Yue Wang",
                "Jia Liu",
                "Min Chen",
                "Rui Wang",
            ]
        );
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_issue_toc("<html></html>", "aaai26").is_empty());
    }
}
