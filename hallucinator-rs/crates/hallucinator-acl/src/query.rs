//! FTS5 search and fuzzy matching for ACL Anthology queries.

use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection};

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
fn get_query_words(title: &str) -> Vec<String> {
    static WORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[a-zA-Z]+").unwrap());
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
        .find_iter(title)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| w.len() >= 4 && !STOP_WORDS.contains(w.as_str()))
        .collect()
}

/// Query the FTS5 index for a title, returning the best match above the threshold.
pub fn query_fts(
    conn: &Connection,
    title: &str,
    threshold: f64,
) -> Result<Option<AclQueryResult>, AclError> {
    let words = get_query_words(title);
    if words.is_empty() {
        return Ok(None);
    }

    let fts_query = words.join(" ");

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

    let norm_query = normalize_title(title);
    if norm_query.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{init_database, insert_batch, rebuild_fts_index, InsertBatch};

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
}
