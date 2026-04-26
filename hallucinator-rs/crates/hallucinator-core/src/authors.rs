use once_cell::sync::Lazy;
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

use crate::matching::fold_special_letters;

/// Common surname prefixes (case-insensitive).
static SURNAME_PREFIXES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "van", "von", "de", "del", "della", "di", "da", "al", "el", "la", "le", "ben", "ibn",
        "mac", "mc", "o",
    ]
    .into_iter()
    .collect()
});

/// Name suffixes to strip.
static NAME_SUFFIXES: Lazy<HashSet<&'static str>> =
    Lazy::new(|| ["jr", "sr", "ii", "iii", "iv", "v"].into_iter().collect());

/// Known organizational author names. When a ref_author matches one of these,
/// we check if the org name appears anywhere in the found_authors list instead
/// of doing normal name matching.
static ORG_AUTHOR_NAMES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "openai",
        "meta",
        "google",
        "deepmind",
        "anthropic",
        "microsoft",
        "deepseek",
        "deepseekai",
        "alibaba",
        "baidu",
        "tencent",
        "nvidia",
        "apple",
        "darpa",
        "ftc",
        "nasa",
        "nist",
        "ieee",
        "acm",
        "who",
        "oecd",
        "unesco",
        "european commission",
        "mistralai",
        "mistral",
        // Government departments/agencies (returned by GovInfo)
        "commerce department",
        "department of commerce",
        "department of defense",
        "department of energy",
        "department of homeland security",
        "department of justice",
        "congress",
        "senate",
        "gao",
        "cisa",
        "fda",
        "epa",
        "fcc",
        "sec",
    ]
    .into_iter()
    .collect()
});

