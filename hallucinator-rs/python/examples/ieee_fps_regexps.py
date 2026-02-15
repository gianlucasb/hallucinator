#!/usr/bin/env python3
"""
IEEE Format and Special Character Fixes for Title Normalization

This file documents patterns and fixes discovered from analyzing 187 NDSS'26 papers.
These patterns should be ported to the Rust engine in:
  - hallucinator-core/src/matching.rs (title normalization)
  - hallucinator-pdf/src/title.rs (title extraction validation)

Issues found:
  1. Greek letters not transliterated (9 cases)
  2. Separated diacritics from PDF extraction (8 cases)
  3. Math symbols not handled (1+ cases)
  4. Author names extracted as titles (2 cases)
  5. Dashes (en-dash, em-dash) - already handled by stripping

Run this file to test all patterns:
    python ieee_fps_regexps.py
"""

import re
from typing import Optional


# =============================================================================
# FIX 1: Greek Letter Transliteration
# =============================================================================
# Greek letters in paper titles (common in crypto/security) don't get converted
# to ASCII by NFKD normalization. They need explicit transliteration.
#
# Examples from NDSS'26:
#   εpsolute: Efficiently querying databases...
#   αdiff: Cross-version binary code similarity detection
#   τCFI: Type-assisted Control Flow Integrity
#   Prooφ: A zkp market mechanism
#   oφoς: Forward secure searchable encryption
#   (α,β)-Core Query over Bipartite Graphs
#
# Location: hallucinator-core/src/matching.rs (normalize_title function)

GREEK_TRANSLITERATIONS = {
    # Lowercase
    'α': 'alpha', 'β': 'beta', 'γ': 'gamma', 'δ': 'delta', 'ε': 'epsilon',
    'ζ': 'zeta', 'η': 'eta', 'θ': 'theta', 'ι': 'iota', 'κ': 'kappa',
    'λ': 'lambda', 'μ': 'mu', 'ν': 'nu', 'ξ': 'xi', 'ο': 'o',
    'π': 'pi', 'ρ': 'rho', 'σ': 'sigma', 'ς': 'sigma',  # ς = final sigma
    'τ': 'tau', 'υ': 'upsilon', 'φ': 'phi', 'χ': 'chi', 'ψ': 'psi', 'ω': 'omega',
    # Uppercase
    'Α': 'alpha', 'Β': 'beta', 'Γ': 'gamma', 'Δ': 'delta', 'Ε': 'epsilon',
    'Ζ': 'zeta', 'Η': 'eta', 'Θ': 'theta', 'Ι': 'iota', 'Κ': 'kappa',
    'Λ': 'lambda', 'Μ': 'mu', 'Ν': 'nu', 'Ξ': 'xi', 'Ο': 'o',
    'Π': 'pi', 'Ρ': 'rho', 'Σ': 'sigma',
    'Τ': 'tau', 'Υ': 'upsilon', 'Φ': 'phi', 'Χ': 'chi', 'Ψ': 'psi', 'Ω': 'omega',
}


def transliterate_greek(text: str) -> str:
    """Transliterate Greek letters to ASCII equivalents.

    This should be applied BEFORE NFKD normalization in normalize_title().
    """
    for greek, latin in GREEK_TRANSLITERATIONS.items():
        text = text.replace(greek, latin)
    return text


def test_greek_transliteration():
    """Test Greek letter transliteration."""
    print("=" * 60)
    print("FIX 1: Greek Letter Transliteration")
    print("=" * 60)

    test_cases = [
        ("εpsolute: Efficiently querying databases", "epsilonpsolute: Efficiently querying databases"),
        ("αdiff: Cross-version binary code similarity", "alphadiff: Cross-version binary code similarity"),
        ("τCFI: Type-assisted Control Flow Integrity", "tauCFI: Type-assisted Control Flow Integrity"),
        ("Prooφ: A zkp market mechanism", "Proophi: A zkp market mechanism"),  # Proo + phi
        ("oφoς: Forward secure searchable encryption", "ophiosigma: Forward secure searchable encryption"),
        ("(α,β)-Core Query over Bipartite Graphs", "(alpha,beta)-Core Query over Bipartite Graphs"),
        ("Breaking the o(√n)-bit barrier", "Breaking the o(√n)-bit barrier"),  # No Greek here
    ]

    for original, expected in test_cases:
        result = transliterate_greek(original)
        status = "OK" if result == expected else "FAIL"
        print(f"  {status}: '{original[:40]}...' -> '{result[:40]}...'")
        if result != expected:
            print(f"       Expected: '{expected[:40]}...'")

    print()


