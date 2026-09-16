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

/// Quote a single word for safe use inside an FTS5 MATCH query string.
///
/// `get_query_words` keeps internal hyphens/apostrophes as part of one
/// token (e.g. "Branch-Guided" -> `branch-guided`), but FTS5's own query
/// mini-language treats bareword `-` as an operator character — an
/// unquoted term like `branch-guided` fails to parse as the literal token
/// and errors out instead of matching. That error was silently swallowed
/// by `fts_match`'s `.filter_map(|r| r.ok())`, making a query containing
/// any hyphenated word look like "zero candidates" instead of a query
/// syntax error — a real, silent false-negative bug (confirmed: FTS5
/// happily finds "Bulbasaur: Branch-Guided ..." via `MATCH 'bulbasaur AND
/// "branch-guided"'` but errors on the unquoted form). Wrapping each term
/// in double quotes makes FTS5 treat it as a literal string instead.
fn quote_fts_term(word: &str) -> String {
    format!("\"{}\"", word.replace('"', "\"\""))
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
) -> Result<Option<CorpusQueryResult>, CorpusError> {
    let words = get_query_words(title);
    if words.is_empty() {
        return Ok(None);
    }
    let norm_query = normalize_title(title);
    if norm_query.is_empty() {
        return Ok(None);
    }

    let fts_query = words
        .iter()
        .map(|w| quote_fts_term(w))
        .collect::<Vec<_>>()
        .join(" ");
    let result = fts_match(conn, &fts_query, &norm_query, threshold)?;
    if result.is_some() {
        return Ok(result);
    }

    if words.len() > 3 {
        let fallback_query = words[..3]
            .iter()
            .map(|w| quote_fts_term(w))
            .collect::<Vec<_>>()
            .join(" ");
        let result = fts_match(conn, &fallback_query, &norm_query, threshold)?;
        if result.is_some() {
            return Ok(result);
        }
    }

    if words.len() >= 2 {
        let take = words.len().min(4);
        let or_words = select_or_fallback_words(conn, &words, take);
        let or_query = or_words
            .iter()
            .map(|w| quote_fts_term(w))
            .collect::<Vec<_>>()
            .join(" OR ");
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
) -> Result<Option<CorpusQueryResult>, CorpusError> {
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

    #[test]
    fn test_hyphenated_word_in_title_does_not_break_the_query() {
        // Regression: FTS5's bareword query parser treats an unquoted
        // internal `-` as an operator character, so a query word like
        // "branch-guided" (kept as one token by `get_query_words` because
        // WORD_RE treats hyphens as word-internal) failed to parse at all
        // when joined unquoted into the MATCH string — and that parse
        // error was silently swallowed by `fts_match`'s
        // `.filter_map(|r| r.ok())`, making a real, indexed title look
        // like "zero candidates". Confirmed against production data:
        // "Bulbasaur: Branch-Guided Online Mutator Generation for Greybox
        // Fuzzing" (real USENIX Security 2026 paper) was unfindable via
        // `CorpusPool::query` despite `MATCH 'bulbasaur'` finding it
        // trivially at the raw SQL level.
        let conn = Connection::open_in_memory().unwrap();
        db::init_database(&conn).unwrap();
        db::insert_publication(
            &conn,
            &db::NewPublication {
                title: "Bulbasaur: Branch-Guided Online Mutator Generation for Greybox Fuzzing"
                    .to_string(),
                authors: vec!["A. Researcher".to_string()],
                url: None,
                source: "usenix2026".to_string(),
            },
        )
        .unwrap();

        let result = query_fts(
            &conn,
            "Bulbasaur: Branch-Guided Online Mutator Generation for Greybox Fuzzing",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(
            result.is_some(),
            "a title containing a hyphenated word must still be found, not silently dropped"
        );
        assert_eq!(result.unwrap().score, 1.0);
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
        db::insert_publication(
            &conn,
            &db::NewPublication {
                title: "Attention Patterns Deep Transformer Networks For Sequence Classification"
                    .to_string(),
                authors: vec!["A. Researcher".to_string()],
                url: None,
                source: "ndss2026".to_string(),
            },
        )
        .unwrap();

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
