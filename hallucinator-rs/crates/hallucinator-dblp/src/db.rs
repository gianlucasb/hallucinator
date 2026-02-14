//! SQLite database operations for DBLP indexing.

use rusqlite::{Connection, params};

use crate::DblpError;

/// Initialize the database with the required schema.
/// Sets WAL mode and NORMAL synchronous for performance.
pub fn init_database(conn: &Connection) -> Result<(), DblpError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS authors (
            uri TEXT PRIMARY KEY,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS publications (
            id INTEGER PRIMARY KEY,
            uri TEXT UNIQUE NOT NULL,
            title TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS publication_authors (
            pub_uri TEXT NOT NULL,
            author_uri TEXT NOT NULL,
            PRIMARY KEY (pub_uri, author_uri)
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

        CREATE INDEX IF NOT EXISTS idx_pub_authors_pub ON publication_authors(pub_uri);
        CREATE INDEX IF NOT EXISTS idx_pub_authors_author ON publication_authors(author_uri);
        "#,
    )?;

    Ok(())
}

/// Configure pragmas for fast bulk loading.
pub fn begin_bulk_load(conn: &Connection) -> Result<(), DblpError> {
    conn.execute_batch(
        r#"
        PRAGMA temp_store = MEMORY;
        DROP INDEX IF EXISTS idx_pub_authors_pub;
        DROP INDEX IF EXISTS idx_pub_authors_author;
        "#,
    )?;
    Ok(())
}

/// Recreate indexes after bulk loading is complete.
pub fn end_bulk_load(conn: &Connection) -> Result<(), DblpError> {
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_pub_authors_pub ON publication_authors(pub_uri);
        CREATE INDEX IF NOT EXISTS idx_pub_authors_author ON publication_authors(author_uri);
        "#,
    )?;
    Ok(())
}

/// Batch of data to insert into the database.
#[derive(Default)]
pub struct InsertBatch {
    pub authors: Vec<(String, String)>,             // (uri, name)
    pub publications: Vec<(String, String)>,        // (uri, title)
    pub publication_authors: Vec<(String, String)>, // (pub_uri, author_uri)
}

impl InsertBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.authors.is_empty()
            && self.publications.is_empty()
            && self.publication_authors.is_empty()
    }

    pub fn len(&self) -> usize {
        self.authors.len() + self.publications.len() + self.publication_authors.len()
    }

    pub fn clear(&mut self) {
        self.authors.clear();
        self.publications.clear();
        self.publication_authors.clear();
    }
}

/// Insert a batch of data into the database using UPSERT semantics.
pub fn insert_batch(conn: &Connection, batch: &InsertBatch) -> Result<(), DblpError> {
    let tx = conn.unchecked_transaction()?;

    {
        let mut author_stmt = tx.prepare_cached(
            "INSERT INTO authors (uri, name) VALUES (?1, ?2) \
             ON CONFLICT(uri) DO UPDATE SET name = excluded.name",
        )?;
        for (uri, name) in &batch.authors {
            author_stmt.execute(params![uri, name])?;
        }
    }

    {
        let mut pub_stmt = tx.prepare_cached(
            "INSERT INTO publications (uri, title) VALUES (?1, ?2) \
             ON CONFLICT(uri) DO UPDATE SET title = excluded.title",
        )?;
        for (uri, title) in &batch.publications {
            pub_stmt.execute(params![uri, title])?;
        }
    }

    {
        let mut rel_stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO publication_authors (pub_uri, author_uri) VALUES (?1, ?2)",
        )?;
        for (pub_uri, author_uri) in &batch.publication_authors {
            rel_stmt.execute(params![pub_uri, author_uri])?;
        }
    }

    tx.commit()?;
    Ok(())
}

/// Rebuild the FTS5 index from the publications table.
pub fn rebuild_fts_index(conn: &Connection) -> Result<(), DblpError> {
    conn.execute(
        "INSERT INTO publications_fts(publications_fts) VALUES('rebuild')",
        [],
    )?;
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

/// Get author names for a publication URI via JOIN.
pub fn get_authors_for_publication(
    conn: &Connection,
    pub_uri: &str,
) -> Result<Vec<String>, DblpError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.name FROM authors a \
         JOIN publication_authors pa ON a.uri = pa.author_uri \
         WHERE pa.pub_uri = ?1",
    )?;
    let authors = stmt
        .query_map(params![pub_uri], |row| row.get::<_, String>(0))?
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
        let mut batch = InsertBatch::new();
        batch.authors.push(("pid/1".into(), "Alice Smith".into()));
        batch
            .publications
            .push(("rec/1".into(), "Test Paper Title".into()));
        batch
            .publication_authors
            .push(("rec/1".into(), "pid/1".into()));

        insert_batch(&conn, &batch).unwrap();

        let (pubs, authors, rels) = get_counts(&conn).unwrap();
        assert_eq!(pubs, 1);
        assert_eq!(authors, 1);
        assert_eq!(rels, 1);
    }

    #[test]
    fn test_upsert_updates_existing() {
        let conn = setup_db();

        let mut batch = InsertBatch::new();
        batch
            .publications
            .push(("rec/1".into(), "Old Title".into()));
        insert_batch(&conn, &batch).unwrap();

        let mut batch2 = InsertBatch::new();
        batch2
            .publications
            .push(("rec/1".into(), "New Title".into()));
        insert_batch(&conn, &batch2).unwrap();

        let title: String = conn
            .query_row(
                "SELECT title FROM publications WHERE uri = ?1",
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
        let mut batch = InsertBatch::new();
        batch.authors.push(("pid/1".into(), "Alice".into()));
        batch.authors.push(("pid/2".into(), "Bob".into()));
        batch.publications.push(("rec/1".into(), "Paper".into()));
        batch
            .publication_authors
            .push(("rec/1".into(), "pid/1".into()));
        batch
            .publication_authors
            .push(("rec/1".into(), "pid/2".into()));
        insert_batch(&conn, &batch).unwrap();

        let mut authors = get_authors_for_publication(&conn, "rec/1").unwrap();
        authors.sort();
        assert_eq!(authors, vec!["Alice", "Bob"]);
    }

    #[test]
    fn test_fts_rebuild_and_query() {
        let conn = setup_db();
        let mut batch = InsertBatch::new();
        batch
            .publications
            .push(("rec/1".into(), "Attention is All you Need".into()));
        batch
            .publications
            .push(("rec/2".into(), "BERT Pre-training".into()));
        insert_batch(&conn, &batch).unwrap();
        rebuild_fts_index(&conn).unwrap();

        // FTS query
        let mut stmt = conn
            .prepare(
                "SELECT p.uri, p.title FROM publications p \
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