/// Validate that at least one author in `ref_authors` matches one in `found_authors`.
///
/// Uses three modes:
/// - **Organization mode**: If ref_author is a known org name (e.g., "OpenAI"),
///   check if the org name appears in any found_author string.
/// - **Last-name-only mode**: If most PDF-extracted authors lack first names/initials,
///   compare only surnames (with partial suffix matching for multi-word surnames).
/// - **Full mode**: Normalize to "FirstInitial surname" and check for set intersection.
pub fn validate_authors(ref_authors: &[String], found_authors: &[String]) -> bool {
    if ref_authors.is_empty() || found_authors.is_empty() {
        return false;
    }

    // Check for organizational authors: when either the reference or the database
    // lists an org name (e.g., "OpenAI", "Qwen Team", "DeepSeek-AI") instead of
    // individual authors, skip author validation — we can't meaningfully compare
    // org names against individual contributor names.
    for author_list in [ref_authors, found_authors] {
        for author in author_list {
            let lower = author.trim().to_lowercase();
            // Strip hyphens for matching (e.g., "DeepSeek-AI" → "deepseekai")
            let dehyphen = lower.replace('-', "");

            // Direct match against known org names
            if ORG_AUTHOR_NAMES.contains(lower.as_str())
                || ORG_AUTHOR_NAMES.contains(dehyphen.as_str())
            {
                return true;
            }

            // "X Team" pattern: "Qwen Team", "DeepSeek-AI Team"
            let words: Vec<&str> = lower.split_whitespace().collect();
            if words.last() == Some(&"team") && words.len() <= 3 {
                return true;
            }
        }
    }

    let ref_clean: Vec<&str> = ref_authors
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .collect();

    // Determine if ref authors are last-name-only
    let last_name_only_count = ref_clean
        .iter()
        .filter(|a| !has_first_name_or_initial(a))
        .count();
    let ref_are_last_name_only = last_name_only_count > ref_clean.len() / 2;

    if ref_are_last_name_only {
        let ref_surnames: Vec<String> = ref_authors
            .iter()
            .filter_map(|a| {
                let s = get_last_name(a);
                if s.is_empty() { None } else { Some(s) }
            })
            .collect();

        let found_surnames: Vec<String> = found_authors
            .iter()
            .filter_map(|a| {
                let s = get_last_name(a);
                if s.is_empty() { None } else { Some(s) }
            })
            .collect();

        for rn in &ref_surnames {
            for fn_ in &found_surnames {
                if rn == fn_ {
                    return true;
                }
                // Check if one surname ends with the other
                if fn_.ends_with(rn.as_str()) || rn.ends_with(fn_.as_str()) {
                    return true;
                }
            }
        }
        false
    } else {
        // Phantom-author guard — when the citation lists noticeably
        // more authors than the DB record, count the surnames in the
        // citation that don't appear in the DB record. A few unmatched
        // names are tolerable (typos, unusual transliterations), but a
        // citation that pads several unrelated names onto a real
        // paper's author list — common for AI-hallucinated
        // bibliographies that splice famous co-authors onto an existing
        // title — should be flagged as a mismatch even though the
        // genuine authors still overlap. Skipped when ref ≤ found (the
        // et-al-truncation case is handled below).
        //
        // We don't gate on `found.len() >= 3` like a more conservative
        // guard would: some DB records are themselves incomplete
        // (DBLP's StackGuard entry has just 1 author of the real 10,
        // and citations with 9 phantoms vs that single hit are common
        // padded-citation patterns). Trusting DB completeness here
        // would hand a "verified" stamp to citations whose author list
        // bears no resemblance to the indexed paper. Better to flag
        // and let the user mark safe than to silently approve.
        if ref_authors.len() > found_authors.len() {
            // Use the same `<initial>:<surname>` fingerprint as the
            // mark-safe identity key (`compute_fp_identity` in cache.rs)
            // — it's strictly stronger than `get_last_name` for the
            // phantom check. Two cases this fixes vs. the surname-only
            // form:
            //
            //  * Particle-prefixed surnames where one side carries the
            //    particle and the other doesn't ("Emiliano De Cristofaro"
            //    vs DBLP's "Cristofaro, E."). `get_last_name` produced
            //    "de cristofaro" vs "cristofaro" → phantom inflated.
            //  * Initial collisions where two authors share a surname
            //    but have different initials ("J. Smith" vs "A. Smith");
            //    surname-only would have called these the same person.
            //
            // The `et al.` token is stripped by author_fingerprint
            // returning None on an unparseable string, so we don't need
            // the explicit `last != "al"` guard the surname-only form
            // had.
            let found_fps: HashSet<String> = found_authors
                .iter()
                .filter_map(|a| crate::cache::author_fingerprint(a))
                .collect();
            let phantom_count = ref_authors
                .iter()
                .filter(|a| {
                    crate::cache::author_fingerprint(a).is_some_and(|fp| !found_fps.contains(&fp))
                })
                .count();
            // Trip the guard when there are 3+ phantom surnames AND
            // they make up >25% of the citation. The absolute floor
            // keeps the check quiet for short citations with one or
            // two unmatched names; the percentage floor keeps it from
            // firing on long-but-mostly-correct rosters.
            if phantom_count >= 3 && phantom_count * 4 > ref_authors.len() {
                return false;
            }
        }

        let ref_set: HashSet<String> = ref_authors.iter().map(|a| normalize_author(a)).collect();
        let found_set: HashSet<String> =
            found_authors.iter().map(|a| normalize_author(a)).collect();

        if !ref_set.is_disjoint(&found_set) {
            return true;
        }

        // Surname-based fallback: compare just last names when initial-based
        // comparison fails. This handles format mismatches like:
        // - "C. Gregory" (Initial + Surname) vs "Gregory Cohen" (FirstName + Surname)
        //   where the surname "Gregory" appears as a first name in the other format
        // - Accent differences already handled by strip_diacritics in get_last_name
        let ref_surnames: HashSet<String> = ref_authors
            .iter()
            .filter_map(|a| {
                let s = get_last_name(a);
                if s.is_empty() { None } else { Some(s) }
            })
            .collect();
        let found_surnames: HashSet<String> = found_authors
            .iter()
            .filter_map(|a| {
                let s = get_last_name(a);
                if s.is_empty() { None } else { Some(s) }
            })
            .collect();

        if !ref_surnames.is_disjoint(&found_surnames) {
            return true;
        }

        // Handle truncated author lists ("et al."):
        // If the reference has significantly fewer authors than the DB result,
        // check if ALL extracted authors appear in the found authors (subset match).
        // This handles cases where the PDF says "Gentry et al." (1 author) but
        // the DB returns all 5 authors including Gentry.
        // Threshold is 5 because ACM format typically shows up to 5 authors
        // before "et al.", and USENIX/IEEE show up to 3.
        if ref_authors.len() < found_authors.len()
            && ref_authors.len() <= 5
            && !ref_surnames.is_empty()
            && ref_surnames.is_subset(&found_surnames)
        {
            return true;
        }

        // Last-name-first (LNF) citation style: some papers cite as
        // "Surname Given, Surname Given, …" (no commas inside a name, no
        // initials). get_last_name picks the *last* token, which is actually
        // the given name in this order — so surname matching fails even
        // though the authors are correct (e.g. "Ekparinya Parinya" vs DBLP's
        // "Parinya Ekparinya").
        //
        // When both lists look like two-token proper-noun names without
        // initials (the common ambiguous shape), also consider the *first*
        // token as a candidate surname. We only take this branch as a
        // fallback after direct surname comparison has failed, so it can't
        // create false positives — any overlap surfaced here was invisible
        // to the stricter checks above.
        if ref_authors.iter().all(|a| is_ambiguous_two_token(a))
            && found_authors.iter().any(|a| is_ambiguous_two_token(a))
        {
            let ref_first_tokens: HashSet<String> = ref_authors
                .iter()
                .filter_map(|a| first_token_lower(a))
                .collect();
            if !ref_first_tokens.is_disjoint(&found_surnames) {
                return true;
            }
        }

        false
    }
}

