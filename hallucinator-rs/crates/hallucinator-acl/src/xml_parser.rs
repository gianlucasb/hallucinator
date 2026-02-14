//! Parser for ACL Anthology XML files.
//!
//! Each XML file follows the structure:
//! ```xml
//! <collection id="2024.acl">
//!   <volume id="long" type="proceedings">
//!     <paper id="1">
//!       <title>Some <fixed-case>BERT</fixed-case> Paper</title>
//!       <author><first>Alice</first><last>Smith</last></author>
//!       <doi>10.18653/v1/2024.acl-long.1</doi>
//!       <url>2024.acl-long.1</url>
//!     </paper>
//!   </volume>
//! </collection>
//! ```

use quick_xml::Reader;
use quick_xml::events::Event;
use std::io::BufRead;

/// A parsed paper record from an ACL Anthology XML file.
#[derive(Debug, Clone)]
pub struct AclPaper {
    pub anthology_id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub doi: Option<String>,
    pub url: Option<String>,
}

/// Parse an ACL Anthology XML file, calling `on_paper` for each paper found.
pub fn parse_xml<R: BufRead>(reader: R, mut on_paper: impl FnMut(AclPaper)) {
    let mut xml_reader = Reader::from_reader(reader);
    xml_reader.config_mut().trim_text(true);

    let mut buf = Vec::new();

    // Track collection/volume IDs for constructing anthology_id
    let mut collection_id = String::new();
    let mut volume_id = String::new();

    // Current paper state
    let mut in_paper = false;
    let mut is_frontmatter = false;
    let mut paper_id = String::new();
    let mut title_text = String::new();
    let mut authors: Vec<String> = Vec::new();
    let mut first_name = String::new();
    let mut last_name = String::new();
    let mut doi_text = String::new();
    let mut url_text = String::new();

    // Nesting tracking
    let mut in_title = false;
    let mut in_author = false;
    let mut in_first = false;
    let mut in_last = false;
    let mut in_doi = false;
    let mut in_url = false;
    let mut title_depth: u32 = 0;

    loop {
        match xml_reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag.as_str() {
                    "collection" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"id" {
                                collection_id = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    "volume" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"id" {
                                volume_id = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    "paper" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"id" {
                                paper_id = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                        // Check for frontmatter (id="0")
                        is_frontmatter = paper_id == "0";
                        in_paper = true;
                        title_text.clear();
                        authors.clear();
                        doi_text.clear();
                        url_text.clear();
                    }
                    "title" if in_paper => {
                        in_title = true;
                        title_depth = 1;
                        title_text.clear();
                    }
                    // Inline markup within titles: <fixed-case>, <tex-math>, <i>, <b>
                    "fixed-case" | "tex-math" | "i" | "b" if in_title => {
                        title_depth += 1;
                    }
                    "author" if in_paper => {
                        in_author = true;
                        first_name.clear();
                        last_name.clear();
                    }
                    "first" if in_author => {
                        in_first = true;
                    }
                    "last" if in_author => {
                        in_last = true;
                    }
                    "doi" if in_paper => {
                        in_doi = true;
                        doi_text.clear();
                    }
                    "url" if in_paper => {
                        in_url = true;
                        url_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                // Handle self-closing tags (shouldn't normally appear for paper elements)
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "collection" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"id" {
                            collection_id = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_title {
                    title_text.push_str(&e.unescape().unwrap_or_default());
                } else if in_first {
                    first_name.push_str(&e.unescape().unwrap_or_default());
                } else if in_last {
                    last_name.push_str(&e.unescape().unwrap_or_default());
                } else if in_doi {
                    doi_text.push_str(&e.unescape().unwrap_or_default());
                } else if in_url {
                    url_text.push_str(&e.unescape().unwrap_or_default());
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag.as_str() {
                    "title" if in_title && title_depth == 1 => {
                        in_title = false;
                        title_depth = 0;
                    }
                    "fixed-case" | "tex-math" | "i" | "b" if in_title => {
                        title_depth = title_depth.saturating_sub(1);
                    }
                    "first" => {
                        in_first = false;
                    }
                    "last" => {
                        in_last = false;
                    }
                    "author" if in_author => {
                        in_author = false;
                        let name = if !first_name.is_empty() && !last_name.is_empty() {
                            format!("{} {}", first_name.trim(), last_name.trim())
                        } else if !last_name.is_empty() {
                            last_name.trim().to_string()
                        } else {
                            first_name.trim().to_string()
                        };
                        if !name.is_empty() {
                            authors.push(name);
                        }
                    }
                    "doi" => {
                        in_doi = false;
                    }
                    "url" => {
                        in_url = false;
                    }
                    "paper" if in_paper => {
                        in_paper = false;

                        // Skip frontmatter
                        if is_frontmatter {
                            continue;
                        }

                        let title = title_text.trim().to_string();
                        if title.is_empty() {
                            continue;
                        }

                        // Construct anthology ID: e.g. "2024.acl-long.1"
                        let anthology_id = if !url_text.is_empty() {
                            url_text.trim().to_string()
                        } else {
                            format!("{}-{}.{}", collection_id, volume_id, paper_id)
                        };

                        // Construct URL
                        let url = format!("https://aclanthology.org/{}", anthology_id);

                        let doi = if doi_text.is_empty() {
                            None
                        } else {
                            Some(doi_text.trim().to_string())
                        };

                        on_paper(AclPaper {
                            anthology_id,
                            title,
                            authors: authors.clone(),
                            doi,
                            url: Some(url),
                        });
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_parse_basic_paper() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<collection id="2024.acl">
  <volume id="long" type="proceedings">
    <paper id="1">
      <title>Attention Patterns in <fixed-case>BERT</fixed-case> Models</title>
      <author><first>Alice</first><last>Smith</last></author>
      <author><first>Bob</first><last>Jones</last></author>
      <doi>10.18653/v1/2024.acl-long.1</doi>
      <url>2024.acl-long.1</url>
    </paper>
  </volume>
</collection>"#;

        let mut papers = Vec::new();
        parse_xml(Cursor::new(xml), |paper| papers.push(paper));

        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].anthology_id, "2024.acl-long.1");
        assert_eq!(papers[0].title, "Attention Patterns in BERT Models");
        assert_eq!(papers[0].authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(
            papers[0].doi,
            Some("10.18653/v1/2024.acl-long.1".to_string())
        );
        assert_eq!(
            papers[0].url,
            Some("https://aclanthology.org/2024.acl-long.1".to_string())
        );
    }

    #[test]
    fn test_skip_frontmatter() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<collection id="2024.acl">
  <volume id="long">
    <paper id="0">
      <title>Proceedings of ACL 2024</title>
    </paper>
    <paper id="1">
      <title>Real Paper Title</title>
      <author><first>Alice</first><last>Smith</last></author>
      <url>2024.acl-long.1</url>
    </paper>
  </volume>
</collection>"#;

        let mut papers = Vec::new();
        parse_xml(Cursor::new(xml), |paper| papers.push(paper));

        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].title, "Real Paper Title");
    }

    #[test]
    fn test_multiple_papers() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<collection id="2023.emnlp">
  <volume id="main">
    <paper id="1">
      <title>First Paper</title>
      <author><first>Alice</first><last>Smith</last></author>
      <url>2023.emnlp-main.1</url>
    </paper>
    <paper id="2">
      <title>Second Paper</title>
      <author><first>Bob</first><last>Jones</last></author>
      <url>2023.emnlp-main.2</url>
    </paper>
  </volume>
</collection>"#;

        let mut papers = Vec::new();
        parse_xml(Cursor::new(xml), |paper| papers.push(paper));

        assert_eq!(papers.len(), 2);
        assert_eq!(papers[0].title, "First Paper");
        assert_eq!(papers[1].title, "Second Paper");
    }

    #[test]
    fn test_nested_markup_in_title() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<collection id="2024.acl">
  <volume id="long">
    <paper id="1">
      <title>Understanding <tex-math>x^2</tex-math> in <i>Deep</i> <fixed-case>NLP</fixed-case></title>
      <url>2024.acl-long.1</url>
    </paper>
  </volume>
</collection>"#;

        let mut papers = Vec::new();
        parse_xml(Cursor::new(xml), |paper| papers.push(paper));

        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].title, "Understanding x^2 in Deep NLP");
    }
}
