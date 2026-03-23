//! FTS5 search and fuzzy matching for DBLP queries.

use hallucinator_common::fuzzy;
use rusqlite::{Connection, params};

use crate::db;
use crate::{DblpError, DblpQueryResult, DblpRecord};

/// Default similarity threshold for fuzzy title matching.
///
/// After `normalize_title()` reduces both titles to lowercase alphanumeric, this
/// threshold is applied to `rapidfuzz::fuzz::ratio`. At 0.90, a 40-alnum-char
/// title can differ by ~4 characters and still match.
///
/// **False negative risk:** A hallucinated title that is >=90% similar to a real
/// DBLP entry will incorrectly pass validation as "verified." This is a known
/// limitation — we currently have no empirical dataset of known-fabricated titles
/// to measure this false negative rate. The existing test harness
/// (`problematic_papers`) only measures recall (can we find real papers?), not
/// precision (do we reject fabricated ones?).
///
/// The 90% threshold was chosen as a pragmatic tradeoff: higher values (93-95%)
/// reject too many legitimate matches caused by trailing periods, LaTeX artifacts,
/// and minor wording differences between BibTeX and DBLP titles. At 90% vs 95%,
/// overall recall improves from 42% to 92% on the problematic-papers dataset.
///
/// Note that the FTS5 query acts as a first gate — a fabricated title must share
/// 3-6 distinctive keywords (AND query) with a real paper before fuzzy matching
/// even runs. This significantly reduces the false negative surface.
pub const DEFAULT_THRESHOLD: f64 = 0.90;

/// Re-export normalize_title from the common crate.
pub fn normalize_title(title: &str) -> String {
    fuzzy::normalize_title_simple(title)
}

/// Re-export get_query_words from the common crate.
pub fn get_query_words(title: &str) -> Vec<String> {
    fuzzy::get_query_words(title, 6)
}

/// Run an FTS5 query and return the best fuzzy match above the threshold.
fn fts_match(
    conn: &Connection,
    fts_query: &str,
    norm_query: &str,
    threshold: f64,
) -> Result<Option<DblpQueryResult>, DblpError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.id, p.key, p.title FROM publications p \
         WHERE p.id IN (SELECT rowid FROM publications_fts WHERE title MATCH ?1) \
         LIMIT 50",
    )?;

    let candidates: Vec<(i64, String, String)> = stmt
        .query_map(params![fts_query], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if candidates.is_empty() {
        return Ok(None);
    }

    let mut best_match: Option<(f64, i64, String, String)> = None;

    for (id, key, candidate_title) in &candidates {
        let norm_candidate = normalize_title(candidate_title);
        if norm_candidate.is_empty() {
            continue;
        }

        let score = rapidfuzz::fuzz::ratio(norm_query.chars(), norm_candidate.chars());

        if score >= threshold
            && best_match
                .as_ref()
                .is_none_or(|(best, _, _, _)| score > *best)
        {
            best_match = Some((score, *id, key.clone(), candidate_title.clone()));
        }
    }

    match best_match {
        Some((score, id, key, matched_title)) => {
            let authors = db::get_authors_for_publication(conn, id)?;
            let url = format!("https://dblp.org/rec/{}", key);
            Ok(Some(DblpQueryResult {
                record: DblpRecord {
                    title: matched_title,
                    authors,
                    url: Some(url),
                },
                score,
            }))
        }
        None => Ok(None),
    }
}

/// Query the FTS5 index for a title, returning the best match above the threshold.
pub fn query_fts(
    conn: &Connection,
    title: &str,
    threshold: f64,
) -> Result<Option<DblpQueryResult>, DblpError> {
    let words = get_query_words(title);
    if words.is_empty() {
        return Ok(None);
    }

    let norm_query = normalize_title(title);
    if norm_query.is_empty() {
        return Ok(None);
    }

    // Primary query: all words joined with AND
    let fts_query = words.join(" ");
    let result = fts_match(conn, &fts_query, &norm_query, threshold)?;
    if result.is_some() {
        return Ok(result);
    }

    // Fallback: retry with top 3 words when primary query returned nothing
    if words.len() > 3 {
        let fallback_query = words[..3].join(" ");
        return fts_match(conn, &fallback_query, &norm_query, threshold);
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        InsertBatch, init_database, insert_batch, insert_or_get_author, insert_or_get_publication,
        rebuild_fts_index,
    };

    fn setup_db_with_data() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();

        let vaswani_id = insert_or_get_author(&conn, "Ashish Vaswani").unwrap();
        let shazeer_id = insert_or_get_author(&conn, "Noam Shazeer").unwrap();
        let attention_id = insert_or_get_publication(
            &conn,
            "conf/nips/VaswaniSPUJGKP17",
            "Attention is All you Need",
        )
        .unwrap();
        insert_or_get_publication(
            &conn,
            "conf/naacl/DevlinCLT19",
            "BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding",
        )
        .unwrap();

        let mut batch = InsertBatch::new();
        batch.publication_authors.push((attention_id, vaswani_id));
        batch.publication_authors.push((attention_id, shazeer_id));
        insert_batch(&conn, &batch).unwrap();
        rebuild_fts_index(&conn).unwrap();

        conn
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
        // "is", "all", "you" are too short or stop words
        assert!(!words.contains(&"is".to_string()));
    }

    #[test]
    fn test_get_query_words_bibtex_braces() {
        let words = get_query_words("{BERT}: Pre-training of Deep Bidirectional Transformers");
        assert!(words.contains(&"bert".to_string()));
        // Hyphens are split: "pre-training" → "training" (pre is <4 chars)
        assert!(words.contains(&"training".to_string()));
    }

    #[test]
    fn test_get_query_words_hyphenated() {
        let words = get_query_words("Machine-Learning Approaches for Natural Language");
        // Hyphens are split for FTS5 compatibility
        assert!(words.contains(&"machine".to_string()));
        assert!(words.contains(&"learning".to_string()));
    }

    #[test]
    fn test_get_query_words_digits() {
        let words = get_query_words("L2 Regularization for 3D Point Cloud Models");
        // "l2" and "3d" are too short, but "point", "cloud", "models" should be present
        assert!(words.contains(&"point".to_string()));
        assert!(words.contains(&"regularization".to_string()));
    }

    #[test]
    fn test_query_fts_exact_match() {
        let conn = setup_db_with_data();
        let result = query_fts(&conn, "Attention is All you Need", DEFAULT_THRESHOLD).unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.score >= DEFAULT_THRESHOLD);
        assert_eq!(result.record.title, "Attention is All you Need");
        assert_eq!(result.record.authors.len(), 2);
        assert_eq!(
            result.record.url,
            Some("https://dblp.org/rec/conf/nips/VaswaniSPUJGKP17".to_string())
        );
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
