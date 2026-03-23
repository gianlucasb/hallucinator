//! `RecordSink` implementation for SQLite with FTS5.

use std::collections::HashMap;

use hallucinator_common::{ParsedRecord, RecordSink};
use rusqlite::Connection;

use crate::db;
use crate::DblpError;

/// A `RecordSink` that writes parsed records into a SQLite database with FTS5.
pub struct SqliteSink<'a> {
    conn: &'a Connection,
    author_cache: HashMap<String, i64>,
    records_inserted: u64,
}

impl<'a> SqliteSink<'a> {
    /// Create a new sink wrapping an initialized SQLite connection.
    ///
    /// The connection should already have the schema initialized via `db::init_database()`
    /// and bulk load pragmas set via `db::begin_bulk_load()`.
    /// The caller must have started a transaction with `BEGIN`.
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            author_cache: HashMap::new(),
            records_inserted: 0,
        }
    }

    /// Number of records inserted so far.
    pub fn records_inserted(&self) -> u64 {
        self.records_inserted
    }
}

impl RecordSink for SqliteSink<'_> {
    type Error = DblpError;

    fn insert_batch(&mut self, records: &[ParsedRecord]) -> Result<(), DblpError> {
        for record in records {
            // Resolve author IDs (insert-or-get + cache)
            let mut author_id_list = Vec::with_capacity(record.authors.len());
            for author in &record.authors {
                let aid = if let Some(&cached) = self.author_cache.get(author) {
                    cached
                } else {
                    let id = db::insert_or_get_author(self.conn, author)?;
                    self.author_cache.insert(author.clone(), id);
                    id
                };
                author_id_list.push(aid);
            }

            // Resolve publication ID
            let pub_id =
                db::insert_or_get_publication(self.conn, &record.source_id, &record.title)?;

            // Insert publication_authors
            let mut pa_stmt = self.conn.prepare_cached(
                "INSERT OR IGNORE INTO publication_authors (pub_id, author_id) VALUES (?1, ?2)",
            )?;
            for aid in author_id_list {
                pa_stmt.execute(rusqlite::params![pub_id, aid])?;
            }

            self.records_inserted += 1;
        }

        Ok(())
    }

    fn finalize(&mut self) -> Result<(), DblpError> {
        db::rebuild_fts_index(self.conn)?;
        Ok(())
    }
}
