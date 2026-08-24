//! Import pipelines that turn an external source (a downloaded conference
//! program page, or this tool's own marked-safe report JSON) into corpus
//! records, deduped and inserted.

pub mod aaai;
pub mod acsac;
pub mod asiaccs;
pub mod asplos;
pub(crate) mod author_parsing;
pub mod blackhat;
pub mod ccs;
pub mod cvf;
pub mod defcon;
pub(crate) mod dom_text;
pub mod dsn;
pub mod esorics;
pub mod eurosp;
pub mod eurosys;
pub mod iclr;
pub mod icml;
pub mod icse;
pub mod ieee_sp;
pub mod imc;
pub mod infocom;
pub mod kdd;
pub mod ndss;
pub mod neurips;
pub mod pets;
pub mod raid;
pub mod report_json;
pub mod sigcomm;
pub mod sigmod;
pub mod sosp;
pub mod usenix;
pub mod www;

use std::path::PathBuf;

use rusqlite::Connection;

use crate::CorpusError;
use crate::db::NewPublication;
use crate::insert::{self, InsertOutcome};

/// Where to read source HTML from for a fetch-based import.
#[derive(Debug, Clone)]
pub enum HtmlSource {
    /// Fetch live from this URL.
    Url(String),
    /// Read a page already saved to disk (e.g. downloaded via a real
    /// browser when the live site blocks automated requests).
    File(PathBuf),
}

/// A realistic browser User-Agent. Some venue sites (USENIX, confirmed
/// during investigation) 403 requests carrying a generic HTTP-client UA;
/// this is the first thing to try before falling back to `HtmlSource::File`.
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Outcome of one import run.
#[derive(Debug, Clone, Default)]
pub struct ImportStats {
    /// Total records the parser produced.
    pub seen: usize,
    pub inserted: usize,
    pub skipped_duplicate: usize,
    pub skipped_no_title: usize,
    /// Records with no author names parsed (e.g. USENIX invited talks and
    /// panels, which share the same page markup as papers but carry no
    /// byline field). Skipped rather than inserted: the `DatabaseBackend`
    /// wrapper requires non-empty authors for a "found" result (same rule
    /// `hallucinator-acl`'s offline backend uses), so an author-less row
    /// could never produce a match — it would just be clutter.
    pub skipped_no_authors: usize,
}

