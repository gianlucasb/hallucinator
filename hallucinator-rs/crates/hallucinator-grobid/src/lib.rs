use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;
use thiserror::Error;

use hallucinator_core::{ExtractionResult, Reference, SkipStats};

#[derive(Error, Debug)]
pub enum GrobidError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("XML parse error: {0}")]
    XmlParse(String),
    #[error("no <biblStruct> entries found in GROBID TEI XML")]
    NoBiblStructEntries,
}

/// Extract references from a GROBID TEI XML file.
pub fn extract_references_from_grobid(path: &Path) -> Result<ExtractionResult, GrobidError> {
    let content = std::fs::read_to_string(path)?;
    extract_references_from_grobid_str(&content)
}

/// Parse GROBID TEI XML content from a string.
pub fn extract_references_from_grobid_str(content: &str) -> Result<ExtractionResult, GrobidError> {
    let entries = parse_bibl_structs(content)?;

    if entries.is_empty() {
        return Err(GrobidError::NoBiblStructEntries);
    }

    let mut stats = SkipStats {
        total_raw: entries.len(),
        ..Default::default()
    };

    let mut references = Vec::new();

    for (idx, entry) in entries.iter().enumerate() {
        let raw_citation = build_raw_citation(entry);

        let title = entry.title.as_deref().map(|t| t.trim().to_string());

        // Skip entries without a title
        let title = match title {
            Some(t) if !t.is_empty() && t.split_whitespace().count() >= 4 => Some(t),
            Some(t) if t.is_empty() => {
                stats.no_title += 1;
                references.push(Reference {
                    raw_citation,
                    title: None,
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    urls: vec![],
                    original_number: idx + 1,
                    skip_reason: Some("no_title".to_string()),
                });
                continue;
            }
            Some(t) => {
                // Short title (<4 words)
                stats.short_title += 1;
                references.push(Reference {
                    raw_citation,
                    title: Some(t),
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    urls: vec![],
                    original_number: idx + 1,
                    skip_reason: Some("short_title".to_string()),
                });
                continue;
            }
            None => {
                stats.no_title += 1;
                references.push(Reference {
                    raw_citation,
                    title: None,
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    urls: vec![],
                    original_number: idx + 1,
                    skip_reason: Some("no_title".to_string()),
                });
                continue;
            }
        };

        if entry.authors.is_empty() {
            stats.no_authors += 1;
        }

        // Normalize DOI and arXiv ID using core utilities
        let doi = entry
            .doi
            .as_deref()
            .and_then(hallucinator_core::extract_doi);
        let arxiv_id = entry.arxiv_id.as_deref().and_then(|raw| {
            // GROBID may store bare IDs like "1706.03762" — prepend "arXiv:" so
            // extract_arxiv_id can recognize the format.
            hallucinator_core::extract_arxiv_id(raw)
                .or_else(|| hallucinator_core::extract_arxiv_id(&format!("arXiv:{}", raw)))
        });

        references.push(Reference {
            raw_citation,
            title,
            authors: entry.authors.clone(),
            doi,
            arxiv_id,
            urls: entry.urls.clone(),
            original_number: idx + 1,
            skip_reason: None,
        });
    }

    Ok(ExtractionResult {
        references,
        skip_stats: stats,
    })
}

/// A single parsed `<biblStruct>` entry.
#[derive(Debug, Default)]
struct BiblEntry {
    title: Option<String>,
    authors: Vec<String>,
    doi: Option<String>,
    arxiv_id: Option<String>,
    urls: Vec<String>,
    venue: Option<String>,
}

fn build_raw_citation(entry: &BiblEntry) -> String {
    let mut parts = Vec::new();
    if !entry.authors.is_empty() {
        parts.push(entry.authors.join(", "));
    }
    if let Some(ref t) = entry.title {
        parts.push(format!("\"{}\"", t));
    }
    if let Some(ref v) = entry.venue {
        parts.push(v.clone());
    }
    if parts.is_empty() {
        "(untitled)".to_string()
    } else {
        parts.join(". ")
    }
}

