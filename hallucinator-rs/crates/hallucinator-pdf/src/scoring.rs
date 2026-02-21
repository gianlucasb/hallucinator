//! Scoring functions for segmentation strategy selection.
//!
//! This module provides a scoring-based approach to select the best
//! segmentation strategy from multiple candidates. Rather than using
//! a waterfall approach, all strategies are run and scored based on
//! quality metrics.

use crate::authors::extract_authors_from_reference_with_config;
use crate::config::PdfParsingConfig;
use crate::section::SegmentationResult;
use crate::title::extract_title_from_reference_with_config;

/// Weights for the scoring function.
///
/// Each weight controls the relative importance of a quality metric:
/// - `coverage`: How much of the reference section text is captured
/// - `completeness`: Fraction of refs with extractable title + authors
/// - `consistency`: Inverse of length variation (more uniform = better)
/// - `specificity`: Strategy-specific score (explicit markers > heuristics)
/// - `count`: Number of references found (more = better, up to a point)
#[derive(Debug, Clone)]
pub struct ScoringWeights {
    pub coverage: f64,
    pub completeness: f64,
    pub consistency: f64,
    pub specificity: f64,
    pub count: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            coverage: 0.15,
            completeness: 0.30,
            consistency: 0.10,
            specificity: 0.25,
            count: 0.20,
        }
    }
}

/// Score a segmentation result based on quality metrics.
///
/// Returns a score in the range [0, 1] where higher is better.
pub fn score_segmentation(
    result: &SegmentationResult,
    ref_section_text: &str,
    config: &PdfParsingConfig,
    weights: &ScoringWeights,
) -> f64 {
    let refs = &result.references;

    if refs.is_empty() {
        return 0.0;
    }

    // 1. Coverage: fraction of text captured
    let total_ref_len: usize = refs.iter().map(|r| r.len()).sum();
    let coverage = (total_ref_len as f64 / ref_section_text.len().max(1) as f64).min(1.0);

    // 2. Completeness: fraction with extractable title + authors
    let complete_count = refs
        .iter()
        .filter(|r| has_extractable_content(r, config))
        .count();
    let completeness = complete_count as f64 / refs.len() as f64;

    // 3. Consistency: inverse coefficient of variation of lengths
    let consistency = 1.0 - coefficient_of_variation(refs.iter().map(|r| r.len()));

    // 4. Specificity: strategy-specific score
    let specificity = result.strategy.specificity_score();

    // 5. Count: normalized reference count (cap at 50)
    let count_score = (refs.len() as f64 / 50.0).min(1.0);

    // 6. Plausibility penalty: if text is long enough to suggest multiple references
    //    but strategy returns very few, apply a penalty.
    //    Typical reference is ~150-300 chars, so expect at least text_len / 300 refs.
    //    This helps prevent fallback from winning when it collapses everything into
    //    one giant "reference".
    let expected_min_refs = (ref_section_text.len() / 300).max(1);
    let plausibility = if refs.len() < expected_min_refs {
        // Heavy penalty for implausibly low count
        refs.len() as f64 / expected_min_refs as f64
    } else {
        1.0
    };

    // Weighted sum with plausibility multiplier
    let base_score = weights.coverage * coverage
        + weights.completeness * completeness
        + weights.consistency * consistency
        + weights.specificity * specificity
        + weights.count * count_score;

    base_score * plausibility
}

/// Check if a raw reference has extractable title and authors.
fn has_extractable_content(raw_ref: &str, config: &PdfParsingConfig) -> bool {
    // Try to extract title
    let (title, _) = extract_title_from_reference_with_config(raw_ref, config);
    let has_title = !title.is_empty()
        && title.split_whitespace().count() >= config.min_title_words;

    // Try to extract authors
    let authors = extract_authors_from_reference_with_config(raw_ref, config);
    let has_authors = !authors.is_empty()
        && !authors.iter().all(|a| a == "__SAME_AS_PREVIOUS__");

    has_title && has_authors
}