async fn fetch_html(source: &HtmlSource) -> Result<String, CorpusError> {
    match source {
        HtmlSource::File(path) => {
            let bytes = std::fs::read(path).map_err(CorpusError::Io)?;
            Ok(decode_body(bytes))
        }
        HtmlSource::Url(url) => {
            let client = reqwest::Client::builder()
                .user_agent(BROWSER_UA)
                .build()
                .map_err(|e| CorpusError::Fetch(e.to_string()))?;
            let resp = client
                .get(url)
                .send()
                .await
                .map_err(|e| CorpusError::Fetch(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(CorpusError::Fetch(format!(
                    "HTTP {} fetching {url} — the site may be blocking automated \
                     requests; try saving the page from a browser and passing \
                     --from-file instead",
                    resp.status()
                )));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| CorpusError::Fetch(e.to_string()))?;
            Ok(decode_body(bytes.to_vec()))
        }
    }
}

/// Decode a fetched body to a UTF-8 string, transparently gunzipping if
/// the bytes are gzip-compressed.
///
/// Needed for Wayback Machine snapshots specifically: some archived pages
/// (confirmed for USENIX's Drupal-served pages) replay the original
/// gzip-compressed response body verbatim under a `text/html`
/// content-type and *without* a `Content-Encoding: gzip` header — so
/// `reqwest`'s normal transport-level decompression never triggers (it
/// only activates on a declared header, not by sniffing bytes). Detecting
/// the gzip magic number (`1f 8b`) ourselves and decompressing covers
/// that case; anything else is decoded as plain UTF-8 (lossily, so a rare
/// encoding quirk degrades gracefully instead of erroring out the whole
/// import).
fn decode_body(bytes: Vec<u8>) -> String {
    use std::io::Read;

    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut decompressed = String::new();
        if decoder.read_to_string(&mut decompressed).is_ok() {
            return decompressed;
        }
        // Fell through: claimed to be gzip but didn't decompress cleanly
        // (truncated archive, double-encoded, ...) — fall back to lossy
        // decoding of the raw bytes rather than losing the fetch entirely.
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn insert_all(conn: &Connection, records: Vec<NewPublication>) -> Result<ImportStats, CorpusError> {
    let mut stats = ImportStats {
        seen: records.len(),
        ..Default::default()
    };
    for rec in records {
        if rec.title.trim().is_empty() {
            stats.skipped_no_title += 1;
            continue;
        }
        if rec.authors.iter().all(|a| a.trim().is_empty()) {
            stats.skipped_no_authors += 1;
            continue;
        }
        match insert::insert_if_new(conn, rec)? {
            InsertOutcome::Inserted(_) => stats.inserted += 1,
            InsertOutcome::SkippedDuplicate => stats.skipped_duplicate += 1,
        }
    }
    Ok(stats)
}

/// Import an NDSS accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"ndss2026"`.
pub async fn import_ndss(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = ndss::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a USENIX `technical-sessions` program page. `source_tag` is
/// stored as each record's provenance, e.g. `"usenix2026"`.
pub async fn import_usenix(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = usenix::parse_technical_sessions(&html, source_tag);
    insert_all(conn, records)
}

/// Import an IEEE S&P ("Oakland") accepted-papers page. `source_tag` is
/// stored as each record's provenance, e.g. `"ieeesp2026"`.
pub async fn import_ieee_sp(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = ieee_sp::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import an ACM CCS accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"ccs2026"`.
pub async fn import_ccs(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = ccs::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a CVF Open Access (CVPR / ICCV / WACV) `?day=all` paper-listing
/// page. `source_tag` is stored as each record's provenance, e.g.
/// `"cvpr2026"`.
pub async fn import_cvf(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = cvf::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a NeurIPS `papers.nips.cc` year-index page. `source_tag` is
/// stored as each record's provenance, e.g. `"neurips2025"`.
pub async fn import_neurips(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = neurips::parse_year_index(&html, source_tag);
    insert_all(conn, records)
}

/// Import one ICSE (or other researchr.org-hosted) track page's Accepted
/// Papers tab. `source_tag` is stored as each record's provenance, e.g.
/// `"icse2026-research"`.
pub async fn import_icse(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = icse::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import one AAAI OJS issue's table-of-contents page. `source_tag` is
/// stored as each record's provenance, e.g. `"aaai26"`.
pub async fn import_aaai(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = aaai::parse_issue_toc(&html, source_tag);
    insert_all(conn, records)
}

/// Import an ACSAC accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"acsac2025"`.
pub async fn import_acsac(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = acsac::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import an ESORICS accepted-papers page. `source_tag` is stored as
/// each record's provenance, e.g. `"esorics2025"`.
pub async fn import_esorics(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = esorics::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a RAID accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"raid2025"`.
pub async fn import_raid(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = raid::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import one ASIACCS submission-cycle accepted-papers page. `source_tag`
/// is stored as each record's provenance, e.g. `"asiaccs2026-cycle1"`.
pub async fn import_asiaccs(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = asiaccs::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a EuroS&P accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"eurosp2026"`.
pub async fn import_eurosp(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = eurosp::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import one DEF CON year's main-track speakers page. `source_tag` is
/// stored as each record's provenance, e.g. `"defcon33"`.
pub async fn import_defcon(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = defcon::parse_speakers(&html, source_tag);
    insert_all(conn, records)
}

/// Import one Black Hat edition's `sessions.json`. `source_tag` is
/// stored as each record's provenance, e.g. `"blackhatus2025"`.
pub async fn import_blackhat(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    // Despite the name, `fetch_html` just fetches text (with transparent
    // gzip fallback) — this is JSON, not HTML, but the same fetch logic
    // applies unchanged.
    let json = fetch_html(&source).await?;
    let records = blackhat::parse_sessions(&json, source_tag);
    insert_all(conn, records)
}

/// Import a SOSP accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"sosp2025"`.
pub async fn import_sosp(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = sosp::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import an ASPLOS program page. `source_tag` is stored as each
/// record's provenance, e.g. `"asplos2026"`.
pub async fn import_asplos(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = asplos::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import an ISCA program page. `source_tag` is stored as each record's
/// provenance, e.g. `"isca2026"`.
///
/// ISCA's `iscaconf.org` program page uses the exact same
/// `div.paper`/`div.paper-title`/`div.paper-authors` markup as ASPLOS's
/// site — evidently a shared conference-site template across systems/
/// architecture venues — so this reuses [`asplos::parse_accepted_papers`]
/// directly rather than duplicating it, the same way NSDI/OSDI reuse
/// `import_usenix` for their shared USENIX Drupal template.
pub async fn import_isca(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = asplos::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import an ICML PMLR volume page. `source_tag` is stored as each
/// record's provenance, e.g. `"icml2026"`.
pub async fn import_icml(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = icml::parse_volume(&html, source_tag);
    insert_all(conn, records)
}

/// Import a Paper Digest "<Venue> Papers with Code & Data" page — built
/// for ICLR, whose own sources are all unusable (see `iclr` module docs).
/// `source_tag` is stored as each record's provenance, e.g. `"iclr2026"`.
pub async fn import_iclr(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = iclr::parse_papers_with_code(&html, source_tag);
    insert_all(conn, records)
}

/// Import a PETS/PoPETs paper-list page. `source_tag` is stored as each
/// record's provenance, e.g. `"pets2026"`.
pub async fn import_pets(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = pets::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import an IEEE INFOCOM accepted-paper-list page. `source_tag` is
/// stored as each record's provenance, e.g. `"infocom2026"`.
pub async fn import_infocom(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = infocom::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import ACM SIGCOMM's accepted-papers page. `source_tag` is stored as
/// each record's provenance, e.g. `"sigcomm2026"`.
pub async fn import_sigcomm(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = sigcomm::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import ACM IMC's accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"imc2026"`.
pub async fn import_imc(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = imc::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a DSN accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"dsn2026"`.
pub async fn import_dsn(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = dsn::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a EuroSys accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"eurosys2026"`.
pub async fn import_eurosys(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = eurosys::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a SIGMOD accepted-papers page. `source_tag` is stored as each
/// record's provenance, e.g. `"sigmod2026"`.
pub async fn import_sigmod(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = sigmod::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a WWW ("The Web Conference") accepted-papers track page.
/// `source_tag` is stored as each record's provenance, e.g. `"www2026"`.
/// Separate pages exist per track (research/industry/short-papers/...) —
/// run once per page you want indexed.
pub async fn import_www(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = www::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import a KDD accepted-papers page (parsing its embedded JS data, not
/// its HTML). `source_tag` is stored as each record's provenance, e.g.
/// `"kdd2026"`.
pub async fn import_kdd(
    conn: &Connection,
    source: HtmlSource,
    source_tag: &str,
) -> Result<ImportStats, CorpusError> {
    let html = fetch_html(&source).await?;
    let records = kdd::parse_accepted_papers(&html, source_tag);
    insert_all(conn, records)
}

/// Import marked-safe references from one of this tool's own report JSON
/// files (already read into `content`). No network access.
pub fn import_report_json(conn: &Connection, content: &str) -> Result<ImportStats, CorpusError> {
    let records =
        report_json::parse_marked_safe(content).map_err(|e| CorpusError::Parse(e.to_string()))?;
    insert_all(conn, records)
}
