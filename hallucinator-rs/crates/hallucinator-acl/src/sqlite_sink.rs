//! `RecordSink` implementation for ACL Anthology SQLite database with FTS5.

use hallucinator_common::{ParsedRecord, RecordSink};
use rusqlite::Connection;

use crate::db;
use crate::AclError;

/// A `RecordSink` that writes parsed records into the ACL Anthology SQLite database.
pub struct SqliteSink<'a> {
    conn: &'a Connection,
    records_inserted: u64,
}

impl<'a> SqliteSink<'a> {
    /// Create a new sink wrapping an initialized SQLite connection.
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            records_inserted: 0,
        }
    }

    /// Number of records inserted so far.
    pub fn records_inserted(&self) -> u64 {
        self.records_inserted
    }
}

impl RecordSink for SqliteSink<'_> {
    type Error = AclError;

    fn insert_batch(&mut self, records: &[ParsedRecord]) -> Result<(), AclError> {
        let mut batch = db::InsertBatch::new();

        for record in records {
            for (i, author) in record.authors.iter().enumerate() {
                batch.authors.push(author.clone());
                batch.publication_authors.push((
                    record.source_id.clone(),
                    author.clone(),
                    i,
                ));
            }
            batch.publications.push((
                record.source_id.clone(),
                record.title.clone(),
                record.url.clone(),
                record.doi.clone(),
            ));
        }

        db::insert_batch(self.conn, &batch)?;
        self.records_inserted += records.len() as u64;
        Ok(())
    }

    fn finalize(&mut self) -> Result<(), AclError> {
        db::rebuild_fts_index(self.conn)?;
        db::end_bulk_load(self.conn)?;
        Ok(())
    }
}
