//! XML parser for DBLP XML dump format.
//!
//! Parses the DBLP XML dump (`dblp.xml.gz`) using SAX-style event processing,
//! extracting publications with their titles, authors, and URLs.

use std::io::BufRead;

use quick_xml::Reader;
use quick_xml::events::Event;

/// A publication extracted from the DBLP XML dump.
#[derive(Debug)]
pub struct Publication {
    /// DBLP key (e.g., "conf/nips/VaswaniSPUJGKP17")
    pub key: String,
    /// Publication title
    pub title: String,
    /// Author names
    pub authors: Vec<String>,
    /// First electronic edition URL (from `<ee>` element)
    pub url: Option<String>,
}

/// DBLP XML element types that represent publications.
const PUB_ELEMENTS: &[&[u8]] = &[
    b"article",
    b"inproceedings",
    b"proceedings",
    b"book",
    b"incollection",
    b"phdthesis",
    b"mastersthesis",
];

fn is_pub_element(name: &[u8]) -> bool {
    PUB_ELEMENTS.contains(&name)
}

/// Which field we're currently reading text for.
enum Field {
    Title,
    Author,
    Url,
}

impl Field {
    fn element_name(&self) -> &[u8] {
        match self {
            Field::Title => b"title",
            Field::Author => b"author",
            Field::Url => b"ee",
        }
    }
}

/// Parse a DBLP XML dump, calling `on_pub` for each publication found.
///
/// Handles the DBLP DTD structure where publication elements (article,
/// inproceedings, etc.) contain `<title>`, `<author>`/`<editor>`, and `<ee>`
/// child elements. Title elements may contain inline formatting sub-elements
/// (`<i>`, `<sub>`, `<sup>`, `<tt>`) whose text content is accumulated.
pub fn parse_xml<R: BufRead>(reader: R, mut on_pub: impl FnMut(Publication)) {
    let mut xml = Reader::from_reader(reader);
    xml.config_mut().trim_text(false);

    let mut buf = Vec::with_capacity(4096);

    // State
    let mut in_pub = false;
    let mut current_key = String::new();
    let mut current_title = String::new();
    let mut current_authors: Vec<String> = Vec::new();
    let mut current_url: Option<String> = None;
    let mut reading: Option<Field> = None;
    let mut text_buf = String::new();
    // Track if we're reading an <editor> (treat same as author)
    let mut reading_editor = false;

    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let name_ref = name.as_ref();

                if is_pub_element(name_ref) {
                    in_pub = true;
                    current_key.clear();
                    current_title.clear();
                    current_authors.clear();
                    current_url = None;
                    reading = None;
                    reading_editor = false;

                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"key" {
                            current_key = String::from_utf8_lossy(&attr.value).into_owned();
                        }
                    }
                } else if in_pub && reading.is_none() {
                    match name_ref {
                        b"title" => {
                            reading = Some(Field::Title);
                            text_buf.clear();
                        }
                        b"author" => {
                            reading = Some(Field::Author);
                            reading_editor = false;
                            text_buf.clear();
                        }
                        b"editor" => {
                            reading = Some(Field::Author);
                            reading_editor = true;
                            text_buf.clear();
                        }
                        b"ee" if current_url.is_none() => {
                            reading = Some(Field::Url);
                            text_buf.clear();
                        }
                        _ => {}
                    }
                }
                // Sub-elements (e.g. <i> inside <title>): continue accumulating
            }

            Ok(Event::Text(ref e)) => {
                if reading.is_some()
                    && let Ok(text) = e.unescape()
                {
                    text_buf.push_str(&text);
                }
            }

            Ok(Event::CData(ref e)) => {
                if reading.is_some() {
                    let text = String::from_utf8_lossy(e.as_ref());
                    text_buf.push_str(&text);
                }
            }

            Ok(Event::End(ref e)) => {
                let name = e.name();
                let name_ref = name.as_ref();

                // Check if we're finishing a field we were reading
                if let Some(ref field) = reading {
                    let target = if reading_editor {
                        b"editor" as &[u8]
                    } else {
                        field.element_name()
                    };

                    if name_ref == target {
                        let text = text_buf.trim().to_string();
                        match reading.take() {
                            Some(Field::Title) => {
                                current_title = text;
                            }
                            Some(Field::Author) => {
                                if !text.is_empty() {
                                    current_authors.push(text);
                                }
                            }
                            Some(Field::Url) => {
                                if !text.is_empty() {
                                    current_url = Some(text);
                                }
                            }
                            None => {}
                        }
                        reading_editor = false;
                    }
                } else if in_pub && is_pub_element(name_ref) {
                    // End of publication element — emit if we got a title
                    if !current_title.is_empty() {
                        on_pub(Publication {
                            key: std::mem::take(&mut current_key),
                            title: std::mem::take(&mut current_title),
                            authors: std::mem::take(&mut current_authors),
                            url: current_url.take(),
                        });
                    }
                    in_pub = false;
                }
            }

            Ok(Event::Eof) => break,
            Err(_) => {
                // Skip malformed elements, continue parsing
                continue;
            }
            _ => {}
        }

        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_article() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<article key="journals/cacm/Knuth74" mdate="2020-01-01">
  <author>Donald E. Knuth</author>
  <title>Computer Programming as an Art.</title>
  <journal>Commun. ACM</journal>
  <year>1974</year>
  <ee>https://doi.org/10.1145/361604.361612</ee>
