//! SQLite schema and query helpers for the offline arXiv metadata database.

use rusqlite::{Connection, OptionalExtension, params};

use crate::{ArxivError, ArxivRecord, ArxivVersion};

/// Schema version marker stored in the `metadata` table. Bump when the
/// schema changes incompatibly; `open()` refuses older versions rather
/// than silently returning wrong results.
pub const SCHEMA_VERSION: &str = "1";

/// Create all tables and FTS5 index on a fresh database.
pub fn create_schema(conn: &Connection) -> Result<(), ArxivError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS metadata (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- One row per arXiv paper. `title` is the latest-version title (what
        -- arXivRaw surfaces in its single <title> element); per-version
        -- titles aren't carried by OAI-PMH, so per-version title-matching
        -- still falls through to the live arXiv API.
        CREATE TABLE IF NOT EXISTS papers (
            arxiv_id    TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            categories  TEXT,
            doi         TEXT,
            license     TEXT,
            latest_v    INTEGER NOT NULL DEFAULT 1
        );

        -- One row per author, in listed order. Keeping each name as a
        -- single TEXT column (vs. forename/surname split) matches
        -- what the online `arXiv` backend returns so the match logic
        -- downstream doesn't need to branch on offline-vs-online.
        CREATE TABLE IF NOT EXISTS authors (
            arxiv_id  TEXT NOT NULL,
            position  INTEGER NOT NULL,
            name      TEXT NOT NULL,
            PRIMARY KEY (arxiv_id, position),
            FOREIGN KEY (arxiv_id) REFERENCES papers(arxiv_id) ON DELETE CASCADE
        );

        -- Per-version submission dates (from arXivRaw <version>). Title
        -- isn't recorded because the OAI feed doesn't include
        -- historical titles, but the dates alone are useful for
        -- staleness / submission-window queries and for future
        -- extensions.
        CREATE TABLE IF NOT EXISTS versions (
            arxiv_id     TEXT NOT NULL,
            version      INTEGER NOT NULL,
            submitted    TEXT,
            PRIMARY KEY (arxiv_id, version),
            FOREIGN KEY (arxiv_id) REFERENCES papers(arxiv_id) ON DELETE CASCADE
        );

        -- FTS5 index for title search. Content-sync'd to `papers` via
        -- trigger, not content-rowid-linked, because arxiv_id is a
        -- string not an integer rowid.
        CREATE VIRTUAL TABLE IF NOT EXISTS titles_fts USING fts5(
            arxiv_id UNINDEXED,
            title,
            tokenize='unicode61 remove_diacritics 2'
        );
        "#,
    )?;
    Ok(())
}

/// Record or update a single arXiv paper. Replaces an existing row for
/// the same `arxiv_id` (used by incremental refreshes when an
/// already-harvested paper got a new version).
///
/// Uses `prepare_cached` for every statement so bulk ingest doesn't
/// re-parse the same SQL 2.5M times — rusqlite caches by SQL string
/// on the connection, so subsequent calls skip the parser.
pub fn upsert_record(conn: &Connection, rec: &ArxivRecord) -> Result<(), ArxivError> {
    {
        let mut stmt = conn.prepare_cached(
            "INSERT OR REPLACE INTO papers (arxiv_id, title, categories, doi, license, latest_v) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        stmt.execute(params![
            rec.id,
            rec.title,
            rec.categories.as_deref(),
            rec.doi.as_deref(),
            rec.license.as_deref(),
            rec.latest_version() as i64,
        ])?;
    }
    // Replace authors and versions. On a fresh ingest the DELETE is a
    // no-op but still cheap (indexed by PK); on a refresh it wipes
    // stale entries before we insert the new list.
    {
        let mut stmt = conn.prepare_cached("DELETE FROM authors WHERE arxiv_id = ?1")?;
        stmt.execute(params![rec.id])?;
    }
    {
        let mut stmt = conn.prepare_cached(
            "INSERT INTO authors (arxiv_id, position, name) VALUES (?1, ?2, ?3)",
        )?;
        for (i, name) in rec.authors.iter().enumerate() {
            stmt.execute(params![rec.id, i as i64, name])?;
        }
    }
    {
        let mut stmt = conn.prepare_cached("DELETE FROM versions WHERE arxiv_id = ?1")?;
        stmt.execute(params![rec.id])?;
    }
    {
        let mut stmt = conn.prepare_cached(
            "INSERT INTO versions (arxiv_id, version, submitted) VALUES (?1, ?2, ?3)",
        )?;
        for v in &rec.versions {
            stmt.execute(params![rec.id, v.version as i64, v.submitted.as_deref()])?;
        }
    }
    // Keep FTS in sync. Delete-then-insert is the standard FTS5
    // pattern when the external key (arxiv_id) might already exist.
    {
        let mut stmt = conn.prepare_cached("DELETE FROM titles_fts WHERE arxiv_id = ?1")?;
        stmt.execute(params![rec.id])?;
    }
    {
        let mut stmt =
            conn.prepare_cached("INSERT INTO titles_fts (arxiv_id, title) VALUES (?1, ?2)")?;
        stmt.execute(params![rec.id, rec.title])?;
    }
    Ok(())
}

