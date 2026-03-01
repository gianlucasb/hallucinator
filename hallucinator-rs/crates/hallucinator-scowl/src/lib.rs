//! SCOWL-based English dictionary for word validation.
//!
//! This crate provides an English dictionary based on SCOWL (Spell Checker
//! Oriented Word Lists) for validating words during hyphenation fixing.
//!
//! # Loading Modes
//!
//! - **Embedded**: Load the compiled-in word list with [`ScowlDictionary::embedded()`]
//! - **File-based**: Load from a file path with [`ScowlDictionary::from_file()`]
//!
//! # Integration with hallucinator-parsing
//!
//! This crate implements the [`Dictionary`] trait from `hallucinator-parsing`,
//! allowing it to be used with [`hallucinator_parsing::text_processing::fix_hyphenation_with_dict`].

use std::collections::HashSet;
use std::io;
use std::path::Path;

use hallucinator_parsing::Dictionary;

/// A dictionary backed by SCOWL word lists.
///
/// Supports both embedded (compile-time) and file-based (runtime) loading.
pub struct ScowlDictionary {
    words: HashSet<String>,
}

impl ScowlDictionary {
    /// Load the embedded SCOWL word list (size 70, ~160K words) plus academic terms.
    ///
    /// This is the recommended way to use the dictionary for most cases.
    /// Includes both the base SCOWL dictionary and a curated list of academic/technical
    /// terms commonly found in research papers.
    pub fn embedded() -> Self {
        let scowl = include_str!("../data/wordlist.txt");
        let academic = include_str!("../data/academic-terms.txt");
        Self::from_multiple(&[scowl, academic])
    }

    /// Load dictionary from multiple string sources.
    ///
    /// Words from all sources are combined into a single dictionary.
    pub fn from_multiple(sources: &[&str]) -> Self {
        let words = sources
            .iter()
            .flat_map(|content| {
                content
                    .lines()
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(|l| l.to_lowercase())
            })
            .collect();
        Self { words }
    }

    /// Load dictionary from a file path.
    ///
    /// This allows loading custom or updated word lists at runtime.
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::from_str(&content))
    }

    /// Load dictionary from string content.
    ///
    /// Each line should contain one word. Empty lines and lines starting
    /// with '#' are ignored.
    pub fn from_str(content: &str) -> Self {
        let words = content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_lowercase())
            .collect();
        Self { words }
    }

    /// Check if a word exists in the dictionary.
    ///
    /// The lookup is case-insensitive.
    pub fn contains(&self, word: &str) -> bool {
        self.words.contains(&word.to_lowercase())
    }

    /// Return the number of words in the dictionary.
    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// Check if the dictionary is empty.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

impl Dictionary for ScowlDictionary {
    fn contains(&self, word: &str) -> bool {
        self.words.contains(&word.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_dictionary_loads() {
        let dict = ScowlDictionary::embedded();
        assert!(dict.len() > 100_000, "Expected >100K words, got {}", dict.len());
    }

    #[test]
    fn test_contains_common_words() {
        let dict = ScowlDictionary::embedded();

        // Common English words
        assert!(dict.contains("the"));
        assert!(dict.contains("hello"));
        assert!(dict.contains("world"));

        // Technical/academic words we specifically need
        assert!(dict.contains("byzantine"));
        assert!(dict.contains("identifier"));
        assert!(dict.contains("transformer"));
        assert!(dict.contains("neural"));
        assert!(dict.contains("classifier"));
        assert!(dict.contains("automated"));
    }

    #[test]
    fn test_contains_academic_terms() {
        let dict = ScowlDictionary::embedded();

        // ML/AI terms from academic supplement
        assert!(dict.contains("tokenization"), "tokenization should be in dict");
        assert!(dict.contains("analyzing"), "analyzing should be in dict");
        assert!(dict.contains("randomized"), "randomized should be in dict");
        assert!(dict.contains("convolutional"), "convolutional should be in dict");
        assert!(dict.contains("hyperparameter"), "hyperparameter should be in dict");
        assert!(dict.contains("backpropagation"), "backpropagation should be in dict");
        assert!(dict.contains("autoencoder"), "autoencoder should be in dict");

        // CS terms
        assert!(dict.contains("middleware"), "middleware should be in dict");
        assert!(dict.contains("containerized"), "containerized should be in dict");
        assert!(dict.contains("virtualization"), "virtualization should be in dict");
        assert!(dict.contains("serialization"), "serialization should be in dict");

        // Security terms
        assert!(dict.contains("ciphertext"), "ciphertext should be in dict");
        assert!(dict.contains("blockchain"), "blockchain should be in dict");
        assert!(dict.contains("ransomware"), "ransomware should be in dict");
    }

    #[test]
    fn test_case_insensitive() {
        let dict = ScowlDictionary::embedded();

        assert!(dict.contains("Byzantine"));
        assert!(dict.contains("BYZANTINE"));
        assert!(dict.contains("byzantine"));
    }

    #[test]
    fn test_does_not_contain_gibberish() {
        let dict = ScowlDictionary::embedded();

        assert!(!dict.contains("asdfghjkl"));
        assert!(!dict.contains("xyzzy123"));
    }

    #[test]
    fn test_from_str() {
        let content = "hello\nworld\n# comment\n\ntest";
        let dict = ScowlDictionary::from_str(content);

        assert_eq!(dict.len(), 3);
        assert!(dict.contains("hello"));
        assert!(dict.contains("world"));
        assert!(dict.contains("test"));
    }
}
