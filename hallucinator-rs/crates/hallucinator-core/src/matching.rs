//! Title normalization and fuzzy matching.
//!
//! Delegates to `hallucinator_common::fuzzy` for the canonical implementations.
//! This module re-exports the functions so that existing callers within
//! hallucinator-core don't need to change.

pub use hallucinator_common::fuzzy::normalize_title;
pub use hallucinator_common::fuzzy::titles_match;

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Basic normalization
    // =========================================================================

    #[test]
    fn test_normalize_title_basic() {
        assert_eq!(normalize_title("Hello, World! 123"), "helloworld123");
    }

    #[test]
    fn test_normalize_title_html_entities() {
        assert_eq!(normalize_title("Foo &amp; Bar"), "foobar");
    }

    #[test]
    fn test_normalize_title_unicode() {
        assert_eq!(normalize_title("résumé"), "resume");
    }

    #[test]
    fn test_titles_match_exact() {
        assert!(titles_match(
            "Detecting Hallucinated References",
            "Detecting Hallucinated References"
        ));
    }

    #[test]
    fn test_titles_match_minor_difference() {
        assert!(titles_match(
            "Detecting Hallucinated References in Academic Papers",
            "Detecting Hallucinated References in Academic Paper"
        ));
    }

    #[test]
    fn test_titles_no_match() {
        assert!(!titles_match(
            "Detecting Hallucinated References",
            "Completely Different Title About Cats"
        ));
    }

    #[test]
    fn test_titles_match_empty() {
        assert!(!titles_match("", "Something"));
        assert!(!titles_match("Something", ""));
    }

    // =========================================================================
    // Greek letter transliteration
    // =========================================================================

    #[test]
    fn test_greek_epsilon() {
        assert_eq!(
            normalize_title("εpsolute: Efficiently querying databases"),
            "epsilonpsoluteefficientlyqueryingdatabases"
        );
    }

    #[test]
    fn test_greek_alpha() {
        assert_eq!(
            normalize_title("αdiff: Cross-version binary code similarity"),
            "alphadiffcrossversionbinarycodesimilarity"
        );
    }

    #[test]
    fn test_greek_tau() {
        assert_eq!(
            normalize_title("τCFI: Type-assisted Control Flow Integrity"),
            "taucfitypeassistedcontrolflowintegrity"
        );
    }

    #[test]
    fn test_greek_phi() {
        assert_eq!(
            normalize_title("Prooφ: A zkp market mechanism"),
            "proophiazkpmarketmechanism"
        );
    }

    #[test]
    fn test_greek_mixed() {
        assert_eq!(
            normalize_title("oφoς: Forward secure searchable encryption"),
            "ophiosigmaforwardsecuresearchableencryption"
        );
    }

    #[test]
    fn test_greek_alpha_beta_pair() {
        assert_eq!(
            normalize_title("(α,β)-Core Query over Bipartite Graphs"),
            "alphabetacorequeryoverbipartitegraphs"
        );
    }

    #[test]
    fn test_greek_uppercase() {
        assert_eq!(
            normalize_title("Δ-learning for robotics"),
            "deltalearningforrobotics"
        );
    }

    // =========================================================================
    // Separated diacritics from PDF extraction
    // =========================================================================

    #[test]
    fn test_diacritic_umlaut_space() {
        assert_eq!(normalize_title("B \u{a8}UNZ"), "bunz");
    }

    #[test]
    fn test_diacritic_umlaut_dottling() {
        assert_eq!(normalize_title("D \u{a8}OTTLING"), "dottling");
    }

    #[test]
    fn test_diacritic_acute_renyi() {
        assert_eq!(normalize_title("R\u{b4}enyi"), "renyi");
    }

    #[test]
    fn test_diacritic_mixed_ordonez() {
        assert_eq!(normalize_title("Ord\u{b4}o\u{2dc}nez"), "ordonez");
    }

    #[test]
    fn test_diacritic_caron_novacek() {
        assert_eq!(normalize_title("Nov\u{b4}a\u{2c7}cek"), "novacek");
    }

    #[test]
    fn test_diacritic_leading_umlaut() {
        assert_eq!(
            normalize_title("\u{a8}Uber das paulische"),
            "uberdaspaulische"
        );
    }

    #[test]
    fn test_diacritic_habock() {
        assert_eq!(normalize_title("HAB \u{a8}OCK"), "habock");
    }

    #[test]
    fn test_diacritic_krol() {
        assert_eq!(normalize_title("KR \u{b4}OL"), "krol");
    }

    #[test]
    fn test_diacritic_grave_riviere() {
        assert_eq!(normalize_title("RIVI`ERE"), "riviere");
    }

    #[test]
    fn test_diacritic_no_change() {
        assert_eq!(
            normalize_title("Normal text without diacritics"),
            "normaltextwithoutdiacritics"
        );
    }

    // =========================================================================
    // Math symbol replacement
    // =========================================================================

    #[test]
    fn test_normalize_h_infinity() {
        assert_eq!(
            normalize_title("H\u{221E} almost state synchronization"),
            "hinfinityalmoststatesynchronization"
        );
        assert_eq!(
            normalize_title("Robust H\u{221E} filtering"),
            "robusthinfinityfiltering"
        );
    }

    #[test]
    fn test_h_infinity_fuzzy_match() {
        assert!(titles_match(
            "H\u{221E} almost state synchronization for homogeneous networks",
            "H-infinity almost state synchronization for homogeneous networks"
        ));
    }

    #[test]
    fn test_math_sqrt() {
        assert_eq!(
            normalize_title("Breaking the o(√n)-bit barrier"),
            "breakingtheosqrtnbitbarrier"
        );
    }

    #[test]
    fn test_math_leq_geq() {
        assert_eq!(normalize_title("x ≤ y"), "xleqy");
        assert_eq!(normalize_title("y ≥ z"), "ygeqz");
    }

    #[test]
    fn test_math_set_ops() {
        assert_eq!(normalize_title("A ∪ B ∩ C"), "acupbcapc");
    }

    #[test]
    fn test_math_arrow() {
        assert_eq!(normalize_title("f: A → B"), "fatob");
    }

    #[test]
    fn test_math_implies() {
        assert_eq!(normalize_title("P ⇒ Q"), "pimpliesq");
    }

    #[test]
    fn test_math_pm_times() {
        assert_eq!(normalize_title("a ± b × c"), "apmbtimesc");
    }

    #[test]
    fn test_math_nabla_partial() {
        assert_eq!(normalize_title("∇f and ∂g"), "nablafandpartialg");
    }

    #[test]
    fn test_math_no_change() {
        assert_eq!(
            normalize_title("Normal title without math"),
            "normaltitlewithoutmath"
        );
    }

    // =========================================================================
    // Combined pipeline
    // =========================================================================

    #[test]
    fn test_combined_greek_and_diacritics() {
        assert_eq!(
            normalize_title("τCFI: Type-assisted Control Flow"),
            "taucfitypeassistedcontrolflow"
        );
    }

    #[test]
    fn test_combined_greek_and_math() {
        assert_eq!(normalize_title("α ≤ β → γ"), "alphaleqbetatogamma");
    }

    #[test]
    fn test_combined_diacritic_and_math() {
        assert_eq!(
            normalize_title("R\u{b4}enyi divergence ≤ KL divergence"),
            "renyidivergenceleqkldivergence"
        );
    }

    #[test]
    fn test_combined_all_three() {
        assert_eq!(
            normalize_title("εpsolute with B \u{a8}UNZ and √n bound"),
            "epsilonpsolutewithbunzandsqrtnbound"
        );
    }

    // =========================================================================
    // Fuzzy matching across normalized forms
    // =========================================================================

    #[test]
    fn test_fuzzy_greek_vs_spelled_out() {
        assert!(titles_match(
            "εpsolute: Efficiently querying databases while providing differential privacy",
            "Epsilonpsolute: Efficiently querying databases while providing differential privacy"
        ));
    }

    #[test]
    fn test_fuzzy_diacritic_vs_clean() {
        assert!(titles_match(
            "R\u{b4}enyi differential privacy of the sampled Gaussian mechanism",
            "Renyi differential privacy of the sampled Gaussian mechanism"
        ));
    }

    #[test]
    fn test_fuzzy_math_vs_word() {
        assert!(titles_match(
            "Breaking the o(√n)-bit barrier: Byzantine agreement with polylog bits",
            "Breaking the o(sqrt n)-bit barrier: Byzantine agreement with polylog bits"
        ));
    }

    #[test]
    fn test_fuzzy_accented_vs_ascii() {
        assert!(titles_match(
            "Déjà Vu: Side-Channel Analysis of Randomization",
            "Deja Vu: Side-Channel Analysis of Randomization"
        ));
    }

    // =========================================================================
    // Conservative prefix matching with subtitle awareness
    // =========================================================================

    #[test]
    fn test_prefix_subtitle_mismatch_rejects() {
        assert!(!titles_match(
            "Won't Somebody Think of the Children?",
            "Won't somebody think of the children? Examining COPPA compliance at scale"
        ));
    }

    #[test]
    fn test_prefix_both_have_subtitle_accepts() {
        assert!(titles_match(
            "Attention is all you need: Transformers for sequence modeling",
            "Attention is all you need: Transformers for sequence modeling and beyond"
        ));
    }

    #[test]
    fn test_prefix_exact_match_still_works() {
        assert!(titles_match(
            "A very long title about detecting hallucinated references in academic papers",
            "A very long title about detecting hallucinated references in academic papers"
        ));
    }

    #[test]
    fn test_prefix_short_title_no_prefix_match() {
        assert!(!titles_match("Short title", "Short title with extra words"));
    }
}
