//! FTS5 search and fuzzy matching for DBLP queries.

use once_cell::sync::Lazy;
use regex::Regex;
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

/// Strip DBLP-style annotation prefixes and suffixes that don't appear in
/// the cited form of a title.
///
/// DBLP routinely stores variants like
///   - `Extended Abstract: HotStuff-2: Optimal Two-Phase Responsive BFT.`
///   - `Brief Announcement: Byzantine Agreement, Broadcast and State Machine Replication ...`
///   - `LTE-advanced: next-generation wireless broadband technology [Invited Paper].`
///   - `Another Advantage of Free Choice: Completely Asynchronous Agreement Protocols (Extended Abstract)`
///   - `The square lattice shuffle, correction.`
///
/// References cite the core title without these annotations, so comparing them
/// literally drops the rapidfuzz ratio below the 0.90 threshold. Stripping on
/// both sides before normalisation lets real matches cross the threshold while
/// preserving precision on truly different titles.
pub fn strip_title_decorations(title: &str) -> String {
    static PREFIX: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)^\s*(?:extended abstract|brief announcement|invited paper|keynote(?: talk)?|tutorial|short paper|work[- ]in[- ]progress|wip|poster)\s*:\s*",
        )
        .unwrap()
    });
    static SUFFIX_BRACKET: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\s*\[(?:invited paper|extended abstract)\]\s*\.?\s*$").unwrap());
    static SUFFIX_PAREN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\s*\((?:extended abstract|invited paper|short paper)\)\s*\.?\s*$")
            .unwrap()
    });
    static SUFFIX_CORRECTION: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i),\s*correction\s*\.?\s*$").unwrap());

    let s = PREFIX.replace(title, "");
    let s = SUFFIX_BRACKET.replace(&s, "");
    let s = SUFFIX_PAREN.replace(&s, "");
    let s = SUFFIX_CORRECTION.replace(&s, "");
    s.trim().trim_end_matches('.').trim().to_string()
}

/// Normalize a title for comparison: lowercase alphanumeric only.
///
/// This is a simplified inline version to avoid depending on hallucinator-core.
pub fn normalize_title(title: &str) -> String {
    static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9]").unwrap());
    let lowered = title.to_lowercase();
    NON_ALNUM.replace_all(&lowered, "").to_string()
}

/// Strip LaTeX markup from a title string for FTS5 query extraction.
///
/// Handles common LaTeX commands found in BibTeX title fields:
/// `\textquoteright` → `'`, `\textendash` → `-`, `$...$` → stripped,
/// `\mathbb{X}` → `X`, `\text{X}` → `X`, `\command` → stripped.
fn strip_latex_for_query(title: &str) -> String {
    static MATH_MODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$[^$]*\$").unwrap());
    static CMD_WITH_ARG: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\\(?:mathbb|mathcal|mathrm|mathit|mathbf|text|textbf|textit|textsc|textrm|emph)\s*\{([^}]*)\}").unwrap()
    });
    static BARE_CMD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\[a-zA-Z]+").unwrap());

    let mut s = title.to_string();

    // Named character commands → replacement
    s = s.replace("\\textquoteright", "'");
    s = s.replace("\\textquoteleft", "'");
    s = s.replace("\\textendash", "-");
    s = s.replace("\\textemdash", "--");

    // Remove math mode entirely: $\mathbb{Z}_p$ → empty
    s = MATH_MODE.replace_all(&s, "").to_string();

    // \mathbb{X}, \text{X}, \emph{X}, etc. → X
    s = CMD_WITH_ARG.replace_all(&s, "$1").to_string();

    // Strip remaining bare \commands
    s = BARE_CMD.replace_all(&s, "").to_string();

    s
}