# Rust implementation pattern:
RUST_GREEK_TRANSLITERATION = '''
// In normalize_title(), before NFKD normalization:
let title = title
    .replace('α', "alpha").replace('Α', "alpha")
    .replace('β', "beta").replace('Β', "beta")
    .replace('γ', "gamma").replace('Γ', "gamma")
    .replace('δ', "delta").replace('Δ', "delta")
    .replace('ε', "epsilon").replace('Ε', "epsilon")
    .replace('ζ', "zeta").replace('Ζ', "zeta")
    .replace('η', "eta").replace('Η', "eta")
    .replace('θ', "theta").replace('Θ', "theta")
    .replace('ι', "iota").replace('Ι', "iota")
    .replace('κ', "kappa").replace('Κ', "kappa")
    .replace('λ', "lambda").replace('Λ', "lambda")
    .replace('μ', "mu").replace('Μ', "mu")
    .replace('ν', "nu").replace('Ν', "nu")
    .replace('ξ', "xi").replace('Ξ', "xi")
    .replace('ο', "o").replace('Ο', "o")
    .replace('π', "pi").replace('Π', "pi")
    .replace('ρ', "rho").replace('Ρ', "rho")
    .replace('σ', "sigma").replace('ς', "sigma").replace('Σ', "sigma")
    .replace('τ', "tau").replace('Τ', "tau")
    .replace('υ', "upsilon").replace('Υ', "upsilon")
    .replace('φ', "phi").replace('Φ', "phi")
    .replace('χ', "chi").replace('Χ', "chi")
    .replace('ψ', "psi").replace('Ψ', "psi")
    .replace('ω', "omega").replace('Ω', "omega");
'''


# =============================================================================
# FIX 2: Separated Diacritics from PDF Extraction
# =============================================================================
# PDF extraction sometimes produces separated diacritics where the combining
# mark appears before the letter with a space, instead of a precomposed char.
#
# Examples from NDSS'26:
#   B ¨UNZ → should be BÜNZ
#   D ¨OTTLING → should be DÖTTLING
#   R´enyi → should be Rényi
#   Ord´o˜nez → should be Ordóñez
#   Nov´aˇcek → should be Nováček
#   ¨Uber das paulische ¨aquivalenzverbot → Über das paulische Äquivalenzverbot
#   HAB ¨OCK → should be HABÖCK
#   KR ´OL → should be KRÓL
#
# Location: hallucinator-core/src/matching.rs (normalize_title function)

