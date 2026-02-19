//! Tantivy search and fuzzy matching for OpenAlex queries.

use once_cell::sync::Lazy;
use regex::Regex;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexReader};

use crate::{OpenAlexError, OpenAlexQueryResult, OpenAlexRecord};

/// Default similarity threshold for fuzzy title matching (same as DBLP).
pub const DEFAULT_THRESHOLD: f64 = 0.90;

/// Normalize a title for comparison: lowercase alphanumeric only.
pub fn normalize_title(title: &str) -> String {
    static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9]").unwrap());
    let lowered = title.to_lowercase();
    NON_ALNUM.replace_all(&lowered, "").to_string()
}

/// Extract meaningful query words for Tantivy search (4+ chars, no stop words).
///
/// Duplicates DBLP's `get_query_words` logic for consistency.
pub fn get_query_words(title: &str) -> Vec<String> {
    // Strip BibTeX capitalization braces
    let title = title.replace(['{', '}'], "");

    static WORD_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"[a-zA-Z0-9]+(?:['\u{2019}\u{2018}\-][a-zA-Z0-9]+)*").unwrap());
    static STOP_WORDS: Lazy<std::collections::HashSet<&'static str>> = Lazy::new(|| {
        [
            "the", "and", "for", "with", "from", "that", "this", "have", "are", "was", "were",
            "been", "being", "has", "had", "does", "did", "will", "would", "could", "should",
            "may", "might", "must", "shall", "can", "not", "but", "its", "our", "their", "your",
            "into", "over", "under", "about", "between", "through", "during", "before", "after",
            "above", "below", "each", "every", "both", "few", "more", "most", "other", "some",
            "such", "only", "than", "too", "very",
        ]
        .into_iter()
        .collect()
    });

    let words_with_info: Vec<(String, String, usize)> = WORD_RE
        .find_iter(&title)
        .flat_map(|m| {
            m.as_str()
                .split('-')
                .map(|s| (s.to_string(), s.to_lowercase()))
                .collect::<Vec<_>>()
        })
        .enumerate()
        .map(|(i, (orig, lower))| (orig, lower, i))
        .filter(|(_, lower, _)| lower.len() >= 4 && !STOP_WORDS.contains(lower.as_str()))
        .collect();

    if words_with_info.len() <= 6 {
        return words_with_info
            .into_iter()
            .map(|(_, lower, _)| lower)
            .collect();
    }

    // Score words by distinctiveness and take top 6
    let mut scored: Vec<(f64, usize, String)> = words_with_info
        .iter()
        .map(|(orig, lower, pos)| {
            let mut score = lower.len() as f64;
            if orig.starts_with(|c: char| c.is_ascii_uppercase()) {
                score += 10.0;
            }
            if orig.len() >= 3
                && orig
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                score += 5.0;
            }
            score -= *pos as f64 * 0.5;
            (score, *pos, lower.clone())
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    scored.truncate(6);
    scored.sort_by_key(|&(_, pos, _)| pos);
    scored.into_iter().map(|(_, _, lower)| lower).collect()
}

/// Query the Tantivy index for a title, returning the best fuzzy match above the threshold.
pub fn query_index(
    index: &Index,
    reader: &IndexReader,
    title: &str,
    threshold: f64,
) -> Result<Option<OpenAlexQueryResult>, OpenAlexError> {
    let words = get_query_words(title);
    if words.is_empty() {
        return Ok(None);
    }

    let norm_query = normalize_title(title);
    if norm_query.is_empty() {
        return Ok(None);
    }

    let schema = index.schema();
    let title_field = schema
        .get_field("title")
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

    let query_parser = QueryParser::for_index(index, vec![title_field]);

    // Primary: all words joined with AND
    let query_str = words.join(" AND ");
    let result = tantivy_match(
        reader,
        &query_parser,
        &query_str,
        &norm_query,
        threshold,
        &schema,
    )?;
    if result.is_some() {
        return Ok(result);
    }

    // Fallback: top 3 words when primary returned nothing
    if words.len() > 3 {
        let fallback_str = words[..3].join(" AND ");
        return tantivy_match(
            reader,
            &query_parser,
            &fallback_str,
            &norm_query,
            threshold,
            &schema,
        );
    }

    Ok(None)
}

