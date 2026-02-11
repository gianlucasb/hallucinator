//! SQLite database operations for ACL Anthology indexing.

use rusqlite::{params, Connection};

use crate::AclError;

/// Initialize the database with the required schema.
pub fn init_database(conn: &Connection) -> Result<(), AclError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS publications (
            id INTEGER PRIMARY KEY,
            anthology_id TEXT UNIQUE,
            title TEXT NOT NULL,
            url TEXT,
            doi TEXT
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
        "#,
    )?;

    Ok(())
}

/// Configure pragmas for fast bulk loading.
pub fn begin_bulk_load(conn: &Connection) -> Result<(), AclError> {
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
pub fn end_bulk_load(conn: &Connection) -> Result<(), AclError> {
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_pub_authors_pub ON publication_authors(pub_id);
        CREATE INDEX IF NOT EXISTS idx_pub_authors_author ON publication_authors(author_id);
        "#,
    )?;
    Ok(())
}

/// Batch of data to insert into the database.
#[derive(Default)]
pub struct InsertBatch {
    /// (anthology_id, title, url, doi)
    pub publications: Vec<(String, String, Option<String>, Option<String>)>,
    /// author names (deduped via UPSERT)
    pub authors: Vec<String>,
    /// (anthology_id, author_name, position)
    pub publication_authors: Vec<(String, String, usize)>,
}

impl InsertBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.publications.is_empty()
    }

    pub fn len(&self) -> usize {
        self.publications.len()
    }

    pub fn clear(&mut self) {
        self.publications.clear();
        self.authors.clear();
        self.publication_authors.clear();
    }
}

/// Insert a batch of data into the database.
pub fn insert_batch(conn: &Connection, batch: &InsertBatch) -> Result<(), AclError> {
    let tx = conn.unchecked_transaction()?;

    // Insert authors (UPSERT on name)
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO authors (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        )?;
        for name in &batch.authors {
            stmt.execute(params![name])?;
        }
    }

    // Insert publications (UPSERT on anthology_id)
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO publications (anthology_id, title, url, doi) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(anthology_id) DO UPDATE SET title = excluded.title, url = excluded.url, doi = excluded.doi",
        )?;
        for (anthology_id, title, url, doi) in &batch.publications {
            stmt.execute(params![anthology_id, title, url, doi])?;
        }
    }

    // Insert publication-author relationships
    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO publication_authors (pub_id, author_id, position) \
             SELECT p.id, a.id, ?3 FROM publications p, authors a \
             WHERE p.anthology_id = ?1 AND a.name = ?2",
        )?;
        for (anthology_id, author_name, position) in &batch.publication_authors {
            stmt.execute(params![anthology_id, author_name, *position as i64])?;
        }
    }

    tx.commit()?;
    Ok(())
}

/// Rebuild the FTS5 index from the publications table.
pub fn rebuild_fts_index(conn: &Connection) -> Result<(), AclError> {
    conn.execute(
        "INSERT INTO publications_fts(publications_fts) VALUES('rebuild')",
        [],
    )?;
    Ok(())
}

/// Get a metadata value by key.
pub fn get_metadata(conn: &Connection, key: &str) -> Result<Option<String>, AclError> {
    let mut stmt = conn.prepare_cached("SELECT value FROM metadata WHERE key = ?1")?;
    let result = stmt.query_row(params![key], |row| row.get(0)).ok();
    Ok(result)
}

/// Set a metadata value (upsert).
pub fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<(), AclError> {
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Get counts of publications, authors, and relations.
pub fn get_counts(conn: &Connection) -> Result<(i64, i64, i64), AclError> {
    let pubs: i64 = conn.query_row("SELECT COUNT(*) FROM publications", [], |row| row.get(0))?;
    let authors: i64 = conn.query_row("SELECT COUNT(*) FROM authors", [], |row| row.get(0))?;
    let relations: i64 = conn.query_row("SELECT COUNT(*) FROM publication_authors", [], |row| {
        row.get(0)
    })?;
    Ok((pubs, authors, relations))
}

/// Get author names for a publication by anthology_id.
pub fn get_authors_for_publication(
    conn: &Connection,
    anthology_id: &str,
) -> Result<Vec<String>, AclError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.name FROM authors a \
         JOIN publication_authors pa ON a.id = pa.author_id \
         JOIN publications p ON p.id = pa.pub_id \
         WHERE p.anthology_id = ?1 \
         ORDER BY pa.position",
    )?;
    let authors = stmt
        .query_map(params![anthology_id], |row| row.get::<_, String>(0))?
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
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM publications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_insert_and_query_batch() {
        let conn = setup_db();
        let mut batch = InsertBatch::new();
        batch.authors.push("Alice Smith".to_string());
        batch.publications.push((
            "2024.acl-1".to_string(),
            "Test Paper Title".to_string(),
            Some("https://aclanthology.org/2024.acl-1".to_string()),
            None,
        ));
        batch
            .publication_authors
            .push(("2024.acl-1".to_string(), "Alice Smith".to_string(), 0));

        insert_batch(&conn, &batch).unwrap();

        let (pubs, authors, rels) = get_counts(&conn).unwrap();
        assert_eq!(pubs, 1);
        assert_eq!(authors, 1);
        assert_eq!(rels, 1);
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
        batch.authors.push("Alice".to_string());
        batch.authors.push("Bob".to_string());
        batch
            .publications
            .push(("2024.acl-1".to_string(), "Paper".to_string(), None, None));
        batch
            .publication_authors
            .push(("2024.acl-1".to_string(), "Alice".to_string(), 0));
        batch
            .publication_authors
            .push(("2024.acl-1".to_string(), "Bob".to_string(), 1));
        insert_batch(&conn, &batch).unwrap();

        let authors = get_authors_for_publication(&conn, "2024.acl-1").unwrap();
        assert_eq!(authors, vec!["Alice", "Bob"]);
    }
}