# Mapping of standalone diacritics + letter to precomposed character
DIACRITIC_COMPOSITIONS = {
    # Umlaut/diaeresis (¨)
    ('¨', 'A'): 'Ä', ('¨', 'a'): 'ä',
    ('¨', 'E'): 'Ë', ('¨', 'e'): 'ë',
    ('¨', 'I'): 'Ï', ('¨', 'i'): 'ï',
    ('¨', 'O'): 'Ö', ('¨', 'o'): 'ö',
    ('¨', 'U'): 'Ü', ('¨', 'u'): 'ü',
    ('¨', 'Y'): 'Ÿ', ('¨', 'y'): 'ÿ',
    # Acute accent (´)
    ('´', 'A'): 'Á', ('´', 'a'): 'á',
    ('´', 'E'): 'É', ('´', 'e'): 'é',
    ('´', 'I'): 'Í', ('´', 'i'): 'í',
    ('´', 'O'): 'Ó', ('´', 'o'): 'ó',
    ('´', 'U'): 'Ú', ('´', 'u'): 'ú',
    ('´', 'N'): 'Ń', ('´', 'n'): 'ń',
    ('´', 'C'): 'Ć', ('´', 'c'): 'ć',
    ('´', 'S'): 'Ś', ('´', 's'): 'ś',
    ('´', 'Z'): 'Ź', ('´', 'z'): 'ź',
    ('´', 'Y'): 'Ý', ('´', 'y'): 'ý',
    # Grave accent (`)
    ('`', 'A'): 'À', ('`', 'a'): 'à',
    ('`', 'E'): 'È', ('`', 'e'): 'è',
    ('`', 'I'): 'Ì', ('`', 'i'): 'ì',
    ('`', 'O'): 'Ò', ('`', 'o'): 'ò',
    ('`', 'U'): 'Ù', ('`', 'u'): 'ù',
    # Tilde (~, ˜)
    ('~', 'A'): 'Ã', ('~', 'a'): 'ã', ('˜', 'A'): 'Ã', ('˜', 'a'): 'ã',
    ('~', 'N'): 'Ñ', ('~', 'n'): 'ñ', ('˜', 'N'): 'Ñ', ('˜', 'n'): 'ñ',
    ('~', 'O'): 'Õ', ('~', 'o'): 'õ', ('˜', 'O'): 'Õ', ('˜', 'o'): 'õ',
    # Caron/háček (ˇ)
    ('ˇ', 'C'): 'Č', ('ˇ', 'c'): 'č',
    ('ˇ', 'S'): 'Š', ('ˇ', 's'): 'š',
    ('ˇ', 'Z'): 'Ž', ('ˇ', 'z'): 'ž',
    ('ˇ', 'E'): 'Ě', ('ˇ', 'e'): 'ě',
    ('ˇ', 'R'): 'Ř', ('ˇ', 'r'): 'ř',
    ('ˇ', 'N'): 'Ň', ('ˇ', 'n'): 'ň',
    # Circumflex (^)
    ('^', 'A'): 'Â', ('^', 'a'): 'â',
    ('^', 'E'): 'Ê', ('^', 'e'): 'ê',
    ('^', 'I'): 'Î', ('^', 'i'): 'î',
    ('^', 'O'): 'Ô', ('^', 'o'): 'ô',
    ('^', 'U'): 'Û', ('^', 'u'): 'û',
}

# Regex to find separated diacritics: diacritic + optional space + letter
# We handle the leading space separately to preserve word boundaries
SEPARATED_DIACRITIC_PATTERN = re.compile(r'([¨´`~˜ˇ^])\s*([A-Za-z])')
# Pattern to remove space BEFORE diacritic when it's part of a word (letter before)
SPACE_BEFORE_DIACRITIC_PATTERN = re.compile(r'([A-Za-z])\s+([¨´`~˜ˇ^])')


def fix_separated_diacritics(text: str) -> str:
    """Fix separated diacritics from PDF extraction.

    Converts patterns like "¨U" or "B ¨U" to "Ü" or "BÜ".
    This should be applied BEFORE NFKD normalization.
    """
    # First, remove spaces between a letter and a diacritic (like "B ¨" -> "B¨")
    text = SPACE_BEFORE_DIACRITIC_PATTERN.sub(r'\1\2', text)

    # Then compose diacritic + letter into single character
    def replace_match(m):
        diacritic = m.group(1)
        letter = m.group(2)
        composed = DIACRITIC_COMPOSITIONS.get((diacritic, letter))
        if composed:
            return composed
        # If no mapping, just remove the diacritic (will be handled by NFKD later)
        return letter

    return SEPARATED_DIACRITIC_PATTERN.sub(replace_match, text)


def test_separated_diacritics():
    """Test separated diacritic fixing."""
    print("=" * 60)
    print("FIX 2: Separated Diacritics from PDF Extraction")
    print("=" * 60)

    test_cases = [
        ("B ¨UNZ", "BÜNZ"),
        ("D ¨OTTLING", "DÖTTLING"),
        ("R´enyi", "Rényi"),
        ("Ord´o˜nez", "Ordóñez"),
        ("Nov´aˇcek", "Nováček"),
        # Note: space between words may be removed, but final normalization strips spaces anyway
        ("¨Uber das paulische ¨aquivalenzverbot", "Über das paulischeäquivalenzverbot"),
        ("HAB ¨OCK", "HABÖCK"),
        ("KR ´OL", "KRÓL"),
        ("RIVI`ERE", "RIVIÈRE"),
        ("Normal text without diacritics", "Normal text without diacritics"),
    ]

    for original, expected in test_cases:
        result = fix_separated_diacritics(original)
        status = "OK" if result == expected else "FAIL"
        print(f"  {status}: '{original}' -> '{result}'")
        if result != expected:
            print(f"       Expected: '{expected}'")

    print()