/// Fast-path upsert that skips the FTS5 index update. Use during
/// bulk ingest when the caller will rebuild the FTS index once at
/// the end via [`rebuild_fts_from_papers`]. FTS5 is the single
/// slowest part of `upsert_record` (tokenisation + inverted-index
/// maintenance per row), so bypassing it during ingest and
/// rebuilding in bulk is typically 5-10× faster overall.
pub fn upsert_record_no_fts(conn: &Connection, rec: &ArxivRecord) -> Result<(), ArxivError> {
    {
        let mut stmt = conn.prepare_cached(
            "INSERT OR REPLACE INTO papers (arxiv_id, title, categories, doi, license, latest_v) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        stmt.execute(params![
            rec.id,
            rec.title,
            rec.categories.as_deref(),
            rec.doi.as_deref(),
            rec.license.as_deref(),
            rec.latest_version() as i64,
        ])?;
    }
    {
        let mut stmt = conn.prepare_cached("DELETE FROM authors WHERE arxiv_id = ?1")?;
        stmt.execute(params![rec.id])?;
    }
    {
        let mut stmt = conn.prepare_cached(
            "INSERT INTO authors (arxiv_id, position, name) VALUES (?1, ?2, ?3)",
        )?;
        for (i, name) in rec.authors.iter().enumerate() {
            stmt.execute(params![rec.id, i as i64, name])?;
        }
    }
    {
        let mut stmt = conn.prepare_cached("DELETE FROM versions WHERE arxiv_id = ?1")?;
        stmt.execute(params![rec.id])?;
    }
    {
        let mut stmt = conn.prepare_cached(
            "INSERT INTO versions (arxiv_id, version, submitted) VALUES (?1, ?2, ?3)",
        )?;
        for v in &rec.versions {
            stmt.execute(params![rec.id, v.version as i64, v.submitted.as_deref()])?;
        }
    }
    Ok(())
}

/// Truncate the FTS5 index and repopulate it from the `papers` table.
/// Paired with [`upsert_record_no_fts`] for bulk-ingest workloads.
/// A single `INSERT ... SELECT` is dramatically cheaper than 2.5M
/// individual `INSERT`s because FTS5 can batch its internal tree
/// writes and skip the delete-then-insert dance.
pub fn rebuild_fts_from_papers(conn: &Connection) -> Result<(), ArxivError> {
    conn.execute_batch(
        "DELETE FROM titles_fts;\n\
         INSERT INTO titles_fts (arxiv_id, title) SELECT arxiv_id, title FROM papers;",
    )?;
    Ok(())
}

