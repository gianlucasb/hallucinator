//! Dictionary trait for word validation.
//!
//! This module defines the [`Dictionary`] trait used by hyphenation fixing
//! to validate whether merged words are valid English words.

/// Trait for word validation dictionaries.
///
/// Implementations can back this with embedded word lists, file-based
/// dictionaries, or external spell-checking services.
pub trait Dictionary: Send + Sync {
    /// Check if a word exists in the dictionary.
    ///
    /// Implementations should perform case-insensitive lookups.
    fn contains(&self, word: &str) -> bool;
}
