//! SQLite schema and query helpers for the offline IACR ePrint database.

use rusqlite::{Connection, OptionalExtension, params};

use crate::{IacrError, IacrRecord};

/// Schema version marker stored in the `metadata` table. Bump when the
/// schema changes incompatibly; `open()` has no forward-compat logic
/// yet — a schema bump means `update-iacr-eprint` will need to rebuild
/// from scratch.
pub const SCHEMA_VERSION: &str = "1";

/// Create all tables + FTS5 index on a fresh database.
pub fn create_schema(conn: &Connection) -> Result<(), IacrError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS metadata (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- One row per IACR ePrint paper. `id` is the canonical
        -- `YYYY/N` shape (no leading zeros in N — the archive itself
        -- uses `2022/1` not `2022/0001`). Stored as TEXT to preserve
        -- that and keep the SQL simple — ID ordering inside a year
        -- isn't semantically meaningful anyway.
        CREATE TABLE IF NOT EXISTS papers (
            id         TEXT PRIMARY KEY,
            title      TEXT NOT NULL,
            category   TEXT,
            date       TEXT
        );

        -- One row per author, in listed order.
        CREATE TABLE IF NOT EXISTS authors (
            paper_id  TEXT NOT NULL,
            position  INTEGER NOT NULL,
            name      TEXT NOT NULL,
            PRIMARY KEY (paper_id, position),
            FOREIGN KEY (paper_id) REFERENCES papers(id) ON DELETE CASCADE
        );

        -- FTS5 title index. unicode61 tokenizer matches what DBLP /
        -- arXiv / ACL use (diacritic-insensitive, word-granularity)
        -- so title queries compose cleanly across crates.
        CREATE VIRTUAL TABLE IF NOT EXISTS titles_fts USING fts5(
            paper_id UNINDEXED,
            title,
            tokenize='unicode61 remove_diacritics 2'
        );
        "#,
    )?;
    Ok(())
}

/// Insert or replace one record + author list + FTS entry. Designed
/// to be cheap enough to call once per OAI-PMH page record — the
/// archive is ~25-30k papers so a per-record FTS update is fine
/// without the begin-bulk / rebuild-fts dance arxiv-offline needs
/// for 2M+ rows.
pub fn upsert_record(conn: &Connection, rec: &IacrRecord) -> Result<(), IacrError> {
    conn.execute(
        "INSERT OR REPLACE INTO papers (id, title, category, date) VALUES (?1, ?2, ?3, ?4)",
        params![
            rec.id,
            rec.title,
            rec.category.as_deref(),
            rec.date.as_deref()
        ],
    )?;
    conn.execute("DELETE FROM authors WHERE paper_id = ?1", params![rec.id])?;
    {
        let mut stmt = conn
            .prepare_cached("INSERT INTO authors (paper_id, position, name) VALUES (?1, ?2, ?3)")?;
        for (i, name) in rec.authors.iter().enumerate() {
            stmt.execute(params![rec.id, i as i64, name])?;
        }
    }
    // Delete-then-insert is the standard FTS5 upsert idiom when
    // content isn't tied to a rowid — the `titles_fts` table stores
    // the ID as UNINDEXED so we key the delete by it directly.
    conn.execute(
        "DELETE FROM titles_fts WHERE paper_id = ?1",
        params![rec.id],
    )?;
    conn.execute(
        "INSERT INTO titles_fts (paper_id, title) VALUES (?1, ?2)",
        params![rec.id, rec.title],
    )?;
    Ok(())
}

/// Point lookup by ePrint ID. Returns the full record (title +
/// authors + category + date) or `None`.
pub fn lookup_by_id(conn: &Connection, id: &str) -> Result<Option<IacrRecord>, IacrError> {
    let paper: Option<(String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, title, category, date FROM papers WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((id, title, category, date)) = paper else {
        return Ok(None);
    };
    let authors = load_authors(conn, &id)?;
    Ok(Some(IacrRecord {
        id,
        title,
        authors,
        category,
        date,
    }))
}

/// FTS5 title search: returns up to `limit` records in BM25 rank
/// order. Caller does its own fuzzy title comparison on top of the
/// FTS matches (same pattern as DBLP / ACL / arXiv offline backends).
pub fn search_by_title(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<IacrRecord>, IacrError> {
    let sanitized = sanitize_fts_query(query);
    if sanitized.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT p.id, p.title, p.category, p.date \
         FROM titles_fts fts \
         JOIN papers p ON p.id = fts.paper_id \
         WHERE fts.title MATCH ?1 \
         ORDER BY bm25(titles_fts) \
         LIMIT ?2",
    )?;

    let rows: Vec<(String, String, Option<String>, Option<String>)> = stmt
        .query_map(params![sanitized, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut results = Vec::with_capacity(rows.len());
    for (id, title, category, date) in rows {
        let authors = load_authors(conn, &id)?;
        results.push(IacrRecord {
            id,
            title,
            authors,
            category,
            date,
        });
    }
    Ok(results)
}

/// Get authors for a paper in position order.
fn load_authors(conn: &Connection, paper_id: &str) -> Result<Vec<String>, IacrError> {
    let mut stmt =
        conn.prepare_cached("SELECT name FROM authors WHERE paper_id = ?1 ORDER BY position ASC")?;
    let rows: Vec<String> = stmt
        .query_map(params![paper_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Strip FTS5 query syntax out of a free-form title. Matches the
/// approach used by `hallucinator-arxiv-offline::db::sanitize_fts_query`
/// so the FTS5 tokenizer sees plain word tokens separated by spaces.
/// Keeps alnum, whitespace, and `_`; every other char (including
/// `-`, `"`, `*`, `(`, `)`, `:`, `/`, `?`) becomes a space. Without
/// this, a title like `"It's a Zero-Knowledge Proof of Knowledge"`
/// would cause FTS5 to parse `Zero-Knowledge` as a NOT operator.
pub fn sanitize_fts_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for c in query.chars() {
        if c.is_alphanumeric() || c == ' ' || c == '_' {
            out.push(c);
        } else {
            out.push(' ');
        }
    }
    // Collapse runs of whitespace to avoid empty-token FTS errors.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Record a small key/value for the database — used by the builder
/// to persist the OAI-PMH `from` timestamp so the next run can ask
/// the server for only newer records.
pub fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<(), IacrError> {
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

/// Read a metadata value previously stored via [`set_metadata`].
pub fn get_metadata(conn: &Connection, key: &str) -> Result<Option<String>, IacrError> {
    conn.query_row(
        "SELECT value FROM metadata WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_fts_operators() {
        // Hyphens, quotes, asterisks, colons must all become spaces;
        // inner tokens survive.
        assert_eq!(
            sanitize_fts_query("Zero-Knowledge Proofs of \"Knowledge\" *"),
            "Zero Knowledge Proofs of Knowledge"
        );
    }

    #[test]
    fn sanitize_keeps_underscore_and_alnum() {
        assert_eq!(sanitize_fts_query("SHA_256 rounds 80"), "SHA_256 rounds 80");
    }
}