/// Extract meaningful query words for FTS5 MATCH (4+ chars, no stop words).
///
/// Handles digits (`L2`, `3D`), hyphens (`Machine-Learning`), and apostrophes (`What's`).
/// Also strips BibTeX braces (`{BERT}` → `BERT`) and LaTeX markup.
pub fn get_query_words(title: &str) -> Vec<String> {
    // Strip LaTeX markup and BibTeX capitalization braces
    let title = strip_latex_for_query(title);
    let title = title.replace(['{', '}'], "");

    static WORD_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"[a-zA-Z0-9]+(?:['\u{2019}\u{2018}\-][a-zA-Z0-9]+)*").unwrap());
    static STOP_WORDS: Lazy<std::collections::HashSet<&'static str>> = Lazy::new(|| {
        [
            // 4+ char stop words (historical length-filter covered short ones).
            "the", "and", "for", "with", "from", "that", "this", "have", "are", "was", "were",
            "been", "being", "has", "had", "does", "did", "will", "would", "could", "should",
            "may", "might", "must", "shall", "can", "not", "but", "its", "our", "their", "your",
            "into", "over", "under", "about", "between", "through", "during", "before", "after",
            "above", "below", "each", "every", "both", "few", "more", "most", "other", "some",
            "such", "only", "than", "too", "very",
            // Short (2-3 char) prepositions / articles / copulas. Needed now
            // that short non-acronym tokens can pass the filter as a fallback
            // when the strong-token set is sparse.
            "is", "as", "of", "to", "in", "on", "at", "it", "or", "an", "be", "we", "by",
            "if", "so", "up", "do", "no",
        ]
        .into_iter()
        .collect()
    });

    // Collect all candidate tokens first, annotated with whether they're
    // "strong" (length >= 4) or "weak" (short all-caps/letter-digit acronyms
    // like DAG, LTE, L2, 3D, or residual non-stopword short tokens like the
    // lowercase "dag" in a cited title). We always keep strong tokens; weak
    // tokens are added only when the strong set is sparse, so common titles
    // stay specific without losing distinctive short keywords.
    let all_tokens: Vec<(String, String, usize, bool)> = WORD_RE
        .find_iter(&title)
        .flat_map(|m| {
            // Split hyphenated words into parts since FTS5's unicode61 tokenizer
            // splits on hyphens. "internet-of-things" → ["internet", "things"]
            m.as_str()
                .split('-')
                .map(|s| (s.to_string(), s.to_lowercase()))
                .collect::<Vec<_>>()
        })
        .enumerate()
        .filter_map(|(i, (orig, lower))| {
            if STOP_WORDS.contains(lower.as_str()) {
                return None;
            }
            let is_strong = lower.len() >= 4;
            let is_acronym = orig.len() >= 2
                && orig.chars().any(|c| c.is_ascii_alphabetic())
                && orig
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
            let is_short_distinctive = lower.len() >= 2 && lower.len() < 4;
            if is_strong || is_acronym || is_short_distinctive {
                Some((orig, lower, i, is_strong || is_acronym))
            } else {
                None
            }
        })
        .collect();

    // Prefer strong tokens. If we have < 3 strong tokens, include weak ones
    // to give FTS5 enough signal — this catches cases like "All you need is
    // dag" where a short, lowercase last word ("dag") carries most of the
    // distinctiveness but the length filter would have dropped it.
    let strong_count = all_tokens.iter().filter(|(_, _, _, s)| *s).count();
    let words_with_info: Vec<(String, String, usize)> = if strong_count >= 3 {
        all_tokens
            .into_iter()
            .filter_map(|(o, l, p, s)| if s { Some((o, l, p)) } else { None })
            .collect()
    } else {
        all_tokens.into_iter().map(|(o, l, p, _)| (o, l, p)).collect()
    };

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
            // Proper nouns / distinctive words
            if orig.starts_with(|c: char| c.is_ascii_uppercase()) {
                score += 10.0;
            }
            // Acronyms (e.g., BERT, NLP)
            if orig.len() >= 3
                && orig
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                score += 5.0;
            }
            // Prefer earlier words
            score -= *pos as f64 * 0.5;
            (score, *pos, lower.clone())
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    scored.truncate(6);
    // Restore original order for natural query phrasing
    scored.sort_by_key(|&(_, pos, _)| pos);
    scored.into_iter().map(|(_, _, lower)| lower).collect()
}

