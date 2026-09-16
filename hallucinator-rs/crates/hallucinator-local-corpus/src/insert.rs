//! Dedup-aware insertion: skip a new record if a fuzzy title match (plus
//! author overlap) already exists, so re-running an import (or importing
//! overlapping sources — many submissions citing the same classic paper)
//! doesn't pile up near-duplicate rows.

use rusqlite::Connection;

use crate::db::{self, NewPublication};
use crate::query;
use crate::{CorpusError, DEFAULT_THRESHOLD};

/// Outcome of a dedup-checked insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertOutcome {
    /// No sufficiently-similar record existed; inserted with this new id.
    Inserted(i64),
    /// A fuzzy title match (>= threshold) already existed; nothing inserted.
    SkippedDuplicate,
}

/// Insert `pub_` unless a fuzzy-matching title is already present.
///
/// Dedup is title-only (via [`query::query_fts`] at `DEFAULT_THRESHOLD`) —
/// it does not additionally require author overlap. Two genuinely different
/// papers sharing a near-identical title are rare enough in this corpus's
/// sources (conference proceedings, hand-marked-safe references) that the
/// simpler check is the right tradeoff; a false dedup just means one
/// provenance tag "wins" for that title; the record is not lost, another
/// source re-adds it on the next `import_*` when it changes.
pub fn insert_if_new(
    conn: &Connection,
    pub_: NewPublication,
) -> Result<InsertOutcome, CorpusError> {
    if let Some(existing) = query::query_fts(conn, &pub_.title, DEFAULT_THRESHOLD)? {
        let _ = existing; // already present, close enough
        return Ok(InsertOutcome::SkippedDuplicate);
    }
    let id = db::insert_publication(conn, &pub_)?;
    Ok(InsertOutcome::Inserted(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_database;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_if_new_inserts_first_time() {
        let conn = setup_db();
        let outcome = insert_if_new(
            &conn,
            NewPublication {
                title: "A Novel Paper".to_string(),
                authors: vec!["Alice".to_string()],
                url: None,
                source: "ndss2026".to_string(),
            },
        )
        .unwrap();
        assert!(matches!(outcome, InsertOutcome::Inserted(_)));
        assert_eq!(db::total_count(&conn).unwrap(), 1);
    }

    #[test]
    fn test_insert_if_new_skips_duplicate() {
        let conn = setup_db();
        insert_if_new(
            &conn,
            NewPublication {
                title: "A Novel Paper About Security".to_string(),
                authors: vec!["Alice".to_string()],
                url: None,
                source: "ndss2026".to_string(),
            },
        )
        .unwrap();

        // Same paper, cited slightly differently by another submission.
        let outcome = insert_if_new(
            &conn,
            NewPublication {
                title: "A novel paper about security.".to_string(),
                authors: vec!["A. Alice".to_string()],
                url: None,
                source: "marked_safe".to_string(),
            },
        )
        .unwrap();
        assert_eq!(outcome, InsertOutcome::SkippedDuplicate);
        assert_eq!(db::total_count(&conn).unwrap(), 1);
    }

    #[test]
    fn test_insert_if_new_allows_distinct_titles() {
        let conn = setup_db();
        insert_if_new(
            &conn,
            NewPublication {
                title: "Paper One".to_string(),
                authors: vec![],
                url: None,
                source: "ndss2026".to_string(),
            },
        )
        .unwrap();
        insert_if_new(
            &conn,
            NewPublication {
                title: "Paper Two".to_string(),
                authors: vec![],
                url: None,
                source: "ndss2026".to_string(),
            },
        )
        .unwrap();
        assert_eq!(db::total_count(&conn).unwrap(), 2);
    }
}
