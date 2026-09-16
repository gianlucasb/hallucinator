//! Shared DOM text-extraction helper, used by venues whose title/authors
//! aren't both inside their own dedicated child elements — PETS (title is
//! direct text before an optional artifact-badge `<a>` and an author
//! `<span>`) and IMC (title is inside a `<strong>`, authors are the
//! direct text that follows it).

use scraper::ElementRef;
use std::ops::Deref;

/// Collect only an element's own direct text-node children (not text from
/// nested elements), concatenated and trimmed.
pub(crate) fn direct_text(el: &ElementRef) -> String {
    el.children()
        .filter_map(|child| child.value().as_text().map(|t| t.deref()))
        .collect::<String>()
        .trim()
        .to_string()
}
