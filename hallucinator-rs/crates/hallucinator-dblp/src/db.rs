//! SQLite database operations for DBLP indexing.

use rusqlite::{Connection, params};

use crate::DblpError;

/// Initialize the database with the required schema.
/// Sets WAL mode and NORMAL synchronous for performance.
pub fn init_database(conn: &Connection) -> Result<(), DblpError> {
    // 8KB pages reduce B-tree depth and I/O overhead for bulk inserts.
    // Must be set before creating any tables.
    conn.pragma_update(None, "page_size", 8192)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS authors (
            id INTEGER PRIMARY KEY,
            name TEXT UNIQUE NOT NULL
        );

        CREATE TABLE IF NOT EXISTS publications (
            id INTEGER PRIMARY KEY,
            key TEXT UNIQUE NOT NULL,
            title TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS publication_authors (
            pub_id INTEGER NOT NULL,
            author_id INTEGER NOT NULL,
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
        "#,
    )?;

    Ok(())
}

/// Configure pragmas for fast bulk loading.
/// Uses `synchronous = OFF` to skip fsync on periodic commits — safe because a
/// crashed build just needs to be re-run from scratch.
pub fn begin_bulk_load(conn: &Connection) -> Result<(), DblpError> {
    conn.execute_batch(
        "PRAGMA synchronous = OFF; \
         PRAGMA temp_store = MEMORY; \
         PRAGMA cache_size = -64000;",  // 64 MB page cache
    )?;
    Ok(())
}

/// Batch of publication_author pairs for test helpers.
#[cfg(test)]
#[derive(Default)]
pub struct InsertBatch {
    pub publication_authors: Vec<(i64, i64)>, // (pub_id, author_id)
}

#[cfg(test)]
impl InsertBatch {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Insert a batch of publication_author pairs (test helper).
#[cfg(test)]
pub fn insert_batch(conn: &Connection, batch: &InsertBatch) -> Result<(), DblpError> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut rel_stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO publication_authors (pub_id, author_id) VALUES (?1, ?2)",
        )?;
        for (pub_id, author_id) in &batch.publication_authors {
            rel_stmt.execute(params![pub_id, author_id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Insert or get an author by name, returning the integer ID.
/// Uses RETURNING clause for a single round-trip instead of INSERT + SELECT.
pub fn insert_or_get_author(conn: &Connection, name: &str) -> Result<i64, DblpError> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO authors (name) VALUES (?1) \
         ON CONFLICT(name) DO UPDATE SET name = name RETURNING id",
    )?;
    let id: i64 = stmt.query_row(params![name], |row| row.get(0))?;
    Ok(id)
}

/// Insert or update a publication by key, returning the integer ID.
/// Uses RETURNING clause for a single round-trip instead of INSERT + SELECT.
pub fn insert_or_get_publication(
    conn: &Connection,
    key: &str,
    title: &str,
) -> Result<i64, DblpError> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO publications (key, title) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET title = excluded.title RETURNING id",
    )?;
    let id: i64 = stmt.query_row(params![key, title], |row| row.get(0))?;
    Ok(id)
}

/// Rebuild the FTS5 index from the publications table.
pub fn rebuild_fts_index(conn: &Connection) -> Result<(), DblpError> {
    conn.execute(
        "INSERT INTO publications_fts(publications_fts) VALUES('rebuild')",
        [],
    )?;
    Ok(())
}

/// Run VACUUM to compact the database file.
pub fn vacuum(conn: &Connection) -> Result<(), DblpError> {
    conn.execute_batch("VACUUM;")?;
    Ok(())
}

/// Get a metadata value by key.
pub fn get_metadata(conn: &Connection, key: &str) -> Result<Option<String>, DblpError> {
    let mut stmt = conn.prepare_cached("SELECT value FROM metadata WHERE key = ?1")?;
    let result = stmt.query_row(params![key], |row| row.get(0)).ok();
    Ok(result)
}