/// Coefficient of variation (std dev / mean), clamped to [0, 1].
fn coefficient_of_variation(lengths: impl Iterator<Item = usize>) -> f64 {
    let lengths: Vec<f64> = lengths.map(|l| l as f64).collect();
    if lengths.len() < 2 {
        return 0.0;
    }

    let mean = lengths.iter().sum::<f64>() / lengths.len() as f64;
    if mean == 0.0 {
        return 1.0;
    }

    let variance = lengths
        .iter()
        .map(|l| (l - mean).powi(2))
        .sum::<f64>()
        / lengths.len() as f64;
    let std_dev = variance.sqrt();

    (std_dev / mean).min(1.0)
}

/// Select the best segmentation result by score.
pub fn select_best_segmentation(
    results: Vec<SegmentationResult>,
    ref_section_text: &str,
    config: &PdfParsingConfig,
    weights: &ScoringWeights,
) -> Option<SegmentationResult> {
    results
        .into_iter()
        .map(|r| {
            let score = score_segmentation(&r, ref_section_text, config, weights);
            (r, score)
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(r, _)| r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::section::SegmentationStrategy;

    #[test]
    fn test_coefficient_of_variation_empty() {
        let cv = coefficient_of_variation(std::iter::empty());
        assert_eq!(cv, 0.0);
    }

    #[test]
    fn test_coefficient_of_variation_single() {
        let cv = coefficient_of_variation([100].into_iter());
        assert_eq!(cv, 0.0);
    }

    #[test]
    fn test_coefficient_of_variation_uniform() {
        let cv = coefficient_of_variation([100, 100, 100].into_iter());
        assert!(cv < 0.001, "CV of uniform data should be ~0: {}", cv);
    }

    #[test]
    fn test_coefficient_of_variation_varied() {
        let cv = coefficient_of_variation([50, 100, 150].into_iter());
        assert!(cv > 0.3, "CV of varied data should be > 0.3: {}", cv);
    }

    #[test]
    fn test_scoring_weights_default_sum() {
        let w = ScoringWeights::default();
        let sum = w.coverage + w.completeness + w.consistency + w.specificity + w.count;
        assert!((sum - 1.0).abs() < 0.001, "Weights should sum to 1.0: {}", sum);
    }

    #[test]
    fn test_score_empty_result() {
        let result = SegmentationResult {
            strategy: SegmentationStrategy::Fallback,
            references: vec![],
        };
        let config = PdfParsingConfig::default();
        let weights = ScoringWeights::default();
        let score = score_segmentation(&result, "some text", &config, &weights);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_score_basic() {
        // Create a simple segmentation result
        let result = SegmentationResult {
            strategy: SegmentationStrategy::Ieee,
            references: vec![
                "Smith, J., Jones, A., \"A paper about something\", IEEE, 2023.".to_string(),
                "Doe, J., \"Another paper here\", ACM, 2022.".to_string(),
            ],
        };
        let ref_text = concat!(
            "[1] Smith, J., Jones, A., \"A paper about something\", IEEE, 2023.\n",
            "[2] Doe, J., \"Another paper here\", ACM, 2022."
        );
        let config = PdfParsingConfig::default();
        let weights = ScoringWeights::default();
        let score = score_segmentation(&result, ref_text, &config, &weights);
        assert!(score > 0.0, "Score should be positive: {}", score);
        assert!(score <= 1.0, "Score should be <= 1.0: {}", score);
    }

    #[test]
    fn test_select_best_segmentation() {
        let results = vec![
            SegmentationResult {
                strategy: SegmentationStrategy::Fallback,
                references: vec!["short".to_string()],
            },
            SegmentationResult {
                strategy: SegmentationStrategy::Ieee,
                references: vec![
                    "Smith, J., \"A proper paper title here\", IEEE, 2023.".to_string(),
                    "Jones, A., \"Another paper title\", ACM, 2022.".to_string(),
                ],
            },
        ];
        let ref_text = "Some reference text here";
        let config = PdfParsingConfig::default();
        let weights = ScoringWeights::default();

        let best = select_best_segmentation(results, ref_text, &config, &weights);
        assert!(best.is_some());
        // IEEE should win due to higher specificity and better content
        assert_eq!(best.unwrap().strategy, SegmentationStrategy::Ieee);
    }
}