/// Write a metadata key/value.
pub fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<(), ArxivError> {
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

/// Read a metadata key.
pub fn get_metadata(conn: &Connection, key: &str) -> Result<Option<String>, ArxivError> {
    let val: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(val)
}

/// Look up a single paper by canonical arXiv ID (e.g., "2403.00108",
/// with or without a `vN` suffix — stripped before lookup).
pub fn lookup_by_id(conn: &Connection, arxiv_id: &str) -> Result<Option<ArxivRecord>, ArxivError> {
    let bare = strip_version_suffix(arxiv_id);
    let row = conn
        .query_row(
            "SELECT arxiv_id, title, categories, doi, license, latest_v \
             FROM papers WHERE arxiv_id = ?1",
            params![bare],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;

    let Some((id, title, categories, doi, license, _latest_v)) = row else {
        return Ok(None);
    };

    let authors = load_authors(conn, &id)?;
    let versions = load_versions(conn, &id)?;
    Ok(Some(ArxivRecord {
        id,
        title,
        authors,
        categories,
        doi,
        license,
        versions,
    }))
}

/// FTS5 title search. Returns up to `limit` candidate arXiv IDs ranked
/// by BM25 relevance. The caller applies fuzzy title matching /
/// author validation on the returned IDs.
pub fn search_by_title(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<String>, ArxivError> {
    // Sanitize FTS5 query: strip syntax chars that could otherwise be
    // interpreted as operators (double quotes, parentheses, etc.) so
    // arbitrary user titles don't cause parse errors.
    let sanitized = sanitize_fts_query(query);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT arxiv_id FROM titles_fts \
         WHERE titles_fts MATCH ?1 \
         ORDER BY bm25(titles_fts) \
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![sanitized, limit as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_authors(conn: &Connection, arxiv_id: &str) -> Result<Vec<String>, ArxivError> {
    let mut stmt = conn.prepare_cached(
        "SELECT name FROM authors WHERE arxiv_id = ?1 ORDER BY position",
    )?;
    let rows = stmt
        .query_map(params![arxiv_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_versions(conn: &Connection, arxiv_id: &str) -> Result<Vec<ArxivVersion>, ArxivError> {
    let mut stmt = conn.prepare_cached(
        "SELECT version, submitted FROM versions WHERE arxiv_id = ?1 ORDER BY version",
    )?;
    let rows = stmt
        .query_map(params![arxiv_id], |row| {
            Ok(ArxivVersion {
                version: row.get::<_, i64>(0)? as u32,
                submitted: row.get::<_, Option<String>>(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Strip a trailing `vN` version suffix, so `2403.00108v2` → `2403.00108`.
/// Old-format IDs like `hep-th/9901001v1` are handled the same way.
fn strip_version_suffix(arxiv_id: &str) -> &str {
    let bytes = arxiv_id.as_bytes();
    let mut end = bytes.len();
    // Trim trailing digits.
    while end > 0 && bytes[end - 1].is_ascii_digit() {
        end -= 1;
    }
    // If what we peeled off was preceded by 'v', drop the 'v' too.
    if end < bytes.len() && end > 0 && bytes[end - 1] == b'v' {
        &arxiv_id[..end - 1]
    } else {
        arxiv_id
    }
}

/// FTS5 MATCH is very strict about query syntax. Replace anything that
/// could be parsed as an operator with a space, then collapse to a
/// whitespace-separated phrase of bare terms. Not perfect — doesn't
/// support quoted phrases or column filters — but safe for
/// library-caller input.
fn sanitize_fts_query(q: &str) -> String {
    q.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_in_memory() -> Connection {
        let conn = Connection::open_in_memory().expect("open :memory:");
        create_schema(&conn).expect("create schema");
        conn
    }

    #[test]
    fn upsert_and_lookup_roundtrip() {
        let conn = open_in_memory();
        let rec = ArxivRecord {
            id: "2403.00108".into(),
            title: "LoRATK: LoRA Once, Backdoor Everywhere".into(),
            authors: vec!["Hongyi Liu".into(), "Shaochen Zhong".into()],
            categories: Some("cs.CR cs.AI".into()),
            doi: None,
            license: Some("http://creativecommons.org/licenses/by/4.0/".into()),
            versions: vec![
                ArxivVersion {
                    version: 1,
                    submitted: Some("Thu, 29 Feb 2024 …".into()),
                },
                ArxivVersion {
                    version: 2,
                    submitted: Some("Wed, 30 Apr 2025 …".into()),
                },
            ],
        };
        upsert_record(&conn, &rec).unwrap();

        let got = lookup_by_id(&conn, "2403.00108").unwrap().unwrap();
        assert_eq!(got.id, rec.id);
        assert_eq!(got.title, rec.title);
        assert_eq!(got.authors, rec.authors);
        assert_eq!(got.versions.len(), 2);
        assert_eq!(got.versions[1].version, 2);
    }

    #[test]
    fn lookup_strips_version_suffix() {
        // Caller may pass "2403.00108v2" or "2403.00108" — both should
        // hit the same row.
        let conn = open_in_memory();
        let rec = ArxivRecord {
            id: "2403.00108".into(),
            title: "LoRATK".into(),
            authors: vec!["A".into()],
            categories: None,
            doi: None,
            license: None,
            versions: vec![ArxivVersion {
                version: 2,
                submitted: None,
            }],
        };
        upsert_record(&conn, &rec).unwrap();
        assert!(lookup_by_id(&conn, "2403.00108").unwrap().is_some());
        assert!(lookup_by_id(&conn, "2403.00108v1").unwrap().is_some());
        assert!(lookup_by_id(&conn, "2403.00108v99").unwrap().is_some());
    }

    #[test]
    fn upsert_replaces_authors_cleanly() {
        // Re-harvesting a paper with a different author list (reorder
        // or addition) must not leave stale rows behind.
        let conn = open_in_memory();
        let mut rec = ArxivRecord {
            id: "2403.00108".into(),
            title: "T".into(),
            authors: vec!["A".into(), "B".into(), "C".into()],
            categories: None,
            doi: None,
            license: None,
            versions: vec![ArxivVersion {
                version: 1,
                submitted: None,
            }],
        };
        upsert_record(&conn, &rec).unwrap();
        rec.authors = vec!["A".into(), "D".into()];
        upsert_record(&conn, &rec).unwrap();
        let got = lookup_by_id(&conn, "2403.00108").unwrap().unwrap();
        assert_eq!(got.authors, vec!["A".to_string(), "D".to_string()]);
    }

    #[test]
    fn title_search_ranks_exact_match_first() {
        let conn = open_in_memory();
        for (id, title) in [
            ("2101.00001", "Attention Is All You Need"),
            ("2101.00002", "All You Need Is Love"),
            ("2101.00003", "Beyond Attention: Token Mixing"),
        ] {
            upsert_record(
                &conn,
                &ArxivRecord {
                    id: id.into(),
                    title: title.into(),
                    authors: vec!["x".into()],
                    categories: None,
                    doi: None,
                    license: None,
                    versions: vec![],
                },
            )
            .unwrap();
        }
        let hits = search_by_title(&conn, "Attention Is All You Need", 5).unwrap();
        assert_eq!(hits.first().map(String::as_str), Some("2101.00001"));
    }

    #[test]
    fn sanitize_fts_strips_problem_chars() {
        assert_eq!(sanitize_fts_query(r#"hello "world""#), "hello world");
        assert_eq!(sanitize_fts_query("a(b)c+d"), "a b c d");
        assert_eq!(sanitize_fts_query("  foo   bar  "), "foo bar");
    }

    #[test]
    fn strip_version_handles_old_and_new_formats() {
        assert_eq!(strip_version_suffix("2403.00108"), "2403.00108");
        assert_eq!(strip_version_suffix("2403.00108v1"), "2403.00108");
        assert_eq!(strip_version_suffix("2403.00108v42"), "2403.00108");
        assert_eq!(strip_version_suffix("hep-th/9901001"), "hep-th/9901001");
        assert_eq!(strip_version_suffix("hep-th/9901001v3"), "hep-th/9901001");
    }
}
