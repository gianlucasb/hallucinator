//! FTS5 search and fuzzy matching for the local corpus.
//!
//! Same approach as `hallucinator-acl`'s query module (word-based FTS5
//! candidate retrieval, then rapidfuzz ratio scoring) — see that crate for
//! the rationale. This is the fix for exact-key fp_override matching being
//! too brittle: citations of the same paper that differ slightly in
//! wording, punctuation, or author-list truncation still resolve here.

use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{Connection, params};

use crate::db;
use crate::{CorpusError, CorpusQueryResult, CorpusRecord};

/// Default similarity threshold for fuzzy title matching. Matches the
/// threshold used throughout the rest of the pipeline (`matching::titles_match`).
pub const DEFAULT_THRESHOLD: f64 = 0.95;

/// Normalize a title for comparison: lowercase alphanumeric only.
fn normalize_title(title: &str) -> String {
    static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9]").unwrap());
    let lowered = title.to_lowercase();
    NON_ALNUM.replace_all(&lowered, "").to_string()
}

/// Extract meaningful query words for FTS5 MATCH (4+ chars, no stop words).
fn get_query_words(title: &str) -> Vec<String> {
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

    WORD_RE
        .find_iter(&title)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| w.len() >= 4 && !STOP_WORDS.contains(w.as_str()))
        .collect()
}

/// Query the FTS5 index for a title, returning the best match above the threshold.
pub fn query_fts(
    conn: &Connection,
    title: &str,
    threshold: f64,
) -> Result<Option<CorpusQueryResult>, CorpusError> {
    let words = get_query_words(title);
    if words.is_empty() {
        return Ok(None);
    }

    let fts_query = words.join(" ");

    let mut stmt = conn.prepare_cached(
        "SELECT p.id, p.title, p.url, p.source FROM publications p \
         WHERE p.id IN (SELECT rowid FROM publications_fts WHERE title MATCH ?1) \
         LIMIT 50",
    )?;

    let candidates: Vec<(i64, String, Option<String>, String)> = stmt
        .query_map(params![fts_query], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if candidates.is_empty() {
        return Ok(None);
    }

    let norm_query = normalize_title(title);
    if norm_query.is_empty() {
        return Ok(None);
    }

    let mut best_match: Option<(f64, i64, String, Option<String>, String)> = None;

    for (pub_id, candidate_title, url, source) in &candidates {
        let norm_candidate = normalize_title(candidate_title);
        if norm_candidate.is_empty() {
            continue;
        }

        let score = rapidfuzz::fuzz::ratio(norm_query.chars(), norm_candidate.chars());

        if score >= threshold
            && best_match
                .as_ref()
                .is_none_or(|(best, _, _, _, _)| score > *best)
        {
            best_match = Some((
                score,
                *pub_id,
                candidate_title.clone(),
                url.clone(),
                source.clone(),
            ));
        }
    }

    match best_match {
        Some((score, pub_id, matched_title, url, source)) => {
            let authors = db::get_authors_for_publication(conn, pub_id)?;
            Ok(Some(CorpusQueryResult {
                record: CorpusRecord {
                    title: matched_title,
                    authors,
                    url,
                    source,
                },
                score,
            }))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{NewPublication, init_database, insert_publication};

    fn setup_db_with_data() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();

        insert_publication(
            &conn,
            &NewPublication {
                title: "Attention Patterns in Transformer Models".to_string(),
                authors: vec!["Matt Post".to_string(), "David Vilar".to_string()],
                url: Some("https://example.org/1".to_string()),
                source: "ndss2026".to_string(),
            },
        )
        .unwrap();
        insert_publication(
            &conn,
            &NewPublication {
                title: "BERT Revisited for Low-Resource Language Understanding".to_string(),
                authors: vec!["Jane Doe".to_string()],
                url: Some("https://example.org/2".to_string()),
                source: "usenix2026".to_string(),
            },
        )
        .unwrap();

        conn
    }

    #[test]
    fn test_normalize_title() {
        assert_eq!(normalize_title("Hello, World! 123"), "helloworld123");
    }

    #[test]
    fn test_query_fts_exact_match() {
        let conn = setup_db_with_data();
        let result = query_fts(
            &conn,
            "Attention Patterns in Transformer Models",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.score >= DEFAULT_THRESHOLD);
        assert_eq!(result.record.authors.len(), 2);
        assert_eq!(result.record.source, "ndss2026");
    }

    #[test]
    fn test_query_fts_fuzzy_match_survives_punctuation_drift() {
        let conn = setup_db_with_data();
        // Trailing punctuation and different capitalization — normalize_title
        // strips both, so this is a 100%-score match. Real bibliographies mix
        // "Title." and "Title" / Title Case and Sentence case constantly.
        let result = query_fts(
            &conn,
            "attention patterns in transformer models.",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().score, 1.0);
    }

    #[test]
    fn test_query_fts_fuzzy_match_survives_inserted_filler_word() {
        let conn = setup_db_with_data();
        // A filler word ("the") some citation styles insert and others
        // drop — short enough (<4 chars) to be excluded from the FTS5
        // candidate-retrieval words, so it doesn't block the match, and
        // small enough that the rapidfuzz ratio still clears 95%. This is
        // exactly the class of drift that broke exact-identity
        // fp_override matching: the same underlying paper, cited with
        // slightly different wording.
        let result = query_fts(
            &conn,
            "Attention Patterns in the Transformer Models",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().score < 1.0);
    }

    #[test]
    fn test_query_fts_rejects_large_drift() {
        let conn = setup_db_with_data();
        // A whole extra parenthetical is real, useful conservatism — the
        // same threshold the rest of the pipeline (`matching::titles_match`)
        // already applies. Not every "close-ish" title should silently match.
        let result = query_fts(
            &conn,
            "Attention Patterns in Transformer Models (Extended Version)",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_query_fts_no_match() {
        let conn = setup_db_with_data();
        let result = query_fts(
            &conn,
            "Completely Unrelated Paper About Marine Biology",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_query_fts_empty() {
        let conn = setup_db_with_data();
        let result = query_fts(&conn, "", DEFAULT_THRESHOLD).unwrap();
        assert!(result.is_none());
    }
}
