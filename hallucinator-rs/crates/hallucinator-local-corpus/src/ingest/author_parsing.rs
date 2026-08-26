//! Shared `"Name (Affiliation)"` author-list parsing, reused across NDSS,
//! IEEE S&P, and most of ACM CCS's historical page formats.
//!
//! The pattern varies in two ways across venues/years without changing its
//! essential shape:
//! - **Separator between entries**: comma, semicolon, or `<br>` (already
//!   collapsed to plain whitespace/newlines by the time this runs) — all
//!   handled the same way, since the regex scans for `(...)` boundaries
//!   directly rather than splitting on a fixed separator first.
//! - **Names per affiliation group**: usually one (`"Name (Aff)"`), but
//!   some formats share one affiliation across several authors
//!   (`"Name and Name (Aff)"`, `"Name, Name, and Name (Aff)"` — Oxford
//!   comma). Each group is split on `" and "`/`,` after extraction.

use once_cell::sync::Lazy;
use regex::Regex;

/// Matches a `name-group (affiliation)` unit directly — a non-greedy run of
/// non-paren characters followed by a parenthesized group — rather than
/// splitting on a separator first, since names/affiliations never contain
/// `(`/`)` but affiliations often contain the same punctuation (commas,
/// semicolons) used to separate entries. Shared between
/// [`parse_paren_grouped_names`] and the heuristic in
/// [`parse_names_maybe_with_affiliations`] that decides whether a list's
/// parens are affiliations at all.
static PAIR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([^()]+?)\(([^()]*)\)").unwrap());