/// Run a Tantivy query and return the best fuzzy match above the threshold.
fn tantivy_match(
    reader: &IndexReader,
    query_parser: &QueryParser,
    query_str: &str,
    norm_query: &str,
    threshold: f64,
    schema: &Schema,
) -> Result<Option<OpenAlexQueryResult>, OpenAlexError> {
    let query = match query_parser.parse_query(query_str) {
        Ok(q) => q,
        Err(_) => return Ok(None),
    };

    let searcher = reader.searcher();
    let top_docs = searcher
        .search(&query, &TopDocs::with_limit(50))
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

    if top_docs.is_empty() {
        return Ok(None);
    }

    let title_field = schema
        .get_field("title")
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;
    let authors_field = schema
        .get_field("authors")
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

    let mut best_match: Option<(f64, String, Vec<String>)> = None;

    for (_score, doc_address) in top_docs {
        let doc = searcher
            .doc::<tantivy::TantivyDocument>(doc_address)
            .map_err(|e| OpenAlexError::Index(e.to_string()))?;

        let candidate_title = doc
            .get_first(title_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let norm_candidate = normalize_title(&candidate_title);
        if norm_candidate.is_empty() {
            continue;
        }

        let fuzzy_score = rapidfuzz::fuzz::ratio(norm_query.chars(), norm_candidate.chars());

        if fuzzy_score >= threshold
            && best_match
                .as_ref()
                .is_none_or(|(best, _, _)| fuzzy_score > *best)
        {
            let authors_str = doc
                .get_first(authors_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let authors: Vec<String> = if authors_str.is_empty() {
                vec![]
            } else {
                authors_str.split('|').map(|s| s.to_string()).collect()
            };

            best_match = Some((fuzzy_score, candidate_title, authors));
        }
    }

    match best_match {
        Some((score, matched_title, authors)) => Ok(Some(OpenAlexQueryResult {
            record: OpenAlexRecord {
                title: matched_title,
                authors,
                url: None,
            },
            score,
        })),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::doc;

    fn build_test_index() -> (Index, IndexReader) {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT | STORED);
        schema_builder.add_text_field("authors", STORED);
        schema_builder.add_u64_field("openalex_id", INDEXED | STORED | FAST);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let mut writer = index.writer(15_000_000).unwrap();

        let title_field = schema.get_field("title").unwrap();
        let authors_field = schema.get_field("authors").unwrap();
        let id_field = schema.get_field("openalex_id").unwrap();

        writer
            .add_document(doc!(
                title_field => "Attention is All you Need",
                authors_field => "Ashish Vaswani|Noam Shazeer",
                id_field => 1u64
            ))
            .unwrap();

        writer
            .add_document(doc!(
                title_field => "BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding",
                authors_field => "Jacob Devlin|Ming-Wei Chang",
                id_field => 2u64
            ))
            .unwrap();

        writer.commit().unwrap();

        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::Manual)
            .try_into()
            .unwrap();

        (index, reader)
    }

    #[test]
    fn test_normalize_title() {
        assert_eq!(normalize_title("Hello, World! 123"), "helloworld123");
        assert_eq!(normalize_title("  A--B  "), "ab");
    }

    #[test]
    fn test_get_query_words() {
        let words = get_query_words("Attention is All you Need");
        assert!(words.contains(&"attention".to_string()));
        assert!(words.contains(&"need".to_string()));
        assert!(!words.contains(&"is".to_string()));
    }

    #[test]
    fn test_query_exact_match() {
        let (index, reader) = build_test_index();
        let result = query_index(
            &index,
            &reader,
            "Attention is All you Need",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.score >= DEFAULT_THRESHOLD);
        assert_eq!(result.record.title, "Attention is All you Need");
        assert_eq!(result.record.authors.len(), 2);
        assert_eq!(result.record.authors[0], "Ashish Vaswani");
    }

    #[test]
    fn test_query_no_match() {
        let (index, reader) = build_test_index();
        let result = query_index(
            &index,
            &reader,
            "Completely Unrelated Paper About Marine Biology",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_query_empty() {
        let (index, reader) = build_test_index();
        let result = query_index(&index, &reader, "", DEFAULT_THRESHOLD).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_query_bert() {
        let (index, reader) = build_test_index();
        let result = query_index(
            &index,
            &reader,
            "BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.score >= DEFAULT_THRESHOLD);
    }
}