/// Parse all `<biblStruct>` entries from GROBID TEI XML.
fn parse_bibl_structs(xml: &str) -> Result<Vec<BiblEntry>, GrobidError> {
    let mut reader = Reader::from_str(xml);

    let mut entries: Vec<BiblEntry> = Vec::new();
    let mut current: Option<BiblEntry> = None;

    // Nesting context flags
    let mut in_analytic = false;
    let mut in_monogr = false;
    let mut in_author = false;
    let mut in_persname = false;

    // Title capture: we want <title level="a" type="main"> from analytic,
    // or <title level="m"> from monogr as fallback.
    let mut in_main_title = false;
    let mut in_monogr_title = false;

    // Author name parts
    let mut in_forename = false;
    let mut in_surname = false;
    let mut forenames: Vec<String> = Vec::new();
    let mut surname = String::new();

    // idno capture
    let mut in_idno_doi = false;
    let mut in_idno_arxiv = false;

    // ref type="url" capture
    let mut in_ref_url = false;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"biblStruct" => {
                        current = Some(BiblEntry::default());
                        in_analytic = false;
                        in_monogr = false;
                    }
                    b"analytic" if current.is_some() => {
                        in_analytic = true;
                    }
                    b"monogr" if current.is_some() => {
                        in_monogr = true;
                    }
                    b"title" if current.is_some() => {
                        // Check attributes for level and type
                        let mut level = None;
                        let mut title_type = None;
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"level" => {
                                    level = Some(String::from_utf8_lossy(&attr.value).to_string());
                                }
                                b"type" => {
                                    title_type =
                                        Some(String::from_utf8_lossy(&attr.value).to_string());
                                }
                                _ => {}
                            }
                        }

                        if in_analytic
                            && level.as_deref() == Some("a")
                            && title_type.as_deref() == Some("main")
                        {
                            in_main_title = true;
                        } else if in_monogr && level.as_deref() == Some("m") {
                            in_monogr_title = true;
                        } else if in_monogr && level.as_deref() == Some("j") {
                            // Journal title — use as venue
                            in_monogr_title = true;
                        }
                    }
                    b"author" if current.is_some() && (in_analytic || in_monogr) => {
                        in_author = true;
                        forenames.clear();
                        surname.clear();
                    }
                    b"persName" if in_author => {
                        in_persname = true;
                    }
                    b"forename" if in_persname => {
                        in_forename = true;
                    }
                    b"surname" if in_persname => {
                        in_surname = true;
                    }
                    b"idno" if current.is_some() => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"type" {
                                let val = String::from_utf8_lossy(&attr.value).to_lowercase();
                                match val.as_str() {
                                    "doi" => in_idno_doi = true,
                                    "arxiv" => in_idno_arxiv = true,
                                    _ => {}
                                }
                            }
                        }
                    }
                    b"ptr" if current.is_some() => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"target" {
                                let url = String::from_utf8_lossy(&attr.value).trim().to_string();
                                if !url.is_empty()
                                    && let Some(ref mut entry) = current
                                {
                                    entry.urls.push(url);
                                }
                            }
                        }
                    }
                    b"ref" if current.is_some() => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"type" {
                                let val = String::from_utf8_lossy(&attr.value);
                                if val == "url" {
                                    in_ref_url = true;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                if local.as_ref() == b"ptr" && current.is_some() {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"target" {
                            let url = String::from_utf8_lossy(&attr.value).trim().to_string();
                            if !url.is_empty()
                                && let Some(ref mut entry) = current
                            {
                                entry.urls.push(url);
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default();
                if let Some(ref mut entry) = current {
                    if in_main_title {
                        let existing = entry.title.get_or_insert_with(String::new);
                        existing.push_str(&text);
                    } else if in_monogr_title {
                        // Use monogr title as venue if we already have an analytic title,
                        // otherwise use it as the main title fallback.
                        if entry.title.is_some() {
                            let venue = entry.venue.get_or_insert_with(String::new);
                            venue.push_str(&text);
                        } else {
                            let existing = entry.title.get_or_insert_with(String::new);
                            existing.push_str(&text);
                        }
                    }
                    if in_forename {
                        forenames.push(text.trim().to_string());
                    }
                    if in_surname {
                        surname.push_str(text.trim());
                    }
                    if in_idno_doi {
                        entry.doi = Some(text.trim().to_string());
                    }
                    if in_idno_arxiv {
                        entry.arxiv_id = Some(text.trim().to_string());
                    }
                    if in_ref_url {
                        let url = text.trim().to_string();
                        if !url.is_empty() {
                            entry.urls.push(url);
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"biblStruct" => {
                        if let Some(entry) = current.take() {
                            entries.push(entry);
                        }
                        in_analytic = false;
                        in_monogr = false;
                    }
                    b"analytic" => in_analytic = false,
                    b"monogr" => in_monogr = false,
                    b"title" => {
                        in_main_title = false;
                        in_monogr_title = false;
                    }
                    b"author" => {
                        // Assemble author name from collected parts
                        if let Some(ref mut entry) = current {
                            let name = assemble_author_name(&forenames, &surname);
                            if !name.is_empty() {
                                entry.authors.push(name);
                            }
                        }
                        in_author = false;
                        in_persname = false;
                        forenames.clear();
                        surname.clear();
                    }
                    b"persName" => in_persname = false,
                    b"forename" => in_forename = false,
                    b"surname" => in_surname = false,
                    b"idno" => {
                        in_idno_doi = false;
                        in_idno_arxiv = false;
                    }
                    b"ref" => in_ref_url = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(GrobidError::XmlParse(format!("{}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(entries)
}

fn assemble_author_name(forenames: &[String], surname: &str) -> String {
    let fore = forenames
        .iter()
        .filter(|f| !f.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let sur = surname.trim();
    match (fore.is_empty(), sur.is_empty()) {
        (true, true) => String::new(),
        (true, false) => sur.to_string(),
        (false, true) => fore,
        (false, false) => format!("{} {}", fore, sur),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <text>
    <back>
      <div type="references">
        <listBibl>
          <biblStruct xml:id="b0">
            <analytic>
              <title level="a" type="main">Attention Is All You Need</title>
              <author>
                <persName><forename type="first">Ashish</forename><surname>Vaswani</surname></persName>
              </author>
              <author>
                <persName><forename type="first">Noam</forename><surname>Shazeer</surname></persName>
              </author>
              <idno type="DOI">10.5555/3295222.3295349</idno>
              <idno type="arXiv">1706.03762</idno>
            </analytic>
            <monogr>
              <title level="j">Advances in Neural Information Processing Systems</title>
              <imprint><date type="published" when="2017"/></imprint>
            </monogr>
          </biblStruct>
          <biblStruct xml:id="b1">
            <analytic>
              <title level="a" type="main">BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding</title>
              <author>
                <persName><forename type="first">Jacob</forename><surname>Devlin</surname></persName>
              </author>
            </analytic>
            <monogr>
              <title level="m">Proceedings of NAACL</title>
              <imprint><date type="published" when="2019"/></imprint>
            </monogr>
            <ptr type="open-access" target="https://arxiv.org/abs/1810.04805"/>
          </biblStruct>
        </listBibl>
      </div>
    </back>
  </text>
</TEI>"#;

    #[test]
    fn test_basic_parsing() {
        let result = extract_references_from_grobid_str(BASIC_XML).unwrap();
        assert_eq!(result.references.len(), 2);
        assert_eq!(result.skip_stats.total_raw, 2);

        let r0 = &result.references[0];
        assert_eq!(r0.title.as_deref(), Some("Attention Is All You Need"));
        assert_eq!(r0.authors, vec!["Ashish Vaswani", "Noam Shazeer"]);
        assert_eq!(r0.doi.as_deref(), Some("10.5555/3295222.3295349"));
        assert!(r0.arxiv_id.is_some());
        assert_eq!(r0.original_number, 1);
        assert!(r0.skip_reason.is_none());

        let r1 = &result.references[1];
        assert_eq!(
            r1.title.as_deref(),
            Some(
                "BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding"
            )
        );
        assert_eq!(r1.authors, vec!["Jacob Devlin"]);
        assert!(
            r1.urls
                .contains(&"https://arxiv.org/abs/1810.04805".to_string())
        );
        assert_eq!(r1.original_number, 2);
    }

    #[test]
    fn test_short_title_skipped() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <text><back><div type="references"><listBibl>
    <biblStruct>
      <analytic>
        <title level="a" type="main">Short Title</title>
        <author><persName><surname>Smith</surname></persName></author>
      </analytic>
      <monogr><imprint><date/></imprint></monogr>
    </biblStruct>
  </listBibl></div></back></text>
</TEI>"#;

        let result = extract_references_from_grobid_str(xml).unwrap();
        assert_eq!(result.references.len(), 1);
        assert_eq!(result.skip_stats.short_title, 1);
        assert_eq!(
            result.references[0].skip_reason.as_deref(),
            Some("short_title")
        );
    }

    #[test]
    fn test_no_title_skipped() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <text><back><div type="references"><listBibl>
    <biblStruct>
      <analytic>
        <author><persName><surname>Doe</surname></persName></author>
      </analytic>
      <monogr><imprint><date/></imprint></monogr>
    </biblStruct>
  </listBibl></div></back></text>
</TEI>"#;

        let result = extract_references_from_grobid_str(xml).unwrap();
        assert_eq!(result.references.len(), 1);
        assert_eq!(result.skip_stats.no_title, 1);
        assert_eq!(
            result.references[0].skip_reason.as_deref(),
            Some("no_title")
        );
    }

    #[test]
    fn test_monograph_title_fallback() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <text><back><div type="references"><listBibl>
    <biblStruct>
      <monogr>
        <title level="m">Introduction to Algorithms Third Edition</title>
        <author><persName><forename type="first">Thomas</forename><forename type="middle">H</forename><surname>Cormen</surname></persName></author>
        <imprint><date type="published" when="2009"/></imprint>
      </monogr>
    </biblStruct>
  </listBibl></div></back></text>
</TEI>"#;

        let result = extract_references_from_grobid_str(xml).unwrap();
        assert_eq!(result.references.len(), 1);
        let r = &result.references[0];
        assert_eq!(
            r.title.as_deref(),
            Some("Introduction to Algorithms Third Edition")
        );
        assert_eq!(r.authors, vec!["Thomas H Cormen"]);
        assert!(r.skip_reason.is_none());
    }

    #[test]
    fn test_no_entries_error() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <text><back><div type="references"><listBibl>
  </listBibl></div></back></text>
</TEI>"#;

        let result = extract_references_from_grobid_str(xml);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GrobidError::NoBiblStructEntries
        ));
    }

    #[test]
    fn test_ref_url_extraction() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <text><back><div type="references"><listBibl>
    <biblStruct>
      <analytic>
        <title level="a" type="main">A Paper With URLs and References Inside</title>
        <author><persName><surname>Author</surname></persName></author>
      </analytic>
      <monogr><imprint><date/></imprint></monogr>
      <ref type="url">https://example.com/paper</ref>
    </biblStruct>
  </listBibl></div></back></text>
</TEI>"#;

        let result = extract_references_from_grobid_str(xml).unwrap();
        let r = &result.references[0];
        assert!(r.urls.contains(&"https://example.com/paper".to_string()));
    }

    #[test]
    fn test_no_authors_tracked() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <text><back><div type="references"><listBibl>
    <biblStruct>
      <analytic>
        <title level="a" type="main">A Paper Without Any Authors Listed</title>
      </analytic>
      <monogr><imprint><date/></imprint></monogr>
    </biblStruct>
  </listBibl></div></back></text>
</TEI>"#;

        let result = extract_references_from_grobid_str(xml).unwrap();
        assert_eq!(result.skip_stats.no_authors, 1);
        assert!(result.references[0].authors.is_empty());
        // Still included (not skipped), just no authors
        assert!(result.references[0].skip_reason.is_none());
    }
}