/// Run an FTS5 query and return the best fuzzy match above the threshold.
///
/// `norm_query_stripped` is the decoration-stripped normalized form of the
/// query title — passing it in (rather than recomputing) allows the caller
/// to avoid re-stripping inside every row of the match loop.
fn fts_match(
    conn: &Connection,
    fts_query: &str,
    norm_query: &str,
    norm_query_stripped: &str,
    threshold: f64,
) -> Result<Option<DblpQueryResult>, DblpError> {
    // ORDER BY rank (FTS5 BM25) surfaces the most relevant titles within the
    // LIMIT window. Without it, common-word queries (e.g. "need" for titles
    // like "All You Need is DAG") can drown the target in 7k+ insertion-order
    // candidates and miss it entirely.
    let mut stmt = conn.prepare_cached(
        "SELECT p.id, p.key, p.title FROM publications p \
         JOIN publications_fts f ON p.id = f.rowid \
         WHERE f.title MATCH ?1 \
         ORDER BY f.rank \
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

        // Pairwise: raw/stripped on both sides. Stripping DBLP decoration
        // prefixes/suffixes ("Extended Abstract:", "[Invited Paper]",
        // ", correction.") recovers near-matches that citations omit.
        let raw_score = rapidfuzz::fuzz::ratio(norm_query.chars(), norm_candidate.chars());
        let score = if raw_score >= threshold {
            raw_score
        } else {
            let stripped_candidate = strip_title_decorations(candidate_title);
            let norm_stripped = normalize_title(&stripped_candidate);
            if norm_stripped.is_empty() || norm_stripped == norm_candidate {
                raw_score.max(rapidfuzz::fuzz::ratio(
                    norm_query_stripped.chars(),
                    norm_candidate.chars(),
                ))
            } else {
                let a =
                    rapidfuzz::fuzz::ratio(norm_query.chars(), norm_stripped.chars());
                let b = rapidfuzz::fuzz::ratio(
                    norm_query_stripped.chars(),
                    norm_stripped.chars(),
                );
                raw_score.max(a).max(b)
            }
        };

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

    // Pre-compute the decoration-stripped normalized form once so the
    // per-candidate loop in fts_match can compare both variants cheaply.
    let norm_query_stripped = normalize_title(&strip_title_decorations(title));

    // Primary query: all words joined with AND
    let fts_query = words.join(" ");
    let result = fts_match(
        conn,
        &fts_query,
        &norm_query,
        &norm_query_stripped,
        threshold,
    )?;
    if result.is_some() {
        return Ok(result);
    }

    // Fallback: retry with top 3 words when primary query returned nothing
    if words.len() > 3 {
        let fallback_query = words[..3].join(" ");
        return fts_match(
            conn,
            &fallback_query,
            &norm_query,
            &norm_query_stripped,
            threshold,
        );
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
        assert!(words.contains(&"point".to_string()));
        assert!(words.contains(&"regularization".to_string()));
        // Short all-caps / digit-letter combinations are kept as acronyms.
        assert!(words.contains(&"l2".to_string()));
        assert!(words.contains(&"3d".to_string()));
    }

    #[test]
    fn test_get_query_words_short_acronyms() {
        // All-caps 2-3 char acronyms are kept — they're high-signal keywords
        // that the 4+ char length filter would otherwise drop, leaving
        // generic titles with too few query terms.
        let words = get_query_words("All You Need is DAG");
        assert!(
            words.contains(&"dag".to_string()),
            "DAG acronym must be kept: {:?}",
            words
        );

        let words = get_query_words("LTE-advanced: next-generation wireless broadband technology");
        assert!(
            words.contains(&"lte".to_string()),
            "LTE acronym must be kept: {:?}",
            words
        );

        let words = get_query_words("SoK: Automated TTP extraction from CTI reports");
        // "TTP" and "CTI" are all-caps; kept. "SoK" is mixed-case; not kept.
        assert!(words.contains(&"ttp".to_string()));
        assert!(words.contains(&"cti".to_string()));
        assert!(!words.contains(&"sok".to_string()));
    }

    #[test]
    fn test_get_query_words_weak_fallback_for_sparse_titles() {
        // "All you need is dag" (lowercase) — after stop-word filtering only
        // "need" is strong (4 chars). With the weak-token fallback, "dag"
        // (3 chars, lowercase) is also included so FTS has enough signal.
        // "all"/"you"/"is" are stop words.
        let words = get_query_words("All you need is dag");
        assert!(words.contains(&"need".to_string()));
        assert!(
            words.contains(&"dag".to_string()),
            "short lowercase 'dag' must fall back in when strong tokens are sparse: {:?}",
            words
        );
    }

    #[test]
    fn test_get_query_words_rejects_short_lowercase() {
        // With three strong tokens in the title, short lowercase words (and
        // stop words) stay filtered out; the weak-token fallback only fires
        // when the strong set is sparse.
        let words = get_query_words("this is a very simple title");
        for bad in ["is", "a", "the", "this"] {
            assert!(
                !words.contains(&bad.to_string()),
                "word {:?} must not appear in {:?}",
                bad,
                words
            );
        }
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

    #[test]
    fn test_strip_title_decorations() {
        // Prefixes
        assert_eq!(
            strip_title_decorations("Extended Abstract: HotStuff-2: Optimal Two-Phase Responsive BFT."),
            "HotStuff-2: Optimal Two-Phase Responsive BFT"
        );
        assert_eq!(
            strip_title_decorations("Brief Announcement: Byzantine Agreement"),
            "Byzantine Agreement"
        );
        assert_eq!(
            strip_title_decorations("Invited Paper: Some Talk"),
            "Some Talk"
        );
        // Bracket suffixes
        assert_eq!(
            strip_title_decorations("LTE-advanced: next-generation wireless broadband technology [Invited Paper]."),
            "LTE-advanced: next-generation wireless broadband technology"
        );
        // Paren suffix
        assert_eq!(
            strip_title_decorations("A Note on Efficient Zero-Knowledge Proofs and Arguments (Extended Abstract)"),
            "A Note on Efficient Zero-Knowledge Proofs and Arguments"
        );
        // ", correction." suffix
        assert_eq!(
            strip_title_decorations("The square lattice shuffle, correction."),
            "The square lattice shuffle"
        );
        // No-op on plain titles
        assert_eq!(
            strip_title_decorations("Attention is All You Need"),
            "Attention is All You Need"
        );
        // Case-insensitive prefix match
        assert_eq!(
            strip_title_decorations("EXTENDED ABSTRACT: X"),
            "X"
        );
    }

    /// Populate a DB with a target paper plus enough noise to push it out of the
    /// first-50 insertion-order window. Verifies that the BM25-ranked FTS query
    /// (`ORDER BY f.rank`) still surfaces the target when a common query word
    /// ("need") would otherwise drown it.
    fn setup_db_with_rank_noise() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();

        let keidar = insert_or_get_author(&conn, "Idit Keidar").unwrap();
        // Insert noise BEFORE the target so insertion-order LIMIT 50 would miss it.
        for i in 0..80 {
            insert_or_get_publication(
                &conn,
                &format!("noise/n{}", i),
                &format!("We Need Something Different Number {}", i),
            )
            .unwrap();
        }
        let target = insert_or_get_publication(
            &conn,
            "conf/podc/KeidarKNS21",
            "All You Need is DAG.",
        )
        .unwrap();

        let mut batch = InsertBatch::new();
        batch.publication_authors.push((target, keidar));
        insert_batch(&conn, &batch).unwrap();
        rebuild_fts_index(&conn).unwrap();

        conn
    }

    #[test]
    fn test_query_fts_rank_orders_by_bm25() {
        // Regression test for BUG #1: without ORDER BY rank, a common query
        // word like "need" (which now pairs with the DAG acronym thanks to
        // BUG #2 fix) would still miss the target if insertion-order LIMIT 50
        // preceded the rank change. With both fixes, the target must be found.
        let conn = setup_db_with_rank_noise();
        let result = query_fts(&conn, "All You Need is DAG", DEFAULT_THRESHOLD).unwrap();
        assert!(result.is_some(), "BM25 ranking should surface target");
        let result = result.unwrap();
        assert_eq!(result.record.title, "All You Need is DAG.");
    }

    #[test]
    fn test_query_fts_matches_despite_extended_abstract_prefix() {
        // Regression test for BUG #4: a citation omitting "Extended Abstract:"
        // should still match a DBLP record that carries the decoration.
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        let malkhi = insert_or_get_author(&conn, "Dahlia Malkhi").unwrap();
        let target = insert_or_get_publication(
            &conn,
            "journals/iacr/MalkhiN23",
            "Extended Abstract: HotStuff-2: Optimal Two-Phase Responsive BFT.",
        )
        .unwrap();
        let mut batch = InsertBatch::new();
        batch.publication_authors.push((target, malkhi));
        insert_batch(&conn, &batch).unwrap();
        rebuild_fts_index(&conn).unwrap();

        let result = query_fts(
            &conn,
            "Hotstuff-2: Optimal two-phase responsive bft",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(
            result.is_some(),
            "citation without Extended Abstract prefix must still match"
        );
    }

    #[test]
    fn test_query_fts_returns_match_with_empty_authors() {
        // Regression test for BUG #3: DBLP legitimately stores authorless
        // records for handbook chapters, anonymised entries, etc. The query
        // layer must surface them; the orchestrator's skip_author_check
        // handles the downstream title-only verification.
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        insert_or_get_publication(
            &conn,
            "books/sp/voecking2011/Blomer11",
            "How to Share a Secret.",
        )
        .unwrap();
        rebuild_fts_index(&conn).unwrap();

        let result = query_fts(&conn, "How to share a secret", DEFAULT_THRESHOLD).unwrap();
        assert!(
            result.is_some(),
            "authorless DBLP records must still be returned"
        );
        let qr = result.unwrap();
        assert!(qr.record.authors.is_empty());
        assert!(qr.score >= DEFAULT_THRESHOLD);
    }

    #[test]
    fn test_query_fts_matches_despite_invited_paper_suffix() {
        // Regression test for BUG #4: " [Invited Paper]" trailer.
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        let ghosh = insert_or_get_author(&conn, "Amitava Ghosh").unwrap();
        let target = insert_or_get_publication(
            &conn,
            "journals/wc/GhoshRMMT10",
            "LTE-advanced: next-generation wireless broadband technology [Invited Paper].",
        )
        .unwrap();
        let mut batch = InsertBatch::new();
        batch.publication_authors.push((target, ghosh));
        insert_batch(&conn, &batch).unwrap();
        rebuild_fts_index(&conn).unwrap();

        let result = query_fts(
            &conn,
            "Lte-advanced: next-generation wireless broadband technology",
            DEFAULT_THRESHOLD,
        )
        .unwrap();
        assert!(
            result.is_some(),
            "citation without [Invited Paper] suffix must still match"
        );
    }
}
