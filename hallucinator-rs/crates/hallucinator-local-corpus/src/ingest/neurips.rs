//! Parse a NeurIPS `papers.nips.cc` year index page into corpus records.
//!
//! Unlike the other venues here, NeurIPS's proceedings site (once a year's
//! papers are published, typically weeks after the conference) is a clean
//! bulk source: one index page per year lists every paper across all
//! tracks (Main Conference, Position Papers, Datasets and Benchmarks) with
//! title and full author list inline — no per-paper fetch needed, and
//! per-paper BibTeX exists too (unused here; the index already has
//! everything this corpus needs). No affiliations are published at all,
//! unlike NDSS/S&P/CCS.
//!
//! There used to be a `DatabaseBackend` that queried `papers.nips.cc`
//! live per-reference (`hallucinator-core/src/db/neurips.rs`), but it was
//! dead code — never registered — and stale: hardcoded to years
//! 2018-2023 and selectors (`li.author`) that no longer match the site.
//! This bulk-index approach replaces it: one fetch covers a whole year
//! instead of a live request per reference, consistent with how the
//! other venues here work.

use scraper::{Html, Selector};

use crate::db::NewPublication;

const NEURIPS_BASE: &str = "https://papers.nips.cc";

/// Parse a NeurIPS year-index page (e.g.
/// `papers.nips.cc/paper_files/paper/2025/vol38-main-conference`) into
/// publication records, across all tracks on the page.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"neurips2025"`.
pub fn parse_year_index(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    // `div.paper-content` is common to every entry regardless of which
    // track it's in (Main Conference / Position Papers / Datasets and
    // Benchmarks all share this wrapper, only a `data-track` attribute on
    // an ancestor differs) — selecting on it picks up every track without
    // needing to enumerate their attribute values.
    let item_sel = Selector::parse("div.paper-content").unwrap();
    let title_sel = Selector::parse("a").unwrap();
    let authors_sel = Selector::parse("span.paper-authors").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = title_el.value().attr("href").map(|href| {
            if href.starts_with("http") {
                href.to_string()
            } else {
                format!("{NEURIPS_BASE}{href}")
            }
        });

        let authors = item
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

    const SAMPLE: &str = r##"
        <ul>
        <li class="conference" data-track="conference">
            <div class="paper-content">
                <a title="paper title" href="/paper_files/paper/2025/hash/0010031a1b4910aa67edbda26a705518-Abstract-Conference.html">NeuralPLexer3: Accurate Biomolecular Complex Structure Prediction with Flow Models</a>
                <span class="paper-authors">Jarren Zhuoran Qiao, Feizhi Ding, Thomas Dresselhaus, Mia Rosenfeld</span>
            </div>
            <span class="paper-track-badge">Main Conference Track</span>
        </li>
        <li class="conference" data-track="datasets_benchmarks">
            <div class="paper-content">
                <a title="paper title" href="/paper_files/paper/2025/hash/deadbeef-Abstract-Datasets_Benchmarks.html">A Benchmark Paper For Testing</a>
                <span class="paper-authors">Solo Author</span>
            </div>
            <span class="paper-track-badge">Datasets and Benchmarks Track</span>
        </li>
        </ul>
    "##;

    #[test]
    fn test_parse_two_papers_across_tracks() {
        let out = parse_year_index(SAMPLE, "neurips2025");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "NeuralPLexer3: Accurate Biomolecular Complex Structure Prediction with Flow Models"
        );
        assert_eq!(
            out[0].url.as_deref(),
            Some(
                "https://papers.nips.cc/paper_files/paper/2025/hash/0010031a1b4910aa67edbda26a705518-Abstract-Conference.html"
            )
        );
        assert_eq!(out[0].source, "neurips2025");
        assert_eq!(
            out[0].authors,
            vec![
                "Jarren Zhuoran Qiao",
                "Feizhi Ding",
                "Thomas Dresselhaus",
                "Mia Rosenfeld"
            ]
        );
        // Datasets and Benchmarks track entry picked up too, not just
        // the main conference track.
        assert_eq!(out[1].title, "A Benchmark Paper For Testing");
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_year_index("<html></html>", "neurips2025").is_empty());
    }
}