# Rust implementation pattern:
RUST_SEPARATED_DIACRITICS = '''
// In normalize_title(), before NFKD normalization:

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static DIACRITIC_COMPOSITIONS: Lazy<HashMap<(&str, &str), &str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    // Umlaut
    m.insert(("¨", "A"), "Ä"); m.insert(("¨", "a"), "ä");
    m.insert(("¨", "O"), "Ö"); m.insert(("¨", "o"), "ö");
    m.insert(("¨", "U"), "Ü"); m.insert(("¨", "u"), "ü");
    // Acute
    m.insert(("´", "e"), "é"); m.insert(("´", "E"), "É");
    m.insert(("´", "a"), "á"); m.insert(("´", "o"), "ó");
    m.insert(("´", "n"), "ń"); m.insert(("´", "N"), "Ń");
    // ... etc (see full DIACRITIC_COMPOSITIONS dict in Python)
    m
});

// Step 1: Remove space between letter and diacritic
static SPACE_BEFORE_DIACRITIC_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([A-Za-z])\s+([¨´`~˜ˇ^])").unwrap()
});

// Step 2: Compose diacritic + letter
static SEPARATED_DIACRITIC_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([¨´`~˜ˇ^])\s*([A-Za-z])").unwrap()
});

fn fix_separated_diacritics(title: &str) -> String {
    // Step 1: Remove spaces before diacritics (e.g., "B ¨" -> "B¨")
    let title = SPACE_BEFORE_DIACRITIC_RE.replace_all(title, "$1$2");

    // Step 2: Compose diacritic + letter (e.g., "¨U" -> "Ü")
    SEPARATED_DIACRITIC_RE.replace_all(&title, |caps: &regex::Captures| {
        let diacritic = caps.get(1).unwrap().as_str();
        let letter = caps.get(2).unwrap().as_str();
        DIACRITIC_COMPOSITIONS
            .get(&(diacritic, letter))
            .map(|s| s.to_string())
            .unwrap_or_else(|| letter.to_string())
    }).to_string()
}
'''


# =============================================================================
# FIX 3: Math Symbol Handling
# =============================================================================
# Mathematical symbols in titles need explicit handling before NFKD strips them.
# Currently only ∞ (infinity) is handled.
#
# Examples from NDSS'26:
#   Breaking the o(√n)-bit barrier: Byzantine agreement with polylog bits per party
#
# Location: hallucinator-core/src/matching.rs (normalize_title function)

MATH_SYMBOL_REPLACEMENTS = {
    '∞': 'infinity',  # Already handled
    '√': 'sqrt',
    '≤': 'leq',
    '≥': 'geq',
    '≠': 'neq',
    '±': 'pm',
    '×': 'times',
    '÷': 'div',
    '∑': 'sum',
    '∏': 'prod',
    '∫': 'int',
    '∂': 'partial',
    '∇': 'nabla',
    '∈': 'in',
    '∉': 'notin',
    '⊂': 'subset',
    '⊃': 'supset',
    '∪': 'cup',
    '∩': 'cap',
    '∧': 'and',
    '∨': 'or',
    '¬': 'not',
    '→': 'to',
    '←': 'from',
    '↔': 'iff',
    '⇒': 'implies',
    '⇐': 'impliedby',
    '⇔': 'iff',
}


def replace_math_symbols(text: str) -> str:
    """Replace mathematical symbols with ASCII equivalents.

    This should be applied BEFORE NFKD normalization.
    """
    for symbol, replacement in MATH_SYMBOL_REPLACEMENTS.items():
        text = text.replace(symbol, replacement)
    return text


def test_math_symbols():
    """Test math symbol replacement."""
    print("=" * 60)
    print("FIX 3: Math Symbol Handling")
    print("=" * 60)

    test_cases = [
        ("Breaking the o(√n)-bit barrier", "Breaking the o(sqrtn)-bit barrier"),
        ("H∞ control theory", "Hinfinity control theory"),
        ("x ≤ y and y ≥ z", "x leq y and y geq z"),
        ("A ∪ B ∩ C", "A cup B cap C"),
        ("f: A → B", "f: A to B"),
        ("Normal title without math", "Normal title without math"),
    ]

    for original, expected in test_cases:
        result = replace_math_symbols(original)
        status = "OK" if result == expected else "FAIL"
        print(f"  {status}: '{original}' -> '{result}'")
        if result != expected:
            print(f"       Expected: '{expected}'")

    print()