/// A name is "ambiguous two-token" if it's exactly two whitespace-separated
/// tokens, both begin with an uppercase letter, and neither is an initial or
/// carries a period (i.e. we cannot tell Given-Family from Family-Given).
fn is_ambiguous_two_token(name: &str) -> bool {
    // split_whitespace already handles leading/trailing whitespace.
    let parts: Vec<&str> = name.split_whitespace().collect();
    if parts.len() != 2 {
        return false;
    }
    for p in &parts {
        if p.contains('.') || p.len() < 2 {
            return false;
        }
        let first = p.chars().next().unwrap();
        if !first.is_uppercase() {
            return false;
        }
    }
    true
}

/// Lowercase first whitespace-separated token (with trailing punctuation
/// trimmed). Used only by the LNF fallback above.
fn first_token_lower(name: &str) -> Option<String> {
    let first = name.split_whitespace().next()?;
    let first = first.trim_end_matches(|c: char| !c.is_alphanumeric());
    if first.is_empty() {
        None
    } else {
        Some(strip_diacritics(first).to_lowercase())
    }
}

/// Extract surname from name parts, handling multi-word surnames and suffixes.
fn get_surname_from_parts(parts: &[&str]) -> String {
    if parts.is_empty() {
        return String::new();
    }

    // Strip name suffixes
    let mut parts = parts.to_vec();
    while parts.len() >= 2
        && NAME_SUFFIXES.contains(parts.last().unwrap().to_lowercase().trim_end_matches('.'))
    {
        parts.pop();
    }

    if parts.is_empty() {
        return String::new();
    }

    // Check for three-part surnames like "De La Cruz"
    if parts.len() >= 3
        && SURNAME_PREFIXES.contains(parts[parts.len() - 3].to_lowercase().trim_end_matches('.'))
    {
        return parts[parts.len() - 3..].join(" ");
    }

    // Check for two-part surnames like "Van Bavel"
    if parts.len() >= 2
        && SURNAME_PREFIXES.contains(parts[parts.len() - 2].to_lowercase().trim_end_matches('.'))
    {
        return parts[parts.len() - 2..].join(" ");
    }

    parts.last().unwrap().to_string()
}

/// Strip diacritics and normalize typographic characters for comparison.
/// "Müller" → "Muller", "Crépeau" → "Crepeau", "Müßig" → "Mussig",
/// "Adıgüzel" → "Adiguzel", "O'Brien" → "O'Brien"
fn strip_diacritics(s: &str) -> String {
    // Normalize curly quotes/apostrophes to ASCII before NFKD
    // (NFKD doesn't decompose U+2019 RIGHT SINGLE QUOTATION MARK)
    let s = s
        .replace(['\u{2019}', '\u{2018}'], "'") // curly single quotes → apostrophe
        .replace(['\u{201C}', '\u{201D}'], "\""); // curly double quotes → straight
    // Fold ß/ı/ø/… to their DBLP/arXiv-style ASCII transliteration; NFKD alone
    // would leave these untouched and the ASCII filter would then drop them.
    let s = fold_special_letters(&s);
    s.nfkd().filter(|c| c.is_ascii()).collect()
}

