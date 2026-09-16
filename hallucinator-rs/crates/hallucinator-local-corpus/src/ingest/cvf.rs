//! Parse a CVF Open Access ("openaccess.thecvf.com") conference paper
//! listing into corpus records.
//!
//! One shared template across CVPR, ICCV, and WACV — confirmed by diffing
//! the fetched `?day=all` page for all three, same markup byte-for-byte
//! modulo conference name. Structure, inside one `<dl>`:
//!
//! ```html
//! <dt class="ptitle"><a href="/content/.../paper.html">Title</a></dt>
//! <dd>
//!   <form class="authsearch"><input name="query_author" value="Name"> ... </form>
//!   <form class="authsearch"><input name="query_author" value="Name"> ... </form>
//! </dd>
//! <dd> ... [pdf] [supp] [bibtex] ... </dd>
//! ```
//!
//! Each `dt.ptitle` is followed by exactly two `<dd>` siblings: the first
//! holds one `<form class="authsearch">` per author with the author's
//! name already in a `query_author` hidden-input `value` attribute — no
//! text-scraping or affiliation-stripping needed, the cleanest author
//! extraction of any venue in this corpus. The second `<dd>` (pdf/supp/
//! bibtex links) is skipped entirely; not walked past, so no need to
//! explicitly locate it.
//!
//! Fetch with `?day=all` (linked as "All Papers" from every conference's
//! menu page) to get the whole proceedings in one page rather than
//! per-day fragments.

use scraper::{ElementRef, Node, Selector};

use crate::db::NewPublication;

const CVF_BASE: &str = "https://openaccess.thecvf.com";

/// Parse a CVF `?day=all` paper-listing page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"cvpr2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = scraper::Html::parse_document(html);
    let title_sel = Selector::parse("dt.ptitle > a").unwrap();
    let author_input_sel = Selector::parse("input[name=\"query_author\"]").unwrap();

    let mut out = Vec::new();
    for title_el in document.select(&title_sel) {
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = title_el.value().attr("href").map(resolve_href);

        // The title `<a>`'s parent is `dt.ptitle`; the authors `<dd>` is
        // that `dt`'s next *element* sibling (skipping whitespace text
        // nodes in between).
        let Some(dt) = title_el.parent().and_then(ElementRef::wrap) else {
            continue;
        };
        let Some(authors_dd) = next_element_sibling(&dt) else {
            continue;
        };

        let authors: Vec<String> = authors_dd
            .select(&author_input_sel)
            .filter_map(|input| input.value().attr("value"))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            url,
            source: source_tag.to_string(),
        });
    }
    out
}

/// Resolve a paper's abstract-page `href` (`"/content/CVPR2026/html/..."`)
/// to an absolute `openaccess.thecvf.com` URL.
fn resolve_href(href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if let Some(stripped) = href.strip_prefix('/') {
        format!("{CVF_BASE}/{stripped}")
    } else {
        format!("{CVF_BASE}/{href}")
    }
}

/// Walk forward from `el` to the next sibling that's an element (skipping
/// text/whitespace nodes in between).
fn next_element_sibling<'a>(el: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    let mut node = el.next_sibling();
    while let Some(n) = node {
        if matches!(n.value(), Node::Element(_))
            && let Some(er) = ElementRef::wrap(n)
        {
            return Some(er);
        }
        node = n.next_sibling();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
        <div id="content">
        <h3>Papers</h3>
        <dl>
        <dd>
        <a href="/CVPR2026">Back</a>
        </dd>
<dt class="ptitle"><br><a href="/content/CVPR2026/html/Xiao_Generalizable_Structure-Aware_Keypoint_Correspondence_CVPR_2026_paper.html">Generalizable Structure-Aware Keypoint Correspondence for Category-Unified 3D Single Object Tracking</a></dt>
<dd>
<form id="form-1" action="/CVPR2026" method="post" class="authsearch">
<input type="hidden" name="query_author" value="Jie Xiao">
<a href="#" onclick="document.getElementById('form-1').submit();">Jie Xiao</a>,
</form>
<form id="form-2" action="/CVPR2026" method="post" class="authsearch">
<input type="hidden" name="query_author" value="Tianzhu Zhang">
<a href="#" onclick="document.getElementById('form-2').submit();">Tianzhu Zhang</a>
</form>
</dd>
<dd>
[<a href="/content/CVPR2026/papers/Xiao_..._paper.pdf">pdf</a>]
<div class="link2">[<a class="fakelink">bibtex</a>]
<div class="bibref pre-white-space">@InProceedings{Xiao_2026_CVPR, author = {Xiao, Jie}}</div>
</div>
</dd>
<dt class="ptitle"><br><a href="/content/CVPR2026/html/Yang_DirectFisheye_CVPR_2026_paper.html">DirectFisheye-GS: Enabling Native Fisheye Input in Gaussian Splatting</a></dt>
<dd>
<form id="form-3" action="/CVPR2026" method="post" class="authsearch">
<input type="hidden" name="query_author" value="Wei Yang">
<a href="#" onclick="document.getElementById('form-3').submit();">Wei Yang</a>
</form>
</dd>
<dd>[<a href="x.pdf">pdf</a>]</dd>
        </dl>
        </div>
    "##;

    #[test]
    fn test_parse_two_papers() {
        let out = parse_accepted_papers(SAMPLE, "cvpr2026");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Generalizable Structure-Aware Keypoint Correspondence for Category-Unified 3D Single Object Tracking"
        );
        assert_eq!(out[0].authors, vec!["Jie Xiao", "Tianzhu Zhang"]);
        assert_eq!(out[0].source, "cvpr2026");
        assert_eq!(
            out[0].url.as_deref(),
            Some(
                "https://openaccess.thecvf.com/content/CVPR2026/html/Xiao_Generalizable_Structure-Aware_Keypoint_Correspondence_CVPR_2026_paper.html"
            )
        );
    }

    #[test]
    fn test_second_paper_authors_dd_correctly_isolated_from_links_dd() {
        // Regression guard for the "which <dd> is the authors one" logic:
        // the second paper's authors <dd> must not accidentally pick up
        // the first paper's trailing links/bibtex <dd>, and must not
        // include anything from its own links <dd> either.
        let out = parse_accepted_papers(SAMPLE, "cvpr2026");
        assert_eq!(
            out[1].title,
            "DirectFisheye-GS: Enabling Native Fisheye Input in Gaussian Splatting"
        );
        assert_eq!(out[1].authors, vec!["Wei Yang"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "cvpr2026").is_empty());
    }

    #[test]
    fn test_resolve_href_absolute_left_untouched() {
        let html = r##"
            <dl>
            <dt class="ptitle"><a href="https://example.org/paper.html">A Title With Enough Words</a></dt>
            <dd><form class="authsearch"><input name="query_author" value="A. Author"></form></dd>
            </dl>
        "##;
        let out = parse_accepted_papers(html, "cvpr2026");
        assert_eq!(
            out[0].url.as_deref(),
            Some("https://example.org/paper.html")
        );
    }
}