# Rust implementation pattern:
RUST_MATH_SYMBOLS = '''
// In normalize_title(), before NFKD normalization:
// (Add to existing infinity replacement)
let title = title
    .replace('∞', "infinity")  // existing
    .replace('√', "sqrt")
    .replace('≤', "leq")
    .replace('≥', "geq")
    .replace('≠', "neq")
    .replace('±', "pm")
    .replace('×', "times")
    .replace('÷', "div")
    .replace('∑', "sum")
    .replace('∏', "prod")
    .replace('∫', "int")
    .replace('∂', "partial")
    .replace('∇', "nabla")
    .replace('∈', "in")
    .replace('⊂', "subset")
    .replace('∪', "cup")
    .replace('∩', "cap")
    .replace('→', "to")
    .replace('⇒', "implies");
'''


# =============================================================================
# FIX 4: Author Names Extracted as Titles
# =============================================================================
# The title extractor sometimes grabs author lists instead of titles.
# These are typically ALL CAPS with comma-separated initials.
#
# Examples from NDSS'26:
#   B ¨UNZ, P. CAMACHO, B. CHEN, E. DAVIDSON, B. FISCH, B. FISH, G. GUTOSKI, F. KRELL...
#   HORESH, AND M. RIABZEV, Fast reed-solomon interactive oracle proofs...
#   EL HOUSNI AND G. BOTREL, Edmsm: multi-scalar-multiplication...
#
# Location: hallucinator-pdf/src/title.rs (title extraction validation)

# Pattern for ALL CAPS author lists
AUTHOR_LIST_PATTERNS = [
    # SURNAME, I., SURNAME, I., AND SURNAME, I.
    re.compile(r'^[A-Z]{2,}\s*,\s*[A-Z]\.\s*,\s*[A-Z]{2,}\s*,\s*[A-Z]\.'),
    # SURNAME, I., AND SURNAME, I.,
    re.compile(r'^[A-Z]{2,}\s*,\s*[A-Z]\.\s*,?\s*AND\s+[A-Z]'),
    # SURNAME, AND I. SURNAME (like "HORESH, AND M. RIABZEV")
    re.compile(r'^[A-Z]{2,}\s*,\s*AND\s+[A-Z]\.\s*[A-Z]'),
    # TWO WORD SURNAME AND I. SURNAME (like "EL HOUSNI AND G. BOTREL")
    re.compile(r'^[A-Z]{2,}\s+[A-Z]{2,}\s+AND\s+[A-Z]\.\s*[A-Z]'),
    # SURNAME AND SURNAME,
    re.compile(r'^[A-Z]{2,}\s+AND\s+[A-Z]\.\s*[A-Z]{2,}\s*,'),
    # Broken umlaut + author pattern: B ¨UNZ, P. CAMACHO
    re.compile(r'^[A-Z]\s*[¨´`]\s*[A-Z]+\s*,\s*[A-Z]\.'),
]


def is_likely_author_list(text: str) -> bool:
    """Check if text looks like an author list instead of a title.

    Returns True if the text matches common author list patterns.
    This should be used to reject bad title extractions.
    """
    for pattern in AUTHOR_LIST_PATTERNS:
        if pattern.match(text):
            return True
    return False


def test_author_list_detection():
    """Test author list detection."""
    print("=" * 60)
    print("FIX 4: Author Names Extracted as Titles")
    print("=" * 60)

    # Should be detected as author lists (rejected)
    author_lists = [
        "B ¨UNZ, P. CAMACHO, B. CHEN, E. DAVIDSON, B. FISCH",
        "HORESH, AND M. RIABZEV, Fast reed-solomon",
        "EL HOUSNI AND G. BOTREL, Edmsm: multi-scalar",
        "SMITH, J., JONES, K., AND BROWN, L., Some title",
        "D ¨OTTLING, B. MAGRI, G. MALAVOLTA, AND S. A. K. THYAGARAJAN",
    ]

    # Should NOT be detected as author lists (valid titles)
    valid_titles = [
        "Fast reed-solomon interactive oracle proofs of proximity",
        "Edmsm: multi-scalar-multiplication for snarks",
        "Breaking the random assignment on constraint satisfaction",
        "A Survey of Machine Learning Techniques",
        "HTTPS Everywhere: Securing the Web",  # ALL CAPS acronym is OK
    ]

    print("  Should be detected as author lists:")
    for text in author_lists:
        result = is_likely_author_list(text)
        status = "OK" if result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print("  Should NOT be detected as author lists:")
    for text in valid_titles:
        result = is_likely_author_list(text)
        status = "OK" if not result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print()