</article>
</dblp>"#;

        let mut pubs = Vec::new();
        parse_xml(xml.as_bytes(), |p| pubs.push(p));

        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].key, "journals/cacm/Knuth74");
        assert_eq!(pubs[0].title, "Computer Programming as an Art.");
        assert_eq!(pubs[0].authors, vec!["Donald E. Knuth"]);
        assert_eq!(
            pubs[0].url.as_deref(),
            Some("https://doi.org/10.1145/361604.361612")
        );
    }

    #[test]
    fn test_parse_inproceedings_multiple_authors() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<inproceedings key="conf/nips/VaswaniSPUJGKP17" mdate="2023-03-30">
  <author>Ashish Vaswani</author>
  <author>Noam Shazeer</author>
  <author>Niki Parmar</author>
  <title>Attention is All you Need.</title>
  <year>2017</year>
  <ee>https://proceedings.neurips.cc/paper/2017/hash/abc</ee>
  <ee>https://arxiv.org/abs/1706.03762</ee>
</inproceedings>
</dblp>"#;

        let mut pubs = Vec::new();
        parse_xml(xml.as_bytes(), |p| pubs.push(p));

        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].authors.len(), 3);
        // Only first <ee> is captured
        assert!(pubs[0].url.as_ref().unwrap().contains("neurips"));
    }

    #[test]
    fn test_parse_title_with_formatting() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<article key="test/1">
  <title>On the <i>k</i>-SAT Problem.</title>
  <author>Test Author</author>
</article>
</dblp>"#;

        let mut pubs = Vec::new();
        parse_xml(xml.as_bytes(), |p| pubs.push(p));

        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].title, "On the k-SAT Problem.");
    }

    #[test]
    fn test_parse_proceedings_with_editors() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<proceedings key="conf/test/2024">
  <editor>Alice Smith</editor>
  <editor>Bob Jones</editor>
  <title>Proceedings of Test Conference 2024.</title>
</proceedings>
</dblp>"#;

        let mut pubs = Vec::new();
        parse_xml(xml.as_bytes(), |p| pubs.push(p));

        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].authors, vec!["Alice Smith", "Bob Jones"]);
    }

    #[test]
    fn test_skip_www_entries() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<www key="homepages/k/Knuth">
  <author>Donald E. Knuth</author>
  <title>Home Page</title>
</www>
<article key="test/1">
  <author>Test</author>
  <title>Real Paper</title>
</article>
</dblp>"#;

        let mut pubs = Vec::new();
        parse_xml(xml.as_bytes(), |p| pubs.push(p));

        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].title, "Real Paper");
    }

    #[test]
    fn test_skip_titleless_entries() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<article key="test/notitle">
  <author>No Title Author</author>
</article>
<article key="test/withtitle">
  <author>Has Title</author>
  <title>A Real Title</title>
</article>
</dblp>"#;

        let mut pubs = Vec::new();
        parse_xml(xml.as_bytes(), |p| pubs.push(p));

        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].key, "test/withtitle");
    }

    #[test]
    fn test_parse_multiple_types() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<article key="a"><title>Article</title><author>A</author></article>
<inproceedings key="b"><title>Inproc</title><author>B</author></inproceedings>
<book key="c"><title>Book</title><author>C</author></book>
<phdthesis key="d"><title>Thesis</title><author>D</author></phdthesis>
</dblp>"#;

        let mut pubs = Vec::new();
        parse_xml(xml.as_bytes(), |p| pubs.push(p));

        assert_eq!(pubs.len(), 4);
    }
}