/// Parse `"Name (Affiliation), Name and Name (Affiliation); ..."` into
/// just the names, dropping every affiliation.
pub fn parse_paren_grouped_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for cap in PAIR_RE.captures_iter(text) {
        let group = strip_leading_separator(&cap[1]).replace(" and ", ", ");
        for name in group.split(',') {
            let name = name.trim();
            if !name.is_empty() && !name.eq_ignore_ascii_case("and") {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Parse an author list that may or may not carry parenthesized
/// affiliations: `"Name (Affiliation), Name (Affiliation)"` delegates to
/// [`parse_paren_grouped_names`]; plain `"Name, Name, Name"` with no
/// affiliations at all (e.g. ISCA's 2023-2025 program pages, unlike its
/// 2026 page or ASPLOS's, which always include them) is split directly on
/// `,`/`;`.
///
/// The two shapes can't be told apart just by "does the text contain a
/// `(`" — ISCA's plain lists occasionally wrap an author's nickname in
/// parens mid-name (`"Boyang (Tony) Yu"`), which [`parse_paren_grouped_names`]
/// would otherwise misparse as an affiliation and mangle. Distinguish them
/// by what follows each `)`: a real affiliation is always followed by a
/// separator (`,`, `;`, `"and "`) or the end of the string, e.g.
/// `"Name (Aff), Name (Aff)"` or `"Name (Aff) and Name (Aff)"`. A nickname
/// is followed directly by another bare word — the rest of the same
/// name — which no genuine affiliation list does.
pub fn parse_names_maybe_with_affiliations(text: &str) -> Vec<String> {
    if parens_look_like_affiliations(text) {
        return parse_paren_grouped_names(text);
    }
    text.split([',', ';'])
        .map(|s| strip_leading_separator(s))
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("and"))
        .collect()
}

/// Whether every parenthesized group in `text` is followed by a
/// list-separator (or the end of the string) rather than a bare word —
/// see [`parse_names_maybe_with_affiliations`]. `false` if there are no
/// parens at all, so the caller still takes the plain-split path.
fn parens_look_like_affiliations(text: &str) -> bool {
    let mut found_any = false;
    for m in PAIR_RE.find_iter(text) {
        found_any = true;
        let remainder = text[m.end()..].trim_start();
        let is_separator = remainder.is_empty()
            || remainder.starts_with(',')
            || remainder.starts_with(';')
            || remainder
                .get(..4)
                .is_some_and(|s| s.eq_ignore_ascii_case("and "))
            || remainder.eq_ignore_ascii_case("and");
        if !is_separator {
            return false;
        }
    }
    found_any
}

/// Trim a captured group's leading punctuation/conjunction left over from
/// the previous group's boundary: `", "`, `"; "`, or a bare `"and "` —
/// the top-level separator before groups whose only author shares the
/// *previous* affiliation, e.g. `"...(Aff) and Solo Author (Aff2)"`.
fn strip_leading_separator(s: &str) -> String {
    let trimmed = s.trim().trim_start_matches([',', ';']).trim_start();
    // `starts_with`/`strip_prefix` are UTF-8-boundary-safe (unlike a fixed
    // byte-range slice, which panics if a multi-byte character in the
    // name — e.g. an accented name right after "and " — happens to
    // straddle the cut point).
    match strip_prefix_case_insensitive(trimmed, "and ") {
        Some(rest) => rest.trim().to_string(),
        None => trimmed.trim().to_string(),
    }
}

/// `str::strip_prefix`, but matching `prefix` case-insensitively.
/// `prefix` must be ASCII (the only case this module needs).
fn strip_prefix_case_insensitive<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let boundary = prefix.len();
    let candidate = s.get(..boundary)?;
    candidate
        .eq_ignore_ascii_case(prefix)
        .then(|| &s[boundary..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_name_per_group_comma_separated() {
        // NDSS-style.
        let names = parse_paren_grouped_names(
            "Licheng Pan (Zhejiang University), Yunsheng Lu (University of Chicago)",
        );
        assert_eq!(names, vec!["Licheng Pan", "Yunsheng Lu"]);
    }

    #[test]
    fn test_single_name_per_group_semicolon_separated() {
        // CCS-2022-style; note the affiliation itself contains a semicolon.
        let names = parse_paren_grouped_names(
            "Karel Dhondt (imec-DistriNet; KU Leuven); Victor Le Pochat (imec-DistriNet; KU Leuven)",
        );
        assert_eq!(names, vec!["Karel Dhondt", "Victor Le Pochat"]);
    }

    #[test]
    fn test_multi_name_per_group_with_and_and_oxford_comma() {
        // IEEE S&P 2016-2019-style.
        let names = parse_paren_grouped_names(
            "Lucca Hirschi and David Baelde (LSV, ENS Cachan) and Stéphanie Delaune (LSV, ENS Cachan & CNRS)",
        );
        assert_eq!(
            names,
            vec!["Lucca Hirschi", "David Baelde", "Stéphanie Delaune"]
        );
    }

    #[test]
    fn test_oxford_comma_group() {
        let names =
            parse_paren_grouped_names("Jiaxing He, Changzheng Wei, and Ying Yan (Ant Group)");
        assert_eq!(names, vec!["Jiaxing He", "Changzheng Wei", "Ying Yan"]);
    }

    #[test]
    fn test_affiliation_with_internal_comma_and_ampersand() {
        let names = parse_paren_grouped_names("Solo Author (LSV, ENS Cachan & CNRS)");
        assert_eq!(names, vec!["Solo Author"]);
    }

    #[test]
    fn test_empty_input() {
        assert!(parse_paren_grouped_names("").is_empty());
    }

    #[test]
    fn test_maybe_with_affiliations_falls_back_to_plain_comma_list() {
        // ISCA 2023-2025-style: no parenthesized affiliations at all.
        let names = parse_names_maybe_with_affiliations(
            "Cong Guo, Jiaming Tang, Weiming Hu, Jingwen Leng, Yuhao Zhu",
        );
        assert_eq!(
            names,
            vec![
                "Cong Guo",
                "Jiaming Tang",
                "Weiming Hu",
                "Jingwen Leng",
                "Yuhao Zhu"
            ]
        );
    }

    #[test]
    fn test_maybe_with_affiliations_not_fooled_by_embedded_nickname() {
        // ISCA 2023/2025-style: a mid-name nickname in parens, not an
        // affiliation — must not be misparsed as one.
        let names = parse_names_maybe_with_affiliations(
            "Sixu Li, Chaojian Li, Wenbo Zhu, Boyang (Tony) Yu, Yingyan (Celine) Lin",
        );
        assert_eq!(
            names,
            vec![
                "Sixu Li",
                "Chaojian Li",
                "Wenbo Zhu",
                "Boyang (Tony) Yu",
                "Yingyan (Celine) Lin"
            ]
        );
    }

    #[test]
    fn test_maybe_with_affiliations_delegates_when_parens_present() {
        let names = parse_names_maybe_with_affiliations(
            "Weihao Cui (Shanghai Jiao Tong University), Yukang Chen (Shanghai Jiao Tong University)",
        );
        assert_eq!(names, vec!["Weihao Cui", "Yukang Chen"]);
    }

    #[test]
    fn test_accented_name_right_after_and_does_not_panic() {
        // Regression: a fixed byte-offset slice used to panic here because
        // "É"/"ã"/etc. are multi-byte UTF-8 and can straddle a fixed cut
        // point — this must neither panic nor mis-split the name.
        let names = parse_paren_grouped_names("Foo Bar (Aff) and Émile Durkheim (Aff2)");
        assert_eq!(names, vec!["Foo Bar", "Émile Durkheim"]);

        let names2 = parse_paren_grouped_names("Foo Bar (Aff) and ãlvaro Silva (Aff2)");
        assert_eq!(names2, vec!["Foo Bar", "ãlvaro Silva"]);
    }
}
