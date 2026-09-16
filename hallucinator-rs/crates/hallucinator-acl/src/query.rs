//! FTS5 search and fuzzy matching for ACL Anthology queries.

use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{Connection, params};

use crate::db;
use crate::{AclError, AclQueryResult, AclRecord};

/// Default similarity threshold for fuzzy title matching.
pub const DEFAULT_THRESHOLD: f64 = 0.95;

/// Normalize a title for comparison: lowercase alphanumeric only.
fn normalize_title(title: &str) -> String {
    static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9]").unwrap());
    let lowered = title.to_lowercase();
    NON_ALNUM.replace_all(&lowered, "").to_string()
}

/// Extract meaningful query words for FTS5 MATCH (4+ chars, no stop words).
///
/// Handles digits (`L2`, `3D`), hyphens (`Machine-Learning`), and apostrophes (`What's`).
/// Also strips BibTeX braces (`{BERT}` → `BERT`).
fn get_query_words(title: &str) -> Vec<String> {
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

    WORD_RE
        .find_iter(&title)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| w.len() >= 4 && !STOP_WORDS.contains(w.as_str()))
        .collect()
}

/// Query the FTS5 index for a title, returning the best match above the threshold.
///
/// Three-tier fallback (mirrors `hallucinator_dblp::query::query_fts_with_authors`):
/// 1. All meaningful words AND-joined (most precise).
/// 2. Top-3 words AND-joined, if (1) found no passing candidate.
/// 3. OR-join of the 4 most selective (rarest) words, if (1) and (2) both
///    missed. Catches citations where the indexed title omits or adds a
///    word the AND queries required (a dropped subtitle, a paraphrase) —
///    confirmed as a real, common cause of false NotFound results: an
///    AND-only query returned zero candidates for the vast majority of a
///    ~1000-reference sample even when the paper was genuinely indexed.
///
/// Each tier still has to clear `threshold` via fuzzy match — OR widens
/// *candidate retrieval*, not acceptance criteria, so precision is
/// unaffected.
pub fn query_fts(
    conn: &Connection,
    title: &str,
    threshold: f64,
) -> Result<Option<AclQueryResult>, AclError> {
    let words = get_query_words(title);
    if words.is_empty() {
        return Ok(None);
    }
    let norm_query = normalize_title(title);
    if norm_query.is_empty() {
        return Ok(None);
    }

    let fts_query = words.join(" ");
    let result = fts_match(conn, &fts_query, &norm_query, threshold)?;
    if result.is_some() {
        return Ok(result);
    }

    if words.len() > 3 {
        let fallback_query = words[..3].join(" ");
        let result = fts_match(conn, &fallback_query, &norm_query, threshold)?;
        if result.is_some() {
            return Ok(result);
        }
    }

    if words.len() >= 2 {
        let take = words.len().min(4);
        let or_words = select_or_fallback_words(conn, &words, take);
        let or_query = or_words.join(" OR ");
        return fts_match(conn, &or_query, &norm_query, threshold);
    }

    Ok(None)
}

/// Run one FTS5 query and return the best fuzzy match above `threshold`.
fn fts_match(
    conn: &Connection,
    fts_query: &str,
    norm_query: &str,
    threshold: f64,
) -> Result<Option<AclQueryResult>, AclError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.anthology_id, p.title, p.url FROM publications p \
         WHERE p.id IN (SELECT rowid FROM publications_fts WHERE title MATCH ?1) \
         LIMIT 50",
    )?;

    let candidates: Vec<(String, String, Option<String>)> = stmt
        .query_map(params![fts_query], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if candidates.is_empty() {
        return Ok(None);
    }

    let mut best_match: Option<(f64, String, String, Option<String>)> = None;

    for (anthology_id, candidate_title, url) in &candidates {
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
            best_match = Some((
                score,
                anthology_id.clone(),
                candidate_title.clone(),
                url.clone(),
            ));
        }
    }

    match best_match {
        Some((score, anthology_id, matched_title, url)) => {
            let authors = db::get_authors_for_publication(conn, &anthology_id)?;
            Ok(Some(AclQueryResult {
                record: AclRecord {
                    title: matched_title,
                    authors,
                    url,
                },
                score,
            }))
        }
        None => Ok(None),
    }
}

/// Pick the `take` most selective (lowest document-frequency) words from
/// `words` for the OR-fallback query. Falls back to extraction order when
/// frequency data isn't available (`ensure_vocab_table` failed on this
/// connection). See `hallucinator_dblp::query::select_or_fallback_words`.
fn select_or_fallback_words(conn: &Connection, words: &[String], take: usize) -> Vec<String> {
    let freqs = term_doc_frequencies(conn, words);
    if freqs.is_empty() {
        return words[..take].to_vec();
    }
    let mut ranked: Vec<&String> = words.iter().collect();
    ranked.sort_by_key(|w| freqs.get(w.as_str()).copied().unwrap_or(0));
    ranked.into_iter().take(take).cloned().collect()
}

