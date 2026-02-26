use std::collections::{HashMap, HashSet};
use std::path::Path;

use mupdf::{Document, TextPageFlags};

use hallucinator_core::{BackendError, PdfBackend};

/// MuPDF-based implementation of [`PdfBackend`].
///
/// This crate is the sole AGPL island — it isolates the mupdf dependency
/// (which is AGPL-3.0) so that non-PDF code paths do not transitively
/// depend on it.
///
/// Header/footer removal uses two complementary strategies:
///
/// 1. **Repeating-element detection** (primary): text blocks whose vertical
///    mid-point falls at the same position (within 2 pt) on ≥ 50% of pages
///    are treated as running headers or footers and excluded.  This correctly
///    handles journals like PoPETs whose running header sits at ~8% from the
///    top — well beyond a naïve fixed-ratio threshold.
///
/// 2. **Fixed-ratio exclusion** (fallback/safety-net): blocks in the top 4%
///    or bottom 5% of a page are always excluded.  This catches headers/footers
///    in very short documents (< 4 pages) where the repeating-element signal is
///    too weak, and handles conference proceedings where the strip sits at the
///    extreme page edge (e.g. USENIX footer lines).
pub struct MupdfBackend {
    /// Fraction of page height from bottom to exclude as footer (0.0–1.0).
    /// Default 0.05. `None` disables footer exclusion.
    footer_exclusion_ratio: Option<f32>,
    /// Fraction of page height from top to exclude as header (0.0–1.0).
    /// Default 0.04. `None` disables header exclusion.
    header_exclusion_ratio: Option<f32>,
}

impl Default for MupdfBackend {
    fn default() -> Self {
        Self {
            footer_exclusion_ratio: Some(0.05),
            header_exclusion_ratio: Some(0.04),
        }
    }
}

impl MupdfBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the footer exclusion ratio. Pass `0.0` to disable.
    pub fn with_footer_exclusion(mut self, ratio: f32) -> Self {
        self.footer_exclusion_ratio = if ratio > 0.0 { Some(ratio) } else { None };
        self
    }

    /// Set the header exclusion ratio. Pass `0.0` to disable.
    pub fn with_header_exclusion(mut self, ratio: f32) -> Self {
        self.header_exclusion_ratio = if ratio > 0.0 { Some(ratio) } else { None };
        self
    }
}

/// Collected data for one text block, sufficient for both repeating-element
/// detection and text assembly.
struct BlockData {
    /// Quantized vertical mid-point: `(y_mid / 2.0).round() as i32`.
    /// Two pt resolution — tight enough to group a running header that
    /// lands at the same absolute position on every page.
    y_bucket: i32,
    /// Top edge of the block in page coordinates (y increases downward).
    y0: f32,
    /// Bottom edge of the block in page coordinates.
    y1: f32,
    /// Pre-extracted text lines (not yet joined).
    lines: Vec<String>,
}

impl PdfBackend for MupdfBackend {
    fn extract_text(&self, path: &Path) -> Result<String, BackendError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| BackendError::OpenError("invalid path encoding".into()))?;

        let document =
            Document::open(path_str).map_err(|e| BackendError::OpenError(e.to_string()))?;

        // ----------------------------------------------------------------
        // Single MuPDF pass: collect block geometry and text for every page.
        // ----------------------------------------------------------------
        //
        // Alongside the block data we build `bucket_page_count`: for each
        // quantized y-bucket, how many distinct pages contain a block there.
        // Pages that share the same y-bucket are likely running header/footer
        // pages; we'll filter them out below.

        let mut all_pages: Vec<(Vec<BlockData>, f32, f32)> = Vec::new(); // (blocks, page_y0, page_y1)
        let mut bucket_page_count: HashMap<i32, usize> = HashMap::new();

        for page_result in document
            .pages()
            .map_err(|e| BackendError::ExtractionError(e.to_string()))?
        {
            let page = page_result.map_err(|e| BackendError::ExtractionError(e.to_string()))?;

            let page_bounds = page
                .bounds()
                .map_err(|e| BackendError::ExtractionError(e.to_string()))?;

            let text_page = page
                .to_text_page(TextPageFlags::empty())
                .map_err(|e| BackendError::ExtractionError(e.to_string()))?;

            let mut page_blocks: Vec<BlockData> = Vec::new();
            let mut seen_buckets: HashSet<i32> = HashSet::new();

            for block in text_page.blocks() {
                let bb = block.bounds();
                let y_mid = (bb.y0 + bb.y1) / 2.0;
                let y_bucket = (y_mid / 2.0).round() as i32;

                // Count each bucket once per page (not once per block).
                if seen_buckets.insert(y_bucket) {
                    *bucket_page_count.entry(y_bucket).or_insert(0) += 1;
                }

                let lines: Vec<String> = block
                    .lines()
                    .map(|line| {
                        line.chars()
                            .map(|c| c.char().unwrap_or('\u{FFFD}'))
                            .collect()
                    })
                    .collect();

                page_blocks.push(BlockData {
                    y_bucket,
                    y0: bb.y0,
                    y1: bb.y1,
                    lines,
                });
            }

            all_pages.push((page_blocks, page_bounds.y0, page_bounds.y1));
        }

        // ----------------------------------------------------------------
        // Identify repeating-element y-buckets.
        //
        // A bucket is "repeating" if it appears on at least half the pages
        // (minimum 2).  For a 16-page paper this is 8 pages; running headers
        // appear on 15/16 while coincidental repeated section positions appear
        // on ≤ 5.  For short documents the fixed-ratio fallback below takes
        // over instead.
        // ----------------------------------------------------------------

        let total_pages = all_pages.len();
        let repeat_threshold = (total_pages / 2).max(2);

        let repeating_buckets: HashSet<i32> = bucket_page_count
            .into_iter()
            .filter(|(_, count)| *count >= repeat_threshold)
            .map(|(bucket, _)| bucket)
            .collect();

        // ----------------------------------------------------------------
        // Assemble output text, skipping repeating-element blocks and
        // fixed-ratio header/footer regions.
        // ----------------------------------------------------------------

        let mut pages_text = Vec::with_capacity(total_pages);

        for (blocks, page_y0, page_y1) in &all_pages {
            let page_height = page_y1 - page_y0;

            let header_threshold = self
                .header_exclusion_ratio
                .map(|r| page_y0 + page_height * r);
            let footer_threshold = self
                .footer_exclusion_ratio
                .map(|r| page_y1 - page_height * r);

            let mut page_text = String::new();

            for block in blocks {
                // Skip blocks whose y-position repeats across many pages.
                if repeating_buckets.contains(&block.y_bucket) {
                    continue;
                }

                // Skip blocks entirely within the fixed-ratio header region.
                if let Some(threshold) = header_threshold
                    && block.y1 <= threshold {
                        continue;
                    }

                // Skip blocks whose top edge is in the fixed-ratio footer region.
                if let Some(threshold) = footer_threshold
                    && block.y0 >= threshold {
                        continue;
                    }

                for line in &block.lines {
                    page_text.push_str(line);
                    page_text.push('\n');
                }
            }

            pages_text.push(page_text);
        }

        Ok(pages_text.join("\n"))
    }
}