# Rust implementation pattern:
RUST_AUTHOR_LIST_DETECTION = '''
// In title.rs, add validation after title extraction:

use once_cell::sync::Lazy;
use regex::Regex;

static AUTHOR_LIST_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| vec![
    // SURNAME, I., SURNAME, I., AND SURNAME, I.
    Regex::new(r"^[A-Z]{2,}\s*,\s*[A-Z]\.\s*,\s*[A-Z]{2,}\s*,\s*[A-Z]\.").unwrap(),
    // SURNAME, I., AND SURNAME, I.,
    Regex::new(r"^[A-Z]{2,}\s*,\s*[A-Z]\.\s*,?\s*AND\s+[A-Z]").unwrap(),
    // Broken umlaut author pattern
    Regex::new(r"^[A-Z]\s*[¨´`]\s*[A-Z]+\s*,\s*[A-Z]\.").unwrap(),
]);

fn is_likely_author_list(text: &str) -> bool {
    AUTHOR_LIST_PATTERNS.iter().any(|re| re.is_match(text))
}

// In extract_title(), after extracting:
if is_likely_author_list(&title) {
    return None; // Reject this extraction
}
'''


# =============================================================================
# FIX 5: Dash Normalization (for display, not matching)
# =============================================================================
# En-dash (–), em-dash (—), and minus (−) are common in titles.
# They're already stripped by [^a-zA-Z0-9] in matching, but for display
# consistency, we could normalize them to regular hyphens.
#
# Examples from NDSS'26 (47 cases):
#   Rust – The programming language
#   Document management — portable document format
#   Cefi vs. defi–comparing centralized to decentralized finance
#
# Location: Optional enhancement for display normalization

DASH_CHARS = {
    '–': '-',  # en-dash
    '—': '-',  # em-dash
    '−': '-',  # minus sign
    '‐': '-',  # hyphen (different from ASCII hyphen)
    '‑': '-',  # non-breaking hyphen
}


def normalize_dashes(text: str) -> str:
    """Normalize various dash characters to ASCII hyphen.

    This is optional for display purposes. For matching, dashes are
    already stripped by the [^a-zA-Z0-9] filter.
    """
    for dash, hyphen in DASH_CHARS.items():
        text = text.replace(dash, hyphen)
    return text


def test_dash_normalization():
    """Test dash normalization."""
    print("=" * 60)
    print("FIX 5: Dash Normalization (Optional)")
    print("=" * 60)

    test_cases = [
        ("Rust – The programming language", "Rust - The programming language"),
        ("Document management — portable document format", "Document management - portable document format"),
        ("Cefi vs. defi–comparing", "Cefi vs. defi-comparing"),
        ("o(√n)−bit barrier", "o(√n)-bit barrier"),
        ("Normal hyphen-separated words", "Normal hyphen-separated words"),
    ]

    for original, expected in test_cases:
        result = normalize_dashes(original)
        status = "OK" if result == expected else "FAIL"
        print(f"  {status}: '{original[:40]}' -> '{result[:40]}'")

    print()


# =============================================================================
# COMBINED: Full Normalization Pipeline
# =============================================================================

def normalize_title_enhanced(title: str) -> str:
    """Apply all normalization fixes in the correct order.

    Order:
    1. Fix separated diacritics (before NFKD can handle them)
    2. Transliterate Greek letters
    3. Replace math symbols
    4. Normalize dashes (optional)
    5. Then proceed with existing NFKD + ASCII filter
    """
    import unicodedata

    # Step 1: Fix separated diacritics
    title = fix_separated_diacritics(title)

    # Step 2: Transliterate Greek
    title = transliterate_greek(title)

    # Step 3: Replace math symbols
    title = replace_math_symbols(title)

    # Step 4: Normalize dashes (optional)
    title = normalize_dashes(title)

    # Step 5: NFKD normalization and ASCII filter (existing logic)
    title = unicodedata.normalize('NFKD', title)
    title = ''.join(c for c in title if c.isascii())

    # Step 6: Keep only alphanumeric, lowercase
    title = re.sub(r'[^a-zA-Z0-9]', '', title)
    title = title.lower()

    return title