/// Normalize an author name to "FirstInitial surname" format for comparison.
fn normalize_author(name: &str) -> String {
    // Strip diacritics first so comparisons are accent-insensitive
    let name = strip_diacritics(name.trim());
    let name = name.trim();

    // AAAI "Surname, Initials" format
    if name.contains(',') {
        let parts: Vec<&str> = name.splitn(2, ',').collect();
        let surname = parts[0].trim();
        let initials = if parts.len() > 1 { parts[1].trim() } else { "" };
        let first_initial = initials.chars().next().unwrap_or(' ');
        return format!("{} {}", first_initial, surname.to_lowercase());
    }

    let parts: Vec<&str> = name.split_whitespace().collect();
    if parts.is_empty() {
        return String::new();
    }

    // Springer "Surname Initial" format: last part is 1-2 uppercase letters
    if parts.len() >= 2 {
        let last = *parts.last().unwrap();
        if last.len() <= 2 && last.chars().all(|c| c.is_uppercase()) {
            let surname = parts[..parts.len() - 1].join(" ");
            let initial = last.chars().next().unwrap();
            return format!("{} {}", initial, surname.to_lowercase());
        }
    }

    // Standard: "FirstName LastName"
    let surname = get_surname_from_parts(&parts);
    let first_initial = parts[0].chars().next().unwrap_or(' ');
    format!("{} {}", first_initial, surname.to_lowercase())
}

/// Get the last name from an author name string (public API for orchestrator).
pub fn get_last_name_public(name: &str) -> String {
    get_last_name(name)
}

/// Get the last name from an author name string.
fn get_last_name(name: &str) -> String {
    // Strip diacritics for accent-insensitive comparison
    let name = strip_diacritics(name.trim());
    let name = name.trim();

    // AAAI "Surname, Initials" format
    if name.contains(',') {
        return name.split(',').next().unwrap().trim().to_lowercase();
    }

    let parts: Vec<&str> = name.split_whitespace().collect();
    if parts.is_empty() {
        return String::new();
    }

    get_surname_from_parts(&parts).to_lowercase()
}

