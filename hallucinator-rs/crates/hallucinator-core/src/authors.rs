use once_cell::sync::Lazy;
use std::collections::HashSet;

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

/// Validate that at least one author in `ref_authors` matches one in `found_authors`.
///
/// Uses two modes:
/// - **Last-name-only mode**: If most PDF-extracted authors lack first names/initials,
///   compare only surnames (with partial suffix matching for multi-word surnames).
/// - **Full mode**: Normalize to "FirstInitial surname" and check for set intersection.
pub fn validate_authors(ref_authors: &[String], found_authors: &[String]) -> bool {
    if ref_authors.is_empty() || found_authors.is_empty() {
        return false;
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
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            })
            .collect();

        let found_surnames: Vec<String> = found_authors
            .iter()
            .filter_map(|a| {
                let s = get_last_name(a);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
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
        let ref_set: HashSet<String> = ref_authors.iter().map(|a| normalize_author(a)).collect();
        let found_set: HashSet<String> =
            found_authors.iter().map(|a| normalize_author(a)).collect();
        !ref_set.is_disjoint(&found_set)
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

/// Normalize an author name to "FirstInitial surname" format for comparison.
fn normalize_author(name: &str) -> String {
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

/// Get the last name from an author name string.
fn get_last_name(name: &str) -> String {
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
}