/// Set a metadata value (upsert).
pub fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<(), DblpError> {
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Get counts of publications, authors, and relations.
pub fn get_counts(conn: &Connection) -> Result<(i64, i64, i64), DblpError> {
    let pubs: i64 = conn.query_row("SELECT COUNT(*) FROM publications", [], |row| row.get(0))?;
    let authors: i64 = conn.query_row("SELECT COUNT(*) FROM authors", [], |row| row.get(0))?;
    let relations: i64 = conn.query_row("SELECT COUNT(*) FROM publication_authors", [], |row| {
        row.get(0)
    })?;
    Ok((pubs, authors, relations))
}

/// Get author names for a publication by its integer ID.
pub fn get_authors_for_publication(
    conn: &Connection,
    pub_id: i64,
) -> Result<Vec<String>, DblpError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.name FROM authors a \
         JOIN publication_authors pa ON a.id = pa.author_id \
         WHERE pa.pub_id = ?1",
    )?;
    let authors = stmt
        .query_map(params![pub_id], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(authors)
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
        // Verify tables exist by querying them
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM publications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_insert_and_query_batch() {
        let conn = setup_db();
        let author_id = insert_or_get_author(&conn, "Alice Smith").unwrap();
        let pub_id = insert_or_get_publication(&conn, "rec/1", "Test Paper Title").unwrap();

        let mut batch = InsertBatch::new();
        batch.publication_authors.push((pub_id, author_id));
        insert_batch(&conn, &batch).unwrap();

        let (pubs, authors, rels) = get_counts(&conn).unwrap();
        assert_eq!(pubs, 1);
        assert_eq!(authors, 1);
        assert_eq!(rels, 1);
    }

    #[test]
    fn test_upsert_updates_existing() {
        let conn = setup_db();

        insert_or_get_publication(&conn, "rec/1", "Old Title").unwrap();
        insert_or_get_publication(&conn, "rec/1", "New Title").unwrap();

        let title: String = conn
            .query_row(
                "SELECT title FROM publications WHERE key = ?1",
                params!["rec/1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "New Title");

        let (pubs, _, _) = get_counts(&conn).unwrap();
        assert_eq!(pubs, 1); // Still just one record
    }

    #[test]
    fn test_metadata() {
        let conn = setup_db();
        assert_eq!(get_metadata(&conn, "foo").unwrap(), None);

        set_metadata(&conn, "foo", "bar").unwrap();
        assert_eq!(get_metadata(&conn, "foo").unwrap(), Some("bar".into()));

        set_metadata(&conn, "foo", "baz").unwrap();
        assert_eq!(get_metadata(&conn, "foo").unwrap(), Some("baz".into()));
    }

    #[test]
    fn test_get_authors_for_publication() {
        let conn = setup_db();
        let alice_id = insert_or_get_author(&conn, "Alice").unwrap();
        let bob_id = insert_or_get_author(&conn, "Bob").unwrap();
        let pub_id = insert_or_get_publication(&conn, "rec/1", "Paper").unwrap();

        let mut batch = InsertBatch::new();
        batch.publication_authors.push((pub_id, alice_id));
        batch.publication_authors.push((pub_id, bob_id));
        insert_batch(&conn, &batch).unwrap();

        let mut authors = get_authors_for_publication(&conn, pub_id).unwrap();
        authors.sort();
        assert_eq!(authors, vec!["Alice", "Bob"]);
    }

    #[test]
    fn test_insert_or_get_author_deduplicates() {
        let conn = setup_db();
        let id1 = insert_or_get_author(&conn, "Alice").unwrap();
        let id2 = insert_or_get_author(&conn, "Alice").unwrap();
        assert_eq!(id1, id2);

        let (_, authors, _) = get_counts(&conn).unwrap();
        assert_eq!(authors, 1);
    }

    #[test]
    fn test_fts_rebuild_and_query() {
        let conn = setup_db();
        insert_or_get_publication(&conn, "rec/1", "Attention is All you Need").unwrap();
        insert_or_get_publication(&conn, "rec/2", "BERT Pre-training").unwrap();
        rebuild_fts_index(&conn).unwrap();

        // FTS query
        let mut stmt = conn
            .prepare(
                "SELECT p.key, p.title FROM publications p \
                 WHERE p.id IN (SELECT rowid FROM publications_fts WHERE title MATCH ?1)",
            )
            .unwrap();
        let results: Vec<(String, String)> = stmt
            .query_map(params!["attention"], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "rec/1");
    }
}
