//! JSONL ingester for the Kaggle `Cornell-University/arxiv` dump.
//!
//! Reads `arxiv-metadata-oai-snapshot.json` (one JSON record per line,
//! ~2.5M lines / ~4 GB) and upserts each record into the offline
//! SQLite database. Replaced the older OAI-PMH harvester — Kaggle's
//! snapshot is a full weekly dump and is far more reliable than
//! OAI-PMH for cold-start bulk builds.

use std::io::BufRead;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::{ArxivError, ArxivRecord, ArxivVersion};

/// Progress events emitted during an ingest run.
#[derive(Debug, Clone)]
pub enum IngestProgress {
    /// First record successfully parsed — ingestion is live.
    Started,
    /// Periodic record count update. Frequency is controlled by the
    /// `progress_every` argument to [`ingest_jsonl`].
    Progress { records_parsed: u64 },
    /// All records consumed. `total` may be less than the number of
    /// input lines when some records were skipped (deleted / malformed).
    Complete { total: u64, elapsed: Duration },
}

/// Subset of the Kaggle record schema we actually persist. Extra
/// fields (`abstract`, `submitter`, `comments`, `journal-ref`,
/// `update_date`, …) are ignored — serde silently drops them.
///
/// The upstream format is documented at
/// <https://www.kaggle.com/datasets/Cornell-University/arxiv>.
#[derive(Debug, Deserialize)]
struct KaggleRecord {
    id: String,
    title: String,
    #[serde(default)]
    categories: Option<String>,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    license: Option<String>,
    /// `authors_parsed` is a list of `[last, first, suffix]` triples.
    /// Cleaner than the free-text `authors` field because it has the
    /// LaTeX stripped.
    #[serde(default)]
    authors_parsed: Vec<Vec<String>>,
    #[serde(default)]
    versions: Vec<KaggleVersion>,
}

#[derive(Debug, Deserialize)]
struct KaggleVersion {
    /// Always shaped like `"v1"`, `"v2"`, … in the dump.
    version: String,
    #[serde(default)]
    created: Option<String>,
}

/// Stream JSONL from `reader`, invoking `sink` once per successfully
/// parsed record and `progress` at milestones. Returns the count of
/// records actually emitted to the sink.
///
/// Malformed individual lines abort the run — partial DBs are worse
/// than failures because a half-built offline index silently returns
/// wrong "not found" answers. Callers that want lenience should wrap
/// `sink` themselves.
pub fn ingest_jsonl<R, F, P>(
    reader: R,
    progress_every: u64,
    mut sink: F,
    mut progress: P,
) -> Result<u64, ArxivError>
where
    R: BufRead,
    F: FnMut(ArxivRecord) -> Result<(), ArxivError>,
    P: FnMut(IngestProgress),
{
    let start = Instant::now();
    let mut count: u64 = 0;
    let mut emitted_start = false;
    let every = progress_every.max(1);

    for (line_idx, line) in reader.lines().enumerate() {
        // Line numbers are 1-based in error messages for human readability.
        let line_no = line_idx as u64 + 1;
        let line = line.map_err(|e| ArxivError::Harvest(format!("read line {line_no}: {e}")))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let krec: KaggleRecord = serde_json::from_str(trimmed)
            .map_err(|e| ArxivError::Harvest(format!("json line {line_no}: {e}")))?;
        let rec = krec.into_arxiv_record();
        sink(rec)?;
        count += 1;
        if !emitted_start {
            emitted_start = true;
            progress(IngestProgress::Started);
        }
        if count.is_multiple_of(every) {
            progress(IngestProgress::Progress {
                records_parsed: count,
            });
        }
    }

    progress(IngestProgress::Complete {
        total: count,
        elapsed: start.elapsed(),
    });
    Ok(count)
}

impl KaggleRecord {
    fn into_arxiv_record(self) -> ArxivRecord {
        let authors: Vec<String> = self
            .authors_parsed
            .into_iter()
            .filter_map(format_author)
            .collect();
        let mut versions: Vec<ArxivVersion> = self
            .versions
            .into_iter()
            .filter_map(parse_version)
            .collect();
        // v1 fallback when the dump omits version history (rare, seen
        // on very old records). Keeps downstream code from having to
        // handle an empty versions list.
        if versions.is_empty() {
            versions.push(ArxivVersion {
                version: 1,
                submitted: None,
            });
        }
        ArxivRecord {
            id: self.id,
            title: normalize_whitespace(&self.title),
            authors,
            categories: self.categories,
            doi: self.doi,
            license: self.license,
            versions,
        }
    }
}

