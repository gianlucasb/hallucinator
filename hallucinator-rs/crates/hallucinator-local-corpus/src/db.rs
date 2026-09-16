//! SQLite schema and low-level storage operations for the local corpus.
//!
//! Mirrors `hallucinator-acl`'s schema (publications / authors /
//! publication_authors / FTS5 title index), with one addition: a `source`
//! column on `publications` recording provenance (`"ndss2026"`,
//! `"usenix2026"`, `"marked_safe"`, ...) so entries can be audited or pruned
//! by where they came from. Unlike ACL Anthology (which has a stable
//! `anthology_id` to upsert on), records here come from disparate scraped
//! and hand-curated sources with no natural key, so publications are
//! insert-only; callers dedupe via a fuzzy query before inserting (see
//! `insert::insert_if_new`).

use rusqlite::{Connection, params};

use crate::CorpusError;

/// Initialize the database with the required schema.
pub fn init_database(conn: &Connection) -> Result<(), CorpusError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS publications (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            url TEXT,
            source TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS authors (
            id INTEGER PRIMARY KEY,
            name TEXT UNIQUE NOT NULL
        );

        CREATE TABLE IF NOT EXISTS publication_authors (
            pub_id INTEGER NOT NULL,
            author_id INTEGER NOT NULL,
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (pub_id, author_id)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS publications_fts USING fts5(
            title,
            content='publications',
            content_rowid='id'
        );

        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_pub_authors_pub ON publication_authors(pub_id);
        CREATE INDEX IF NOT EXISTS idx_pub_authors_author ON publication_authors(author_id);
        CREATE INDEX IF NOT EXISTS idx_publications_source ON publications(source);
        "#,
    )?;

    Ok(())
}

/// Create a session-local (temp-schema) `fts5vocab` table exposing
/// per-term document frequency for `publications_fts`, so query-time code
/// can pick the most *selective* words for an OR-fallback query instead of
/// arbitrary extraction order. Mirrors `hallucinator_dblp::db::ensure_vocab_table`.
///
/// 'row' mode reports one row per term with a `doc` column (number of rows
/// containing the term at least once). `temp.` keeps this out of the
/// on-disk schema — it's rebuilt per connection, not persisted.
pub fn ensure_vocab_table(conn: &Connection) -> Result<(), CorpusError> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS temp.publications_vocab \
         USING fts5vocab('main', 'publications_fts', 'row');",
    )?;
    Ok(())
}

/// A publication to be inserted, before it has an id.
#[derive(Debug, Clone)]
pub struct NewPublication {
    pub title: String,
    pub authors: Vec<String>,
    pub url: Option<String>,
    /// Provenance tag, e.g. `"ndss2026"`, `"usenix2026"`, `"marked_safe"`.
    pub source: String,
}

/// Insert one publication and its authors. Does NOT dedupe — callers should
/// check `query::query_fts` first (see `insert::insert_if_new`). Runs inside
/// a transaction so a partial failure can't leave orphaned author links.
pub fn insert_publication(conn: &Connection, pub_: &NewPublication) -> Result<i64, CorpusError> {
    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "INSERT INTO publications (title, url, source) VALUES (?1, ?2, ?3)",
        params![pub_.title, pub_.url, pub_.source],
    )?;
    let pub_id = tx.last_insert_rowid();

    tx.execute(
        "INSERT INTO publications_fts(rowid, title) VALUES (?1, ?2)",
        params![pub_id, pub_.title],
    )?;

    {
        let mut insert_author = tx.prepare_cached(
            "INSERT INTO authors (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        )?;
        let mut link_author = tx.prepare_cached(
            "INSERT OR IGNORE INTO publication_authors (pub_id, author_id, position) \
             SELECT ?1, id, ?2 FROM authors WHERE name = ?3",
        )?;
        for (pos, author) in pub_.authors.iter().enumerate() {
            let author = author.trim();
            if author.is_empty() {
                continue;
            }
            insert_author.execute(params![author])?;
            link_author.execute(params![pub_id, pos as i64, author])?;
        }
    }

    tx.commit()?;
    Ok(pub_id)
}

/// Get author names for a publication by internal id, in citation order.
pub fn get_authors_for_publication(
    conn: &Connection,
    pub_id: i64,
) -> Result<Vec<String>, CorpusError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.name FROM authors a \
         JOIN publication_authors pa ON a.id = pa.author_id \
         WHERE pa.pub_id = ?1 \
         ORDER BY pa.position",
    )?;
    let authors = stmt
        .query_map(params![pub_id], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(authors)
}

/// Total publication count, and count for a specific `source` tag.
pub fn count_by_source(conn: &Connection, source: &str) -> Result<i64, CorpusError> {
    conn.query_row(
        "SELECT COUNT(*) FROM publications WHERE source = ?1",
        params![source],
        |row| row.get(0),
    )
    .map_err(CorpusError::from)
}

/// Total publication count across all sources.
pub fn total_count(conn: &Connection) -> Result<i64, CorpusError> {
    conn.query_row("SELECT COUNT(*) FROM publications", [], |row| row.get(0))
        .map_err(CorpusError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        conn
    }

    #[test]
    fn test_init_creates_tables() {
        let conn = setup_db();
        assert_eq!(total_count(&conn).unwrap(), 0);
    }

    #[test]
    fn test_insert_and_get_authors() {
        let conn = setup_db();
        let pub_id = insert_publication(
            &conn,
            &NewPublication {
                title: "A Great Paper".to_string(),
                authors: vec!["Alice Smith".to_string(), "Bob Jones".to_string()],
                url: Some("https://example.org/paper".to_string()),
                source: "ndss2026".to_string(),
            },
        )
        .unwrap();

        let authors = get_authors_for_publication(&conn, pub_id).unwrap();
        assert_eq!(authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(total_count(&conn).unwrap(), 1);
        assert_eq!(count_by_source(&conn, "ndss2026").unwrap(), 1);
        assert_eq!(count_by_source(&conn, "usenix2026").unwrap(), 0);
    }

    #[test]
    fn test_shared_author_across_publications() {
        let conn = setup_db();
        insert_publication(
            &conn,
            &NewPublication {
                title: "Paper One".to_string(),
                authors: vec!["Alice Smith".to_string()],
                url: None,
                source: "marked_safe".to_string(),
            },
        )
        .unwrap();
        insert_publication(
            &conn,
            &NewPublication {
                title: "Paper Two".to_string(),
                authors: vec!["Alice Smith".to_string()],
                url: None,
                source: "marked_safe".to_string(),
            },
        )
        .unwrap();

        // Author row is shared (UNIQUE name), not duplicated.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM authors", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_empty_author_skipped() {
        let conn = setup_db();
        let pub_id = insert_publication(
            &conn,
            &NewPublication {
                title: "Paper".to_string(),
                authors: vec!["  ".to_string(), "Real Author".to_string()],
                url: None,
                source: "marked_safe".to_string(),
            },
        )
        .unwrap();
        let authors = get_authors_for_publication(&conn, pub_id).unwrap();
        assert_eq!(authors, vec!["Real Author"]);
    }
}