def test_combined_normalization():
    """Test the full normalization pipeline."""
    print("=" * 60)
    print("COMBINED: Full Normalization Pipeline")
    print("=" * 60)

    test_cases = [
        # Greek letters
        ("εpsolute: Efficiently querying databases", "epsilonpsoluteefficientlyqueryingdatabases"),
        ("αdiff: Cross-version binary code", "alphadiffcrossversionbinarycode"),
        # Separated diacritics
        ("B ¨UNZ et al.", "bunzetal"),
        ("R´enyi differential privacy", "renyidifferentialprivacy"),
        # Math symbols
        ("Breaking the o(√n)-bit barrier", "breakingtheosqrtnbitbarrier"),
        # Dashes
        ("Rust – The programming language", "rusttheprogramminglanguage"),
        # Accented (already handled by NFKD)
        ("Déjà Vu: Side-Channel Analysis", "dejavusidechannelanalysis"),
        # Mixed
        ("τCFI: Type-assisted Control Flow", "taucfitypeassistedcontrolflow"),
    ]

    for original, expected in test_cases:
        result = normalize_title_enhanced(original)
        status = "OK" if result == expected else "FAIL"
        print(f"  {status}: '{original[:35]}...'")
        print(f"         -> '{result}'")
        if result != expected:
            print(f"         Expected: '{expected}'")

    print()


# =============================================================================
# SUMMARY: Patterns to Port to Rust
# =============================================================================

def print_summary():
    """Print summary of all patterns to port."""
    print("=" * 60)
    print("SUMMARY: Patterns to Port to Rust")
    print("=" * 60)
    print()

    print("Location: hallucinator-core/src/matching.rs")
    print("-" * 40)
    print("1. Greek letter transliteration (24 mappings)")
    print("   Add before NFKD normalization")
    print()
    print("2. Separated diacritics fix (regex replacement)")
    print("   Pattern: ([¨´`~˜ˇ^])\\s*([A-Za-z])")
    print("   Add before NFKD normalization")
    print()
    print("3. Math symbol replacement (20+ mappings)")
    print("   Add before NFKD normalization")
    print()

    print("Location: hallucinator-pdf/src/title.rs")
    print("-" * 40)
    print("4. Author list detection (reject bad extractions)")
    print("   Pattern: ^[A-Z]{2,}\\s*,\\s*[A-Z]\\.\\s*,")
    print("   Add as validation after title extraction")
    print()

    print("Location: hallucinator-core/src/doi.rs (or hallucinator-pdf)")
    print("-" * 40)
    print("5. DOI cleaning (parentheses handling)")
    print("   - Allow parens in DOI pattern: [^\\s\\]>,]+ instead of [^\\s\\]>)}]+")
    print("   - Add clean_doi() to strip unbalanced trailing parens")
    print("   - Fixes: 10.1016/0021-9681(87)90171-8")
    print()

    print("Impact: ~79 titles with special characters in NDSS'26 dataset")
    print("  - 9 Greek letters")
    print("  - 8 broken diacritics")
    print("  - 47 dashes")
    print("  - 9 accented chars (already handled)")
    print("  - 2 author-as-title")
    print()


# =============================================================================
# FIX 6: DOI Cleaning (Parentheses Handling)
# =============================================================================
# DOIs can legitimately contain parentheses, e.g., 10.1016/0021-9681(87)90171-8
# However, trailing unbalanced ')' are likely reference delimiters, not part of DOI.
#
# Issue: DOI regex patterns that exclude ')' will truncate these DOIs incorrectly.
# Fix: Allow parentheses in DOI patterns, then clean up unbalanced trailing parens.
#
# Examples:
#   10.1016/0021-9681(87)90171-8  -> keep as-is (balanced)
#   10.1016/0021-9681(87)90171-8) -> strip trailing ) (unbalanced)
#   (doi: 10.1016/0021-9681(87)90171-8) -> extract 10.1016/0021-9681(87)90171-8
#
# Location: hallucinator-core/src/doi.rs or hallucinator-pdf/src/doi.rs