/// Format a Kaggle `authors_parsed` triple `[last, first, suffix]` as
/// a single display string `"first last"` (or `"first last suffix"`
/// when suffix is non-empty). Returns `None` when both last and first
/// are blank.
fn format_author(parts: Vec<String>) -> Option<String> {
    let last = parts.first().map(String::as_str).unwrap_or("").trim();
    let first = parts.get(1).map(String::as_str).unwrap_or("").trim();
    let suffix = parts.get(2).map(String::as_str).unwrap_or("").trim();
    if last.is_empty() && first.is_empty() {
        return None;
    }
    let mut out = String::new();
    if !first.is_empty() {
        out.push_str(first);
    }
    if !last.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(last);
    }
    if !suffix.is_empty() {
        out.push(' ');
        out.push_str(suffix);
    }
    Some(out)
}

fn parse_version(v: KaggleVersion) -> Option<ArxivVersion> {
    let s = v.version.trim_start_matches('v');
    let n: u32 = s.parse().ok()?;
    Some(ArxivVersion {
        version: n,
        submitted: v.created,
    })
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_line() -> &'static str {
        r#"{"id":"0704.0001","submitter":"Pavel Nadolsky","authors":"C. Balázs, E. L. Berger, P. M. Nadolsky, C.-P. Yuan","title":"Calculation of prompt diphoton production cross sections at Tevatron and\n  LHC energies","comments":"37 pages, 15 figures","journal-ref":"Phys.Rev.D76:013009,2007","doi":"10.1103/PhysRevD.76.013009","report-no":"ANL-HEP-PR-07-12","categories":"hep-ph","license":null,"abstract":"  ...","versions":[{"version":"v1","created":"Mon, 2 Apr 2007 19:18:42 GMT"},{"version":"v2","created":"Tue, 24 Jul 2007 20:10:27 GMT"}],"update_date":"2008-11-26","authors_parsed":[["Balázs","C.",""],["Berger","E. L.",""],["Nadolsky","P. M.",""],["Yuan","C.-P.",""]]}"#
    }

    #[test]
    fn parses_one_kaggle_record() {
        let mut recs: Vec<ArxivRecord> = Vec::new();
        let count = ingest_jsonl(
            Cursor::new(sample_line()),
            1000,
            |r| {
                recs.push(r);
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(count, 1);
        let r = &recs[0];
        assert_eq!(r.id, "0704.0001");
        // Line-break in the raw title is normalised away.
        assert_eq!(
            r.title,
            "Calculation of prompt diphoton production cross sections at Tevatron and LHC energies"
        );
        assert_eq!(r.authors.len(), 4);
        assert_eq!(r.authors[0], "C. Balázs");
        assert_eq!(r.authors[3], "C.-P. Yuan");
        assert_eq!(r.doi.as_deref(), Some("10.1103/PhysRevD.76.013009"));
        assert_eq!(r.categories.as_deref(), Some("hep-ph"));
        assert!(r.license.is_none()); // JSON `null` deserialised to None
        assert_eq!(r.versions.len(), 2);
        assert_eq!(r.versions[1].version, 2);
    }

    #[test]
    fn skips_blank_lines() {
        let input = format!("\n\n{line}\n\n", line = sample_line());
        let mut seen: u64 = 0;
        let count = ingest_jsonl(
            Cursor::new(input),
            1000,
            |_r| {
                seen += 1;
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(seen, 1);
    }

    #[test]
    fn malformed_json_aborts_with_line_context() {
        let input = format!("{}\nnot valid json\n", sample_line());
        let err = ingest_jsonl(Cursor::new(input), 1000, |_| Ok(()), |_| {}).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("json line 2"), "got: {msg}");
    }

    #[test]
    fn empty_authors_parsed_is_fine() {
        // Very early records sometimes have no authors_parsed.
        let line = r#"{"id":"hep-th/9901001","title":"Old paper","versions":[{"version":"v1"}]}"#;
        let mut recs: Vec<ArxivRecord> = Vec::new();
        ingest_jsonl(
            Cursor::new(line),
            1000,
            |r| {
                recs.push(r);
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(recs[0].id, "hep-th/9901001");
        assert!(recs[0].authors.is_empty());
        assert_eq!(recs[0].versions.len(), 1);
    }

    #[test]
    fn missing_versions_gets_v1_fallback() {
        let line = r#"{"id":"x","title":"t","authors_parsed":[]}"#;
        let mut recs: Vec<ArxivRecord> = Vec::new();
        ingest_jsonl(
            Cursor::new(line),
            1000,
            |r| {
                recs.push(r);
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(recs[0].versions.len(), 1);
        assert_eq!(recs[0].versions[0].version, 1);
    }

    #[test]
    fn format_author_handles_suffix_and_blanks() {
        assert_eq!(
            format_author(vec!["Balázs".into(), "C.".into(), "".into()]).as_deref(),
            Some("C. Balázs")
        );
        assert_eq!(
            format_author(vec!["Smith".into(), "John".into(), "Jr.".into()]).as_deref(),
            Some("John Smith Jr.")
        );
        assert_eq!(
            format_author(vec!["LastOnly".into()]).as_deref(),
            Some("LastOnly")
        );
        assert_eq!(format_author(vec!["".into(), "".into()]), None);
    }
}
