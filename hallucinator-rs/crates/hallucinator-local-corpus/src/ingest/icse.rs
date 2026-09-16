//! Parse an ICSE track page's "Accepted Papers" tab into corpus records.
//!
//! ICSE (and most other researchr.org-hosted software engineering
//! conferences — the platform is shared across many SE venues, not just
//! ICSE) publishes no separate BibTeX/CSV/JSON export, but each track's
//! page (`conf.researchr.org/track/icse-YYYY/icse-YYYY-{track}`) embeds
//! a clean `#event-overview` tab ("Accepted Papers") alongside the messy
//! full mixed-track schedule (`#program`) in the same static HTML
//! document — titles, full author lists (as profile links, easy to pull
//! clean names from), and a DOI/pre-print link where available. No
//! author affiliations on this tab (they only exist on `#program`, mixed
//! in with every other track's sessions) — same tradeoff already made
//! for NeurIPS, which also has no affiliations.
//!
//! Each track (Research Track, SEIP, NIER, Demonstrations, Posters, ...)
//! is its own separate page with its own separate Accepted Papers tab —
//! there's no single page listing every track, so importing a full
//! edition means one fetch per track (`--source-tag` distinguishes them,
//! e.g. `"icse2026-research"`, `"icse2026-seip"`).
//!
//! Verified against `dblp.org`/CrossRef as of 2026-08-21: ICSE 2026's
//! main Research Track isn't in DBLP at all yet, and a spot-checked DOI
//! from this page doesn't resolve via CrossRef either — a real, current
//! gap, months post-conference, larger than any of the other venues
//! checked so far.

use scraper::{Html, Selector};

use crate::db::NewPublication;