/// Check if a name contains a first name or initial (not just a surname).
fn has_first_name_or_initial(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }

    // "Surname, Initial" format
    if name.contains(',') {
        let parts: Vec<&str> = name.splitn(2, ',').collect();
        return parts.len() > 1 && !parts[1].trim().is_empty();
    }

    let parts: Vec<&str> = name.split_whitespace().collect();
    // Strip name suffixes
    let core_parts: Vec<&str> = parts
        .iter()
        .filter(|p| !NAME_SUFFIXES.contains(p.to_lowercase().trim_end_matches('.')))
        .copied()
        .collect();

    if core_parts.len() <= 1 {
        return false;
    }

    // Check for initials in non-last positions
    for part in &core_parts[..core_parts.len() - 1] {
        if part.trim_end_matches('.').len() == 1 {
            return true;
        }
    }

    // Check Springer "Surname Initial" format (last part is 1-2 uppercase)
    let last = *core_parts.last().unwrap();
    if last.len() <= 2 && last.chars().all(|c| c.is_uppercase()) {
        return true;
    }

    // Check if first part is a first name
    let first = core_parts[0].trim_end_matches('.');
    if first.len() >= 2
        && first.chars().next().is_some_and(|c| c.is_uppercase())
        && !SURNAME_PREFIXES.contains(first.to_lowercase().as_str())
        && core_parts.len() >= 2
    {
        let second = core_parts[1].trim_end_matches('.');
        if second.len() >= 2 && second.chars().next().is_some_and(|c| c.is_uppercase()) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_validate_authors_basic() {
        assert!(validate_authors(
            &s(&["John Smith", "Alice Jones"]),
            &s(&["John Smith", "Bob Brown"]),
        ));
    }

    #[test]
    fn test_validate_authors_no_overlap() {
        assert!(!validate_authors(&s(&["John Smith"]), &s(&["Bob Brown"]),));
    }

    #[test]
    fn test_validate_authors_last_name_only() {
        // Last-name-only mode
        assert!(validate_authors(
            &s(&["Smith", "Jones"]),
            &s(&["John Smith", "Alice Jones"]),
        ));
    }

    #[test]
    fn test_validate_authors_multi_word_surname() {
        assert!(validate_authors(
            &s(&["Jay Van Bavel"]),
            &s(&["J. J. Van Bavel"]),
        ));
    }

    #[test]
    fn test_validate_authors_aaai_format() {
        assert!(validate_authors(
            &s(&["Bail, C. A.", "Jones, M."]),
            &s(&["Christopher Bail", "Michael Jones"]),
        ));
    }

    #[test]
    fn test_normalize_author_springer() {
        assert_eq!(normalize_author("Abrahao S"), "S abrahao");
    }

    #[test]
    fn test_normalize_author_standard() {
        assert_eq!(normalize_author("John Smith"), "J smith");
    }

    #[test]
    fn test_normalize_author_aaai() {
        assert_eq!(normalize_author("Bail, C. A."), "C bail");
    }

    #[test]
    fn test_get_last_name_multi_word() {
        assert_eq!(get_last_name("Jay Van Bavel"), "van bavel");
    }

    #[test]
    fn test_empty() {
        assert!(!validate_authors(&[], &s(&["Smith"])));
        assert!(!validate_authors(&s(&["Smith"]), &[]));
    }

    #[test]
    fn test_org_author_openai() {
        // "OpenAI" as ref author should match found authors from the OpenAI org
        assert!(validate_authors(
            &s(&["OpenAI"]),
            &s(&["Josh Achiam", "Steven Adler"]),
        ));
    }

    #[test]
    fn test_org_author_team() {
        // "Qwen Team" as org author — skip validation, accept any found authors
        assert!(validate_authors(
            &s(&["Qwen Team"]),
            &s(&["An Yang", "Baosong Yang"]),
        ));
    }

    #[test]
    fn test_org_author_meta() {
        // "Meta" is a known org — skip validation
        assert!(validate_authors(
            &s(&["Meta"]),
            &s(&["Hugo Touvron", "Thibaut Lavril"]),
        ));
    }

    #[test]
    fn test_org_author_deepseek() {
        // "DeepSeek-AI" with hyphen should also match
        assert!(validate_authors(&s(&["DeepSeek-AI"]), &s(&["Some Author"]),));
    }

    #[test]
    fn test_org_author_found_side() {
        // DBLP returns "DeepSeek-AI" as org, but PDF has individual authors
        assert!(validate_authors(
            &s(&["Daya Guo", "Dejian Yang", "Haowei Zhang"]),
            &s(&["DeepSeek-AI"]),
        ));
    }

    #[test]
    fn test_accent_insensitive_muller() {
        // "Müller" from PDF should match "Muller" from DB
        assert!(validate_authors(
            &s(&["Nicolas M. Müller"]),
            &s(&["Nicolas M. Muller"]),
        ));
    }

    #[test]
    fn test_accent_insensitive_crepeau() {
        // "Crépeau" should match "Crepeau"
        assert!(validate_authors(
            &s(&["C. Crépeau", "D. Gottesman"]),
            &s(&["Claude Crepeau", "Daniel Gottesman"]),
        ));
    }

    #[test]
    fn test_accent_insensitive_doupe() {
        // "Doupé" should match "Doupe"
        assert!(validate_authors(
            &s(&["Huahong Tu", "Adam Doupé"]),
            &s(&["Huahong Tu", "Adam Doupe"]),
        ));
    }

    #[test]
    fn test_accent_insensitive_tramer() {
        // "Tramèr" should match "Tramer"
        assert!(validate_authors(
            &s(&["Florian Tramèr"]),
            &s(&["Florian Tramer"]),
        ));
    }

    #[test]
    fn test_sharp_s_folds_to_ss() {
        // PDF: "Müßig", DBLP/arXiv: "Mussig" (ß → ss).
        // NFKD alone would drop ß and produce "Mig", which never matched.
        assert!(validate_authors(&s(&["Hans Müßig"]), &s(&["Hans Mussig"]),));
    }

    #[test]
    fn test_dotless_i_folds_to_i() {
        // PDF: "Adıgüzel" (Turkish dotless i), DBLP: "Adiguzel".
        // NFKD alone would drop ı and produce "Adgzel", which never matched.
        assert!(validate_authors(
            &s(&["Cemal Adıgüzel"]),
            &s(&["Cemal Adiguzel"]),
        ));
    }

    #[test]
    fn test_slashed_o_folds_to_o() {
        // PDF: "Bjørn Østergaard", DBLP: "Bjorn Ostergaard".
        assert!(validate_authors(
            &s(&["Bjørn Østergaard"]),
            &s(&["Bjorn Ostergaard"]),
        ));
    }

    #[test]
    fn test_l_with_stroke_folds_to_l() {
        // PDF: "Wojciech Łukasz", DBLP: "Wojciech Lukasz".
        assert!(validate_authors(
            &s(&["Wojciech Łukasz"]),
            &s(&["Wojciech Lukasz"]),
        ));
    }

    #[test]
    fn test_accent_insensitive_last_name_only() {
        // Last-name-only mode with accents
        assert!(validate_authors(
            &s(&["Müller", "Köbis"]),
            &s(&["Nicolas Muller", "Nils Kobis"]),
        ));
    }

    #[test]
    fn test_curly_quote_obrien() {
        // PDF uses curly quote U+2019, DB uses straight apostrophe
        assert!(validate_authors(
            &s(&["Sean O\u{2019}Brien"]),
            &s(&["Sean O'Brien"]),
        ));
    }

    #[test]
    fn test_et_al_subset_single_author() {
        // PDF says "Gentry" (et al. truncated), DB has "Boneh, Gentry"
        assert!(validate_authors(
            &s(&["Craig Gentry"]),
            &s(&["Dan Boneh", "Craig Gentry"]),
        ));
    }

    #[test]
    fn test_phantom_authors_padded_citation_rejected() {
        // Real USENIX 2022 paper "Are Your Sensitive Attributes Private?"
        // has 5 authors. A hallucinated citation pads in 5 famous security
        // researchers as fake co-authors — the genuine 5 still overlap, so
        // the standard intersection would pass. The phantom-author guard
        // must catch this and flag it as a mismatch.
        assert!(!validate_authors(
            &s(&[
                "Shagufta Mehnaz",
                "Sayanton V Dibbo",
                "Roberta De Viti",
                "Ehsanul Kabir",
                "Björn B Brandenburg",
                "Stefan Mangard",
                "Ninghui Li",
                "Elisa Bertino",
                "Michael Backes",
                "Emiliano De Cristofaro",
            ]),
            &s(&[
                "Shagufta Mehnaz",
                "Sayanton V. Dibbo",
                "Ehsanul Kabir",
                "Ninghui Li",
                "Elisa Bertino",
            ]),
        ));
    }

    #[test]
    fn test_phantom_authors_one_extra_still_passes() {
        // A single unmatched name (e.g., a typo'd or unusual
        // transliteration) shouldn't trip the guard — only sustained
        // padding should.
        assert!(validate_authors(
            &s(&[
                "Alice Author",
                "Bob Author",
                "Carol Author",
                "Dave Author",
                "Eve Author",
                "Frank Typo",
            ]),
            &s(&[
                "Alice Author",
                "Bob Author",
                "Carol Author",
                "Dave Author",
                "Eve Author",
            ]),
        ));
    }

    #[test]
    fn test_phantom_authors_fires_even_with_small_found() {
        // Real failure: DBLP's StackGuard (USENIX 1998) entry has only
        // one author indexed (Crispan Cowan) although the actual paper
        // has 10. A padded citation (10 ref authors, 1 DBLP author, 9
        // surnames not matching the lone DB author) used to pass the
        // standard intersection because the phantom guard was gated on
        // `found.len() >= 3` — DB-completeness is too generous a
        // benefit-of-the-doubt when the citation/DB skew is this
        // extreme. The guard now fires regardless of DB size, so
        // citations that don't match the indexed author list (whatever
        // its size) are flagged as mismatches.
        assert!(!validate_authors(
            &s(&[
                "Crispan Cowan",
                "Calton Pu",
                "Dave Maier",
                "Heather Hintony",
                "Jonathan Walpole",
                "Peat Bakke",
                "Steve Beattie",
                "Aaron Grier",
                "Perry Wagle",
                "Qian Zhang",
            ]),
            &s(&["Crispan Cowan"]),
        ));
    }

    #[test]
    fn test_phantom_authors_use_fingerprint_normalization() {
        // The phantom guard now keys on `<initial>:<surname>`
        // fingerprints (the same form `compute_fp_identity` uses) so
        // particle-prefixed surnames don't inflate the phantom count
        // when one side carries the particle and the other doesn't:
        //   ref:  "Emiliano De Cristofaro"  →  e:cristofaro
        //   db:   "Cristofaro, E."          →  e:cristofaro
        // Surname-only would have produced "de cristofaro" vs
        // "cristofaro" — different — and counted the matched author
        // as a phantom. With fingerprints they collide, so a citation
        // whose authors all match the DB record (even via different
        // surname formats) doesn't trip the guard.
        assert!(validate_authors(
            &s(&[
                "Alice Author",
                "Bob Author",
                "Carol Author",
                "Emiliano De Cristofaro",
            ]),
            &s(&[
                "Alice Author",
                "Bob Author",
                "Carol Author",
                "Cristofaro, E.",
            ]),
        ));
    }

    #[test]
    fn test_et_al_subset_two_authors() {
        // PDF says "Dwork, Roth" (et al. truncated), DB has "Dwork, Roth, Others"
        assert!(validate_authors(
            &s(&["Cynthia Dwork", "Aaron Roth"]),
            &s(&["Cynthia Dwork", "Aaron Roth", "Guy Rothblum"]),
        ));
    }

    #[test]
    fn test_et_al_no_false_positive() {
        // Subset match should NOT match when ref authors are NOT in found authors
        assert!(!validate_authors(
            &s(&["John Smith"]),
            &s(&["Alice Jones", "Bob Brown"]),
        ));
    }

    #[test]
    fn test_et_al_many_ref_authors_no_subset() {
        // When ref has many authors (>3), subset match is disabled,
        // so completely different authors should NOT match
        assert!(!validate_authors(
            &s(&["X. Alpha", "Y. Beta", "Z. Gamma", "W. Delta"]),
            &s(&[
                "A. One", "B. Two", "C. Three", "D. Four", "E. Five", "F. Six"
            ]),
        ));
    }

    // ─── Fix D: last-name-first citation style ───

    #[test]
    fn test_lnf_style_matches_dblp_given_family() {
        // USENIX 2025 case: "The attack of the clones against proof-of-authority"
        // cited as "Ekparinya Parinya, Gramoli Vincent, and Jourjon Guillaume"
        // — family-first — while DBLP has standard "Parinya Ekparinya",
        // "Vincent Gramoli", "Guillaume Jourjon".
        assert!(validate_authors(
            &s(&["Ekparinya Parinya", "Gramoli Vincent", "Jourjon Guillaume"]),
            &s(&["Parinya Ekparinya", "Vincent Gramoli", "Guillaume Jourjon"]),
        ));
    }

    #[test]
    fn test_lnf_style_still_rejects_wrong_paper() {
        // LNF fallback must not turn unrelated authors into a match.
        // "Alice Bob, Charlie Dave" (two-token, ambiguous) against unrelated
        // surnames — no first-token or last-token overlap either way.
        assert!(!validate_authors(
            &s(&["Alice Bob", "Charlie Dave"]),
            &s(&["Eve Foster", "Frank Greene"]),
        ));
    }

    #[test]
    fn test_lnf_fallback_skipped_when_names_have_initials() {
        // When names carry initials, the ordering is unambiguous — the
        // LNF fallback should not fire. This guards against enabling the
        // fallback for normal IEEE-style refs and accidentally matching
        // "J. Smith" to DBLP's "Smith John" just because both contain "smith".
        assert!(!validate_authors(
            &s(&["J. Smith"]),
            &s(&["Alice Kumar", "Robert Chen"]),
        ));
    }

    #[test]
    fn test_is_ambiguous_two_token() {
        assert!(is_ambiguous_two_token("Ekparinya Parinya"));
        assert!(is_ambiguous_two_token("Gramoli Vincent"));
        // Three tokens — not ambiguous two-token
        assert!(!is_ambiguous_two_token("John M. Smith"));
        // Initial — not ambiguous
        assert!(!is_ambiguous_two_token("J. Smith"));
        assert!(!is_ambiguous_two_token("Smith J."));
        // One token
        assert!(!is_ambiguous_two_token("Madonna"));
        // Lowercase — not a proper noun
        assert!(!is_ambiguous_two_token("john smith"));
    }
}
