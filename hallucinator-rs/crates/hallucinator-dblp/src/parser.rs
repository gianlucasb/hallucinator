//! N-Triples line parser for DBLP RDF data.

/// DBLP RDF predicates we care about.
pub const TITLE: &str = "https://dblp.org/rdf/schema#title";
pub const DC_TITLE: &str = "http://purl.org/dc/terms/title";
pub const AUTHORED_BY: &str = "https://dblp.org/rdf/schema#authoredBy";
pub const PRIMARY_CREATOR_NAME: &str = "https://dblp.org/rdf/schema#primaryCreatorName";
pub const CREATOR_NAME: &str = "https://dblp.org/rdf/schema#creatorName";

/// A parsed RDF triple from an N-Triples line.
#[derive(Debug, Clone)]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// True if object is a URI (enclosed in `<>`), false if literal (quoted string).
    pub object_is_uri: bool,
}

/// Parse a single N-Triples line into a Triple.
///
/// N-Triples format:
/// - `<subject> <predicate> <object> .` (URI object)
/// - `<subject> <predicate> "literal"^^<type> .` (typed literal)
/// - `<subject> <predicate> "literal"@lang .` (language-tagged literal)
/// - `<subject> <predicate> "literal" .` (plain literal)
///
/// Returns None for comments, empty lines, or malformed lines.
pub fn parse_line(line: &str) -> Option<Triple> {
    let line = line.trim();

    // Skip empty lines and comments
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Parse subject (must be URI)
    let (subject, rest) = parse_uri(line)?;

    // Parse predicate (must be URI)
    let rest = rest.trim_start();
    let (predicate, rest) = parse_uri(rest)?;

    // Parse object (URI or literal)
    let rest = rest.trim_start();
    let (object, object_is_uri, rest) = parse_object(rest)?;

    // Verify line ends with " ."
    let rest = rest.trim_start();
    if rest != "." {
        return None;
    }

    Some(Triple {
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object,
        object_is_uri,
    })
}

/// Parse a URI enclosed in angle brackets: `<uri>`
/// Returns the URI content and the remaining string.
fn parse_uri(s: &str) -> Option<(&str, &str)> {
    if !s.starts_with('<') {
        return None;
    }

    let end = s[1..].find('>')?;
    let uri = &s[1..=end];
    let rest = &s[end + 2..];

    Some((uri, rest))
}

/// Parse an object (URI or literal).
/// Returns (value, is_uri, remaining_string).
fn parse_object(s: &str) -> Option<(String, bool, &str)> {
    if s.starts_with('<') {
        // URI object
        let (uri, rest) = parse_uri(s)?;
        Some((uri.to_string(), true, rest))
    } else if s.starts_with('"') {
        // Literal object
        let (literal, rest) = parse_literal(s)?;
        Some((literal, false, rest))
    } else {
        None
    }
}

/// Parse a quoted literal, handling escapes and stripping type/language annotations.
/// Returns the unescaped literal content and remaining string.
fn parse_literal(s: &str) -> Option<(String, &str)> {
    if !s.starts_with('"') {
        return None;
    }

    let mut chars = s[1..].char_indices();
    let mut result = String::new();
    let mut end_pos = 0;

    while let Some((i, c)) = chars.next() {
        match c {
            '"' => {
                // Found closing quote
                end_pos = i + 2; // +1 for the char, +1 for the opening quote
                break;
            }
            '\\' => {
                // Escape sequence
                if let Some((_, escaped)) = chars.next() {
                    match escaped {
                        'n' => result.push('\n'),
                        'r' => result.push('\r'),
                        't' => result.push('\t'),
                        '\\' => result.push('\\'),
                        '"' => result.push('"'),
                        'u' => {
                            // Unicode escape \uXXXX
                            let mut hex = String::with_capacity(4);
                            for _ in 0..4 {
                                if let Some((_, h)) = chars.next() {
                                    hex.push(h);
                                }
                            }
                            if let Ok(code) = u32::from_str_radix(&hex, 16)
                                && let Some(ch) = char::from_u32(code)
                            {
                                result.push(ch);
                            }
                        }
                        'U' => {
                            // Unicode escape \UXXXXXXXX
                            let mut hex = String::with_capacity(8);
                            for _ in 0..8 {
                                if let Some((_, h)) = chars.next() {
                                    hex.push(h);
                                }
                            }
                            if let Ok(code) = u32::from_str_radix(&hex, 16)
                                && let Some(ch) = char::from_u32(code)
                            {
                                result.push(ch);
                            }
                        }
                        _ => result.push(escaped),
                    }
                }
            }
            _ => result.push(c),
        }
    }

    let rest = &s[end_pos..];

    // Skip type annotation (^^<type>) or language tag (@lang)
    let rest = if rest.starts_with("^^") {
        // Skip type URI
        if let Some(end) = rest.find('>') {
            &rest[end + 1..]
        } else {
            rest
        }
    } else if rest.starts_with('@') {
        // Skip language tag (until whitespace)
        rest.trim_start_matches(|c: char| !c.is_whitespace())
    } else {
        rest
    };

    Some((result, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uri_object() {
        let line = r#"<https://dblp.org/rec/conf/nips/VaswaniSPUJGKP17> <https://dblp.org/rdf/schema#authoredBy> <https://dblp.org/pid/05/5893> ."#;
        let triple = parse_line(line).unwrap();
        assert_eq!(
            triple.subject,
            "https://dblp.org/rec/conf/nips/VaswaniSPUJGKP17"
        );
        assert_eq!(triple.predicate, "https://dblp.org/rdf/schema#authoredBy");
        assert_eq!(triple.object, "https://dblp.org/pid/05/5893");
        assert!(triple.object_is_uri);
    }

    #[test]
    fn test_parse_literal_object() {
        let line = r#"<https://dblp.org/rec/conf/nips/VaswaniSPUJGKP17> <https://dblp.org/rdf/schema#title> "Attention is All you Need" ."#;
        let triple = parse_line(line).unwrap();
        assert_eq!(triple.object, "Attention is All you Need");
        assert!(!triple.object_is_uri);
    }

    #[test]
    fn test_parse_typed_literal() {
        let line = r#"<https://dblp.org/rec/123> <http://purl.org/dc/terms/title> "Some Title"^^<http://www.w3.org/2001/XMLSchema#string> ."#;
        let triple = parse_line(line).unwrap();
        assert_eq!(triple.object, "Some Title");
    }

    #[test]
    fn test_parse_language_tagged() {
        let line =
            r#"<https://dblp.org/rec/123> <http://purl.org/dc/terms/title> "Some Title"@en ."#;
        let triple = parse_line(line).unwrap();
        assert_eq!(triple.object, "Some Title");
    }

    #[test]
    fn test_parse_escaped_quote() {
        let line =
            r#"<https://dblp.org/rec/123> <http://purl.org/dc/terms/title> "He said \"hello\"" ."#;
        let triple = parse_line(line).unwrap();
        assert_eq!(triple.object, "He said \"hello\"");
    }

    #[test]
    fn test_skip_comment() {
        let line = "# This is a comment";
        assert!(parse_line(line).is_none());
    }

    #[test]
    fn test_skip_empty() {
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
    }
}