def clean_doi(doi: str) -> str:
    """Clean a DOI string by removing trailing punctuation and unbalanced parentheses.

    DOIs can legitimately contain parentheses (e.g., 10.1016/0021-9681(87)90171-8),
    but trailing unbalanced ')' are likely reference delimiters, not part of the DOI.
    """
    # First, strip common trailing punctuation
    doi = doi.rstrip('.,;:')

    # Handle unbalanced parentheses at the end
    # If DOI ends with ')' and parens are unbalanced, strip trailing ')'
    while doi.endswith(')'):
        open_count = doi.count('(')
        close_count = doi.count(')')
        if close_count > open_count:
            doi = doi[:-1].rstrip('.,;:')
        else:
            break

    # Similarly for brackets and braces (less common but possible)
    while doi.endswith(']') and doi.count(']') > doi.count('['):
        doi = doi[:-1].rstrip('.,;:')
    while doi.endswith('}') and doi.count('}') > doi.count('{'):
        doi = doi[:-1].rstrip('.,;:')

    return doi


def test_clean_doi():
    """Test DOI cleaning with parentheses."""
    print("=" * 60)
    print("FIX 6: DOI Cleaning (Parentheses Handling)")
    print("=" * 60)

    test_cases = [
        # Balanced parentheses - should not strip
        ("10.1016/0021-9681(87)90171-8", "10.1016/0021-9681(87)90171-8"),
        # Unbalanced - trailing ) should be stripped
        ("10.1016/0021-9681(87)90171-8)", "10.1016/0021-9681(87)90171-8"),
        # Double unbalanced
        ("10.1016/0021-9681(87)90171-8))", "10.1016/0021-9681(87)90171-8"),
        # Trailing punctuation
        ("10.1016/0021-9681(87)90171-8.,", "10.1016/0021-9681(87)90171-8"),
        # Normal DOI (no parens)
        ("10.1145/3442381.3450048", "10.1145/3442381.3450048"),
        # Multiple balanced parens
        ("10.1000/test(1)(2)suffix", "10.1000/test(1)(2)suffix"),
        # Mixed punctuation and unbalanced
        ("10.1016/test(1).,)", "10.1016/test(1)"),
    ]

    for doi, expected in test_cases:
        result = clean_doi(doi)
        status = "OK" if result == expected else "FAIL"
        print(f"  {status}: '{doi}' -> '{result}'")
        if result != expected:
            print(f"       Expected: '{expected}'")

    print()


# Rust implementation pattern:
RUST_CLEAN_DOI = '''
// In doi.rs:

/// Clean a DOI string by removing trailing punctuation and unbalanced parentheses.
///
/// DOIs can legitimately contain parentheses (e.g., 10.1016/0021-9681(87)90171-8),
/// but trailing unbalanced ')' are likely reference delimiters, not part of the DOI.
fn clean_doi(doi: &str) -> String {
    let mut doi = doi.trim_end_matches(|c| c == '.' || c == ',' || c == ';' || c == ':');

    // Handle unbalanced parentheses at the end
    loop {
        if doi.ends_with(')') {
            let open_count = doi.matches('(').count();
            let close_count = doi.matches(')').count();
            if close_count > open_count {
                doi = &doi[..doi.len() - 1];
                doi = doi.trim_end_matches(|c| c == '.' || c == ',' || c == ';' || c == ':');
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Similarly for brackets
    loop {
        if doi.ends_with(']') && doi.matches(']').count() > doi.matches('[').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(|c| c == '.' || c == ',' || c == ';' || c == ':');
        } else {
            break;
        }
    }

    // Similarly for braces
    loop {
        if doi.ends_with('}') && doi.matches('}').count() > doi.matches('{').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(|c| c == '.' || c == ',' || c == ';' || c == ':');
        } else {
            break;
        }
    }

    doi.to_string()
}

// DOI extraction pattern - allow parentheses in DOI suffix:
// Old: r"10\\.\\d{4,}/[^\\s\\]>)}]+"
// New: r"10\\.\\d{4,}/[^\\s\\]>,]+"
'''


# =============================================================================
# MAIN
# =============================================================================

if __name__ == "__main__":
    test_greek_transliteration()
    test_separated_diacritics()
    test_math_symbols()
    test_author_list_detection()
    test_dash_normalization()
    test_combined_normalization()
    test_clean_doi()
    print_summary()

    print("=" * 60)
    print("All tests completed.")
    print()
    print("To port these fixes to Rust, update:")
    print("  - hallucinator-core/src/matching.rs (normalize_title)")
    print("  - hallucinator-pdf/src/title.rs (title validation)")
    print("=" * 60)