/// Look up FTS5 document frequency (`fts5vocab` 'row' mode's `doc` column)
/// for each of `words`, via the session-local vocab table
/// `db::ensure_vocab_table` creates. Returns an empty map if that table
/// doesn't exist on this connection.
fn term_doc_frequencies(
    conn: &Connection,
    words: &[String],
) -> std::collections::HashMap<String, i64> {
    let mut freqs = std::collections::HashMap::new();
    let Ok(mut stmt) =
        conn.prepare_cached("SELECT doc FROM temp.publications_vocab WHERE term = ?1")
    else {
        return freqs;
    };
    for w in words {
        if let Ok(doc) = stmt.query_row(params![w], |row| row.get::<_, i64>(0)) {
            freqs.insert(w.clone(), doc);
        }
    }
    freqs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{InsertBatch, init_database, insert_batch, rebuild_fts_index};

    fn setup_db_with_data() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();

        let mut batch = InsertBatch::new();
        batch.authors.push("Matt Post".to_string());
        batch.authors.push("David Vilar".to_string());
        batch.publications.push((
            "2024.acl-long.1".to_string(),
            "Attention Patterns in Transformer Models".to_string(),
            Some("https://aclanthology.org/2024.acl-long.1".to_string()),
            None,
        ));
        batch.publications.push((
            "2023.emnlp-main.5".to_string(),
            "BERT Revisited for Low-Resource Language Understanding".to_string(),
            Some("https://aclanthology.org/2023.emnlp-main.5".to_string()),
            None,
        ));
        batch
            .publication_authors
            .push(("2024.acl-long.1".to_string(), "Matt Post".to_string(), 0));
        batch.publication_authors.push((
            "2024.acl-long.1".to_string(),
            "David Vilar".to_string(),
            1,
        ));
        insert_batch(&conn, &batch).unwrap();
        rebuild_fts_index(&conn).unwrap();

        conn
    }

    #[test]
    fn test_normalize_title() {
        assert_eq!(normalize_title("Hello, World! 123"), "helloworld123");
    }

    #[test]
    fn test_get_query_words() {
        let words = get_query_words("Attention Patterns in Transformer Models");
        assert!(words.contains(&"attention".to_string()));
        assert!(words.contains(&"patterns".to_string()));
        assert!(words.contains(&"transformer".to_string()));
        assert!(words.contains(&"models".to_string()));
    }

    #[test]
    fn test_get_query_words_bibtex_braces() {
        let words = get_query_words("{BERT}: Pre-training of Deep Bidirectional Transformers");
        assert!(words.contains(&"bert".to_string()));
        assert!(words.contains(&"pre-training".to_string()));
    }

    #[test]
    fn test_get_query_words_hyphenated() {
        let words = get_query_words("Machine-Learning Approaches for Natural Language");
        assert!(words.contains(&"machine-learning".to_string()));
    }

    #[test]
    fn test_get_query_words_digits() {
        let words = get_query_words("L2 Regularization for 3D Point Cloud Models");
        assert!(words.contains(&"point".to_string()));
        assert!(words.contains(&"regularization".to_string()));
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

    #[test]
    fn test_or_fallback_finds_when_one_citation_word_mismatches_db() {
        // The OR fallback exists to handle citations where one of the
        // meaningful query words isn't present in the DB title verbatim
        // (typo, singular/plural drift, paraphrase). AND requires every
        // word, so a single mismatch drops the whole candidate; OR still
        // finds it via the other words, and the fuzzy-similarity gate
        // then decides whether the candidate is actually the right paper.
        //
        // Here the citation says "Pattern" (singular) where the DB has
        // "Patterns" (plural) — a one-character difference that breaks
        // exact FTS5 token matching (AND requires the literal token
        // "pattern", which the index doesn't have) but barely dents the
        // fuzzy-similarity score once retrieved.
        let conn = Connection::open_in_memory().unwrap();
        db::init_database(&conn).unwrap();
        let mut batch = InsertBatch::new();
        batch.publications.push((
            "2024.acl-long.99".to_string(),
            "Attention Patterns Deep Transformer Networks For Sequence Classification".to_string(),
            None,
            None,
        ));
        insert_batch(&conn, &batch).unwrap();
        rebuild_fts_index(&conn).unwrap();

        let result = query_fts(
            &conn,
            "Attention Pattern Deep Transformer Networks For Sequence Classification",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(
            result.is_some(),
            "OR fallback should find a candidate when one citation word is absent from the DB title"
        );
    }
}