/// The text of just `el`'s direct text-node children, skipping any
/// nested elements entirely.
///
/// Unlike `.text()` (which walks every descendant text node and
/// concatenates them with no separator), this excludes badge `<span>`s
/// researchr sometimes nests inside the title link — e.g. an "Add to
/// program" star icon (no text of its own, harmless either way) or a
/// "Distinguished Paper Award" badge (real text, and without this guard
/// it glues onto the title's own text with no space: `"...Satisfying
/// ModelsDistinguished Paper Award"`).
fn direct_text(el: &scraper::ElementRef) -> String {
    el.children()
        .filter_map(|node| node.value().as_text())
        .map(|t| &**t)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Parse one ICSE (or other researchr.org) track page's Accepted Papers
/// tab into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"icse2026-research"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    // Scoped to the #event-overview tab specifically — the same document
    // also contains the #program tab's mixed-track schedule table, which
    // must not be picked up here.
    let row_sel = Selector::parse("#event-overview tr").unwrap();
    let title_sel = Selector::parse("a[data-event-modal]").unwrap();
    let authors_sel = Selector::parse("div.performers a.navigate").unwrap();
    let link_sel = Selector::parse("a.publication-link").unwrap();

    let mut out = Vec::new();
    for row in document.select(&row_sel) {
        let Some(title_el) = row.select(&title_sel).next() else {
            continue;
        };
        let title = direct_text(&title_el);
        if title.is_empty() {
            continue;
        }

        let authors: Vec<String> = row
            .select(&authors_sel)
            .map(|a| a.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Prefer a DOI link over a pre-print link when both exist.
        let links: Vec<&str> = row
            .select(&link_sel)
            .filter_map(|a| a.value().attr("href"))
            .collect();
        let url = links
            .iter()
            .find(|href| href.contains("doi.org"))
            .or_else(|| links.first())
            .map(|s| s.to_string());

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
        <div class="tab-content">
          <div id="program" class="tab-pane">
            <table><tr><td><a data-event-modal="wrong">This Is From The Wrong Tab, Must Not Appear</a>
              <div class="performers"><a class="navigate">Nobody</a></div></td></tr></table>
          </div>
          <div id="event-overview" class="tab-pane">
            <h3>Accepted Papers</h3>
            <table class="table table-condensed">
              <thead><tr><th></th><th>Title</th></tr></thead>
              <tr>
                <td><span data-event-star="a"></span></td>
                <td>
                  <a href="#" data-event-modal="a">3D Software Synthesis Driven by Constraint-Expressive Intermediate Representation<span class="pull-right"><span class="output-badge"><img alt="Virtual Attendance" src="x.png"/></span></span></a>
                  <div class="prog-track">Research Track</div>
                  <div class="performers"><a href="https://conf.researchr.org/profile/icse-2026/shuqing" class="navigate">Shuqing Li</a>, <a href="https://conf.researchr.org/profile/icse-2026/ansonylam" class="navigate">Anson Y. Lam</a></div>
                  <a href="https://arxiv.org/abs/2507.18625" target="_blank" class="publication-link navigate"><span class="glyphicon glyphicon-link"></span> Pre-print</a>
                </td>
              </tr>
              <tr>
                <td><span data-event-star="b"></span></td>
                <td>
                  <a href="#" data-event-modal="b">A Causal Perspective on Measuring, Explaining and Mitigating Smells in LLM-Generated Code</a>
                  <div class="prog-track">Research Track</div>
                  <div class="performers"><a href="https://conf.researchr.org/profile/icse-2026/alejandrovelasco" class="navigate">Alejandro Velasco</a>, <a href="https://conf.researchr.org/profile/icse-2026/denysposhyvanyk1" class="navigate">Denys Poshyvanyk</a></div>
                  <a href="https://doi.org/10.1145/3744916.3773164" target="_blank" class="publication-link navigate"><span class="glyphicon glyphicon-link"></span> DOI</a>
                  <a href="https://arxiv.org/abs/2511.15817" target="_blank" class="publication-link navigate"><span class="glyphicon glyphicon-link"></span> Pre-print</a>
                </td>
              </tr>
              <tr>
                <td><span data-event-star="c"></span></td>
                <td>
                  <a href="#" data-event-modal="c">Accelerating IC3 Verification by Exploiting Unsatisfiable Cores and Satisfying Models<span class="pull-right"><span title="This paper won an award as a distinguished paper" data-facet-badge="Distinguished Paper Award" class="output-badge"><span class="label-primary label">Distinguished Paper Award</span></span></span></a>
                  <div class="prog-track">Research Track</div>
                  <div class="performers"><a class="navigate">Xinyi Gong</a></div>
                </td>
              </tr>
            </table>
          </div>
        </div>
    "##;

    #[test]
    fn test_parse_papers_ignores_program_tab() {
        let out = parse_accepted_papers(SAMPLE, "icse2026-research");
        assert_eq!(out.len(), 3);
        assert!(!out.iter().any(|r| r.title.contains("Wrong Tab")));
    }

    #[test]
    fn test_title_excludes_award_badge_text() {
        // Regression: a "Distinguished Paper Award" badge span has real
        // text content (unlike the attendance-mode badge, which is just
        // an <img>) — without excluding nested-span text entirely, this
        // used to glue onto the title with no separator:
        // "...Satisfying ModelsDistinguished Paper Award".
        let out = parse_accepted_papers(SAMPLE, "icse2026-research");
        assert_eq!(
            out[2].title,
            "Accelerating IC3 Verification by Exploiting Unsatisfiable Cores and Satisfying Models"
        );
        assert!(!out[2].title.contains("Distinguished"));
    }

    #[test]
    fn test_title_excludes_attendance_badge() {
        let out = parse_accepted_papers(SAMPLE, "icse2026-research");
        assert_eq!(
            out[0].title,
            "3D Software Synthesis Driven by Constraint-Expressive Intermediate Representation"
        );
        assert_eq!(out[0].authors, vec!["Shuqing Li", "Anson Y. Lam"]);
        assert_eq!(
            out[0].url.as_deref(),
            Some("https://arxiv.org/abs/2507.18625")
        );
    }

    #[test]
    fn test_prefers_doi_link_over_preprint() {
        let out = parse_accepted_papers(SAMPLE, "icse2026-research");
        assert_eq!(
            out[1].url.as_deref(),
            Some("https://doi.org/10.1145/3744916.3773164")
        );
        assert_eq!(out[1].source, "icse2026-research");
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "icse2026-research").is_empty());
    }
}
