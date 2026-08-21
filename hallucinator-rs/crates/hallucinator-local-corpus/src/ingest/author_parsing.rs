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

/// Parse `"Name (Affiliation), Name and Name (Affiliation); ..."` into
/// just the names, dropping every affiliation.
///
/// Matches each `name-group (affiliation)` unit directly — a non-greedy
/// run of non-paren characters followed by a parenthesized group — rather
/// than splitting on a separator first, since names/affiliations never
/// contain `(`/`)` but affiliations often contain the same punctuation
/// (commas, semicolons) used to separate entries.
pub fn parse_paren_grouped_names(text: &str) -> Vec<String> {
    static PAIR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([^()]+?)\(([^()]*)\)").unwrap());

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
