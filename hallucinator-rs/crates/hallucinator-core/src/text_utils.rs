use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

/// Extract URLs from reference text.
///
/// Handles common PDF extraction artifacts:
/// - Broken URLs with spaces in "http://" (e.g., "http : //")
/// - Spaced punctuation in URLs (e.g., "www . example . org / path")
/// - Line breaks within URLs
/// - Trailing punctuation
///
/// Excludes:
/// - DOI URLs (already handled via extract_doi)
/// - Academic URLs (doi.org, arxiv.org, etc.)
pub fn extract_urls(text: &str) -> Vec<String> {
    // First, fix broken URL prefixes (common PDF extraction issue)
    // "http : //" or "ht tp://" or "https : / /" or "https : // " etc.
    // Note: Some PDFs even have spaces between the slashes: "https : / /"
    // The trailing \s* consumes any space between the slashes and the domain
    static BROKEN_PREFIX: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)ht\s*tp\s*(s?)\s*:\s*/\s*/\s*").unwrap());
    let text_fixed = BROKEN_PREFIX.replace_all(text, "http$1://");

    // Fix spaced punctuation in URL regions: " . " → "." and " / " → "/"
    // This handles PDFs that add spaces around all punctuation.
    // We apply this aggressively after the protocol is fixed.
    let text_fixed = fix_spaced_url_punctuation(&text_fixed);

    // Fix URLs split across lines - multiple patterns:
    //
    // Pattern 1: Protocol split after colon: "https:\n//www.example.com"
    static PROTOCOL_SPLIT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(https?):\s*\n\s*(//[^\s\]>]+)").unwrap());
    let text_fixed = PROTOCOL_SPLIT.replace_all(&text_fixed, "$1:$2");

    // Pattern 2: Domain split mid-word: "https://www.pytho\nn.org" or "https://www.julien\nverneaut.com"
    // Match URL followed by newline and continuation that looks like domain/path (starts with lowercase letter)
    static DOMAIN_SPLIT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(https?://[^\s\]>\n]+)\s*\n\s*([a-z][^\s\]>]*)").unwrap());
    let text_fixed = DOMAIN_SPLIT.replace_all(&text_fixed, "$1$2");

    // Pattern 3: Path continuation: URL continues with /path after newline
    static PATH_SPLIT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(https?://[^\s\]>]+)\s*\n\s*(/[^\s\]>]*)").unwrap());
    let text_fixed = PATH_SPLIT.replace_all(&text_fixed, "$1$2");

    // URL regex that captures common URL patterns.
    //
    // Excludes `[` as well as `]`: when a PDF collapses whitespace between
    // consecutive bibliography entries, the next entry's `[N]` marker glues
    // onto the tail of the current URL ("url/page[42"). `]` was already
    // excluded; `[` needs to be too. Real URLs practically never contain a
    // literal `[` outside percent-encoding, so this is safe.
    static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?://[^\s\[\]>\)\},]+").unwrap());

    // Academic domains to exclude (these are handled by dedicated backends)
    static ACADEMIC_DOMAINS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(doi\.org|arxiv\.org|acm\.org|ieee\.org|usenix\.org|semanticscholar\.org|dblp\.org|aclanthology\.org|openreview\.net|neurips\.cc|proceedings\.mlr\.press)").unwrap()
    });

    let mut urls = Vec::new();
    let mut seen = HashSet::new();

    for m in URL_RE.find_iter(&text_fixed) {
        let mut url = m.as_str().to_string();

        // Strip backslash artifacts: LaTeX `\-` soft-hyphen escapes sometimes
        // leak into the PDF text layer as literal backslashes (seen on NDSS
        // 2026 f700 ref 1: "VM\-Escapes.pdf"). Backslash is never valid inside
        // a URL, so dropping it restores the original path.
        if url.contains('\\') {
            url = url.replace('\\', "");
        }

        // Clean trailing punctuation (common in citations). Includes the
        // typographic closing/opening quotes (U+201C–201D, U+2018–2019)
        // so that web citations like `...microsoft/SEAL.\u{201D}` don't
        // URL-encode the quote into the path and 404 the URL Check
        // (NDSS 2026 f182 refs 56–61).
        url = url
            .trim_end_matches([
                '.', ',', ';', ':', ')', ']', '}', '"', '\'', '\u{201C}', '\u{201D}', '\u{2018}',
                '\u{2019}',
            ])
            .to_string();

        // Skip academic URLs (handled by dedicated backends)
        if ACADEMIC_DOMAINS.is_match(&url) {
            continue;
        }

        // Skip DOI URLs (already extracted separately)
        if url.contains("doi.org") {
            continue;
        }

        // Deduplicate
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }

    urls
}

/// Fix spaced punctuation within URL regions.
///
/// PDFs sometimes add spaces around punctuation, producing URLs like:
/// `https://www . example . org / path / to / file`
///
/// This function finds URL regions (starting with `https://`) and removes
/// spaces around `.` and `/` within them.
fn fix_spaced_url_punctuation(text: &str) -> String {
    // Find URL-like regions and fix spacing within them
    static URL_REGION: Lazy<Regex> = Lazy::new(|| {
        // Match https:// followed by characters that could be URL parts (including spaces around punctuation)
        // Exclude () to avoid capturing "(visited on...)" annotations.
        // Include U+223C (TILDE OPERATOR, ∼) because some PDF fonts render
        // the URL tilde `~` as ∼, so `~user/path` ends up as `∼user/path`.
        // Without this, URL_REGION breaks at ∼ and the filename-underscore
        // recovery below never runs on academic URLs like www.*.edu/∼user/.
        Regex::new(r"https?://[\w\s.\-/~∼:@!$&'+,;=%?#\[\]]+").unwrap()
    });

    let mut result = text.to_string();

    // Process each potential URL region
    for m in URL_REGION.find_iter(text) {
        let region = m.as_str();
        let fixed = fix_url_spacing(region);
        if fixed != region {
            result = result.replace(region, &fixed);
        }
    }

    result
}

/// Fix spacing within a single URL region.
fn fix_url_spacing(url_region: &str) -> String {
    let mut result = url_region.to_string();

    // Rejoin host-internal whitespace: `https://multim edia.3m.com` →
    // `https://multimedia.3m.com`. Real hostnames never contain a
    // space, so when PDF extraction splits a hostname mid-token
    // (NDSS 2026 f1059 ref 28 raw is `https://multim\nedia.3m.com/...`
    // because the domain straddled a line break), the whitespace is
    // always an artifact. Fires only when the left fragment is a
    // single dotless word-token and the right fragment looks like a
    // normal domain ending (`word.tld`). If the left already contained
    // a dot, the regex engine stops `[\w\-]+` at the dot and `\s+`
    // fails — so complete-domain + narrative shapes like
    // `https://example.com See more at foo.bar` don't match. Must run
    // before `SPACED_DOT`, which would otherwise consume the `.tld`
    // dot and lose the anchor we rely on.
    static HOST_SPACE_FIX: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(https?://[\w\-]+)\s+([\w\-]+\.[\w\-.]+)").unwrap());
    result = HOST_SPACE_FIX.replace_all(&result, "$1$2").to_string();

    // Remove spaces around dots inside a URL region: " . " → "."
    //
    // Only fires when the character after the dot is lowercase (or a
    // digit). URL-internal dots — domain labels (`.com`, `.cmu`,
    // `.edu/path`), filename extensions (`.pdf`), versioned paths
    // (`v1.2.3`) — are overwhelmingly followed by lowercase or digits in
    // real PDFs. A dot followed by an uppercase letter almost always
    // signals a sentence boundary where URL_REGION's greedy match has
    // extended past the URL's true end and into narrative text. Real case:
    //
    //   "...bluesky_downloader. GitHub repository" (bluesky-blocks-paper ref 3)
    //
    // Previously SPACED_DOT collapsed `r. G` → `r.G`, so URL_RE picked up
    // `https://github.com/mrd0ll4r/bluesky_downloader.GitHub` and URL Check
    // got a 404 on what is otherwise a perfectly valid repo URL. Requiring
    // lowercase-or-digit after the dot lets `cs. cmu.edu` (lowercase `c`)
    // still collapse while leaving sentence-ending `downloader. GitHub`
    // (uppercase `G`) alone.
    static SPACED_DOT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)\s*\.\s*([a-z0-9])").unwrap());
    result = SPACED_DOT.replace_all(&result, "$1.$2").to_string();

    // Strip whitespace immediately before a `/` inside a URL region.
    // PDFs frequently render every path separator as `word / word`;
    // after this rule fires, every `/` is glued to the preceding
    // token and the only remaining slash-adjacent whitespace is on
    // the right side, handled by SLASH_SPACE below. Real example —
    // NDSS 2026 f106 ref 23:
    //
    //   `https://www.dennemeyer.com/ fileadmin / a / media - library
    //    / reports / cybersecurity in mobility 2024 - 05.pdf`
    //
    // A previous attempt relied on `SPACED_SLASH` (`(\w)\s*/\s*(\w)`),
    // but `replace_all` consumed the trailing `\w` of each match, so
    // alternating boundaries like `.../a /` stayed unfixed on every
    // pass (looping didn't help either — each iteration consumed the
    // same boundary and missed the same neighbor). Splitting the
    // job into two non-overlapping rules — "strip before" (here) and
    // "strip after" (SLASH_SPACE) — sidesteps the consumption issue
    // because neither rule needs the `\w` of the other side.
    static SPACE_BEFORE_SLASH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)\s+/").unwrap());
    result = SPACE_BEFORE_SLASH.replace_all(&result, "$1/").to_string();

    // Remove spaces around slashes when between URL parts: " / " or "/ " → "/"
    // Only fix when the slash is between alphanumeric/URL-like characters
    // This avoids joining "url/ (visited" → "url/(visited"
    // Kept as a belt-and-braces follow-up to SPACE_BEFORE_SLASH: the
    // combination of SPACE_BEFORE_SLASH → SPACED_SLASH → SLASH_SPACE
    // now normalises every shape we've seen in PDF extraction.
    static SPACED_SLASH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)\s*/\s*(\w)").unwrap());
    result = SPACED_SLASH.replace_all(&result, "$1/$2").to_string();

    // Also handle slash at end of a path segment followed by space+continuation:
    // "org/ wiki" → "org/wiki" (space only after slash, not before)
    static SLASH_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"/\s+(\w)").unwrap());
    result = SLASH_SPACE.replace_all(&result, "/$1").to_string();

    // Remove spaces around hyphens in paths: "call- for- papers" → "call-for-papers"
    static SPACED_HYPHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)\s*-\s*(\w)").unwrap());
    result = SPACED_HYPHEN.replace_all(&result, "$1-$2").to_string();

    // Collapse whitespace immediately before a fragment: PDFs sometimes
    // wrap a line after the trailing `/` of a path and before the `#`
    // of a URL fragment. Example from NDSS 2026 s1381 ref 20 where the
    // raw text is `…/docker/container/run/\n#example-join-…` — neither
    // SLASH_SPACE (`/\s+\w`, `#` isn't a word char) nor PATH_SPLIT
    // (requires the continuation to start with `/`) absorbs this shape.
    //
    // Uses `\s+` so both horizontal whitespace and newlines collapse,
    // because the rule fires only inside URL_REGION matches (narrative
    // `#hashtag` after a URL is vanishingly rare in academic citations
    // — fragments are the overwhelming case).
    static SPACE_BEFORE_HASH: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+#").unwrap());
    result = SPACE_BEFORE_HASH.replace_all(&result, "#").to_string();

    // Join middle-segment spaces as underscores inside URL paths.
    //
    // A path segment wrapped between two `/`s that contains only word-like
    // tokens separated by whitespace is treated as a filename where PDF
    // rendering lost the underscores. Real NDSS 2026 examples:
    //
    //   .../kernelctf/CVE-2024-26581 lts cos mitigation/docs/exploit.md
    //     → .../kernelctf/CVE-2024-26581_lts_cos_mitigation/docs/exploit.md
    //   .../help.apple.com/pdf/security/en US/apple-platform-security-guide.pdf
    //     → .../help.apple.com/pdf/security/en_US/apple-platform-security-guide.pdf
    //   .../docs.zephyrproject.org/.../native sim/doc/index.html
    //     → .../docs.zephyrproject.org/.../native_sim/doc/index.html
    //
    // Safety comes from the `/` on both sides and the token class
    // `[A-Za-z0-9\-]+`, which excludes URL-structural punctuation (`:`,
    // `.`, `?`, `#`). That means narrative like `"/repo See https"` can't
    // match: the `https` token would need `/` after it, but the next char
    // is `:`. Observed false-positive rate on 957 URL-bearing references
    // across the NDSS 2026 corpus: zero.
    //
    // Applied in a fixed-point loop because consuming the trailing `/` on
    // the first match blocks overlap on shapes like `/foo bar/baz qux/`
    // (first pass rewrites `/foo_bar/`; the second pass catches
    // `/baz_qux/`). Typical URL has <10 passes; terminates fast.
    static INTERNAL_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    static MIDDLE_SPACED_SEGMENT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"/([A-Za-z0-9\-]+(?:\s+[A-Za-z0-9\-]+)+)/").unwrap());
    loop {
        let next = MIDDLE_SPACED_SEGMENT
            .replace_all(&result, |caps: &regex::Captures| {
                let rebuilt = INTERNAL_WS.replace_all(&caps[1], "_");
                format!("/{}/", rebuilt)
            })
            .to_string();
        if next == result {
            break;
        }
        result = next;
    }

    // Restore underscores lost by PDF font rendering in a trailing filename.
    //
    // Some PDFs render `_` inside a URL path as literal whitespace, so
    // `fuzzing/cjson_read_fuzzer.c` comes through as `fuzzing/cjson read
    // fuzzer.c`. To recover without touching narrative text that might
    // follow a URL, the rewrite is gated on:
    //
    //   * the match starting at a `/` (inside a path segment),
    //   * the token sequence ending in a short file extension
    //     (`.[A-Za-z]{1,6}`), and
    //   * the trailer being end-of-region OR a typical citation delimiter
    //     (`,`, `;`, `)`) — i.e. what sits immediately after a URL in a
    //     bibliography entry. This is the main safety: the pattern only
    //     rewrites a filename-like suffix at the tail of the URL, not in
    //     the middle of narrative.
    //
    // The anchor used to be `\s*$` (strict end-of-region), but URL_REGION's
    // character class includes `,` and `\s`, so it happily extends into
    // `, 2010` tails on real citations like
    // `https://www.dgp.toronto.edu/∼ravin/papers/chi2010 tabbedbrowsing.pdf, 2010`
    // (NDSS 2026 f328 ref [73]). A lookahead that accepts the citation
    // delimiters preserves the safety guarantee (narrative like
    // `... ref.txt Section 3 has` still does not match) while letting the
    // rule fire on real references.
    // The Rust regex crate does not support lookaround, so the trailer is
    // captured in group 2 and restored verbatim in the replacement closure.
    //
    // The token class is `[A-Za-z0-9\-]+` rather than `[A-Za-z0-9]+` so
    // that filenames with hyphens — e.g.,
    // `M3AAWG Hosting Abuse BCPs-2015-03.pdf` (NDSS 2026 f468 ref 2) or
    // `24-27349-006 Matter-1.4-Core-Specification.pdf` (f94 ref 38) — match.
    // The preceding whitespace still has to be the only separator between
    // tokens (not another `-`), so existing hyphenated tails that already
    // parse correctly stay untouched.
    // Trailer accepts `.` too, not just `,;)`. PDF bibliographies often
    // terminate a URL with a narrative period — `...cybersecurity in
    // mobility 2024-05.pdf.` (NDSS 2026 f106 ref 23). Without `.` in
    // the trailer, the rule couldn't fire on those shapes because the
    // filename's own extension would anchor `.pdf` and the following
    // narrative `.` wouldn't satisfy the trailer. `.` is safe here
    // because filenames WITHOUT internal spaces can never reach this
    // rule — the `(?:\s+[A-Za-z0-9\-]+)+` segment requires at least
    // one internal space, so `/file.pdf.Next-sentence` won't match.
    static FILENAME_LOST_UNDERSCORES: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"/([A-Za-z0-9\-]+(?:\s+[A-Za-z0-9\-]+)+\.[A-Za-z0-9]{1,6})(\s*(?:$|[.,;)]))")
            .unwrap()
    });
    result = FILENAME_LOST_UNDERSCORES
        .replace(&result, |caps: &regex::Captures| {
            let rebuilt = INTERNAL_WS.replace_all(&caps[1], "_");
            format!("/{}{}", rebuilt, &caps[2])
        })
        .to_string();

    result
}

/// Strip unbalanced trailing parentheses, brackets, and braces from a DOI.
fn clean_doi(doi: &str) -> String {
    let mut doi = doi.trim_end_matches(['.', ',', ';', ':']);

    // Strip unbalanced trailing )
    loop {
        if doi.ends_with(')') && doi.matches(')').count() > doi.matches('(').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(['.', ',', ';', ':']);
        } else {
            break;
        }
    }

    // Strip unbalanced trailing ]
    loop {
        if doi.ends_with(']') && doi.matches(']').count() > doi.matches('[').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(['.', ',', ';', ':']);
        } else {
            break;
        }
    }

    // Strip unbalanced trailing }
    loop {
        if doi.ends_with('}') && doi.matches('}').count() > doi.matches('{').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(['.', ',', ';', ':']);
        } else {
            break;
        }
    }

    doi.to_string()
}

/// Extract DOI from reference text.
///
/// Handles formats like:
/// - `10.1234/example`
/// - `doi:10.1234/example`
/// - `https://doi.org/10.1234/example`
/// - `http://dx.doi.org/10.1234/example`
///
/// Also handles DOIs split across lines (common in PDFs) and DOIs
/// containing parentheses (e.g., `10.1016/0021-9681(87)90171-8`).
pub fn extract_doi(text: &str) -> Option<String> {
    // Fix DOIs that are split across lines

    // Pattern 1: DOI ending with period + newline + 3+ digits
    static FIX1: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(10\.\d{4,}/[^\s\]>,]+\.)\s*\n\s*(\d{3,})").unwrap());
    let text_fixed = FIX1.replace_all(text, "$1$2");

    // Pattern 1b: DOI ending with digits + newline + DOI continuation
    static FIX1B: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(10\.\d{4,}/[^\s\]>,]+\d)\s*\n\s*(\d+(?:\.\d+)*)").unwrap());
    let text_fixed = FIX1B.replace_all(&text_fixed, "$1$2");

    // Pattern 2: DOI ending with dash + newline + continuation
    static FIX2: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(10\.\d{4,}/[^\s\]>,]+-)\s*\n\s*(\S+)").unwrap());
    let text_fixed = FIX2.replace_all(&text_fixed, "$1$2");

    // Pattern 3: URL split across lines (period variant)
    static FIX3: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(https?://(?:dx\.)?doi\.org/10\.\d{4,}/[^\s\]>,]+\.)\s*\n\s*(\d+)")
            .unwrap()
    });
    let text_fixed = FIX3.replace_all(&text_fixed, "$1$2");

    // Pattern 3b: URL split mid-number
    static FIX3B: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)(https?://(?:dx\.)?doi\.org/10\.\d{4,}/[^\s\]>,]+\d)\s*\n\s*(\d[^\s\]>,]*)",
        )
        .unwrap()
    });
    let text_fixed = FIX3B.replace_all(&text_fixed, "$1$2");

    // Priority 1: Extract from URL format (most reliable)
    static URL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)https?://(?:dx\.)?doi\.org/(10\.\d{4,}/[^\s\]>},]+)").unwrap()
    });
    if let Some(caps) = URL_RE.captures(&text_fixed) {
        let doi = caps.get(1).unwrap().as_str();
        return Some(clean_doi(doi));
    }

    // Priority 2: DOI pattern without URL prefix
    static DOI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"10\.\d{4,}/[^\s\]>},]+").unwrap());
    if let Some(m) = DOI_RE.find(&text_fixed) {
        let doi = m.as_str();
        return Some(clean_doi(doi));
    }

    None
}

/// Extract arXiv ID from reference text.
///
/// Handles formats like:
/// - `arXiv:2301.12345`
/// - `arXiv:2301.12345v1`
/// - `arxiv.org/abs/2301.12345`
/// - `arXiv:hep-th/9901001` (old format)
/// - `10.48550/arXiv.2301.12345` (DOI format)
/// - `CoRR, abs/2301.12345` (CoRR format)
///
/// Also handles IDs split across lines.
pub fn extract_arxiv_id(text: &str) -> Option<String> {
    // Fix IDs split across lines
    static FIX1: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(arXiv:\d{4}\.)\s*\n\s*(\d+)").unwrap());
    let text_fixed = FIX1.replace_all(text, "$1$2");

    static FIX2: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(arxiv\.org/abs/\d{4}\.)\s*\n\s*(\d+)").unwrap());
    let text_fixed = FIX2.replace_all(&text_fixed, "$1$2");

    // arXiv DOI format: 10.48550/arXiv.YYMM.NNNNN (newer DOI format for arXiv papers)
    static ARXIV_DOI_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)10\.48550/arXiv\.(\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = ARXIV_DOI_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // New format: YYMM.NNNNN (with optional version)
    static NEW_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arXiv[:\s]+(\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = NEW_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // URL format: arxiv.org/abs/YYMM.NNNNN
    static URL_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arxiv\.org/abs/(\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = URL_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // CoRR format: "CoRR, abs/YYMM.NNNNN" or "CoRR abs/YYMM.NNNNN"
    static CORR_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)CoRR[,:\s]+abs[/:](\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = CORR_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // Old format: category/YYMMNNN (e.g., hep-th/9901001)
    static OLD_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arXiv[:\s]+([a-z-]+/\d{7}(?:v\d+)?)").unwrap());
    if let Some(caps) = OLD_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // URL old format
    static URL_OLD_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arxiv\.org/abs/([a-z-]+/\d{7}(?:v\d+)?)").unwrap());
    if let Some(caps) = URL_OLD_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // CoRR old format: "CoRR, abs/category/YYMMNNN"
    static CORR_OLD_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)CoRR[,:\s]+abs[/:]([a-z-]+/\d{7}(?:v\d+)?)").unwrap());
    if let Some(caps) = CORR_OLD_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    None
}

/// Common words to skip when building search queries.
static STOP_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "a", "an", "the", "of", "and", "or", "for", "to", "in", "on", "with", "by",
    ]
    .into_iter()
    .collect()
});

/// Extract `n` significant words from a title for building search queries.
///
/// Skips stop words and very short words, but keeps short alphanumeric
/// terms like "L2", "3D", "AI", "5G".
pub fn get_query_words(title: &str, n: usize) -> Vec<String> {
    // Strip BibTeX capitalization braces: {BERT} → BERT, {M}ixup → Mixup
    let title = title.replace(['{', '}'], "");

    static WORD_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[a-zA-Z0-9]+(?:['\u{2019}\u{2018}\-][a-zA-Z0-9]+)*[?!]?").unwrap()
    });

    let all_words: Vec<&str> = WORD_RE.find_iter(&title).map(|m| m.as_str()).collect();

    let significant: Vec<&str> = all_words
        .iter()
        .copied()
        .filter(|w| is_significant(w))
        .collect();

    if significant.len() >= 3 {
        significant.into_iter().take(n).map(String::from).collect()
    } else {
        all_words.into_iter().take(n).map(String::from).collect()
    }
}

fn is_significant(w: &str) -> bool {
    // Strip trailing ?! before checking (e.g., "important?" → "important")
    let w = w.trim_end_matches(['?', '!']);
    if STOP_WORDS.contains(w.to_lowercase().as_str()) {
        return false;
    }
    if w.len() >= 3 {
        return true;
    }
    // Keep short words that mix letters and digits (technical terms)
    let has_letter = w.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = w.chars().any(|c| c.is_ascii_digit());
    has_letter && has_digit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_doi_basic() {
        assert_eq!(
            extract_doi("doi: 10.1145/3442381.3450048"),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_url() {
        assert_eq!(
            extract_doi("https://doi.org/10.1145/3442381.3450048"),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_split_across_lines() {
        assert_eq!(
            extract_doi("10.1145/3442381.\n3450048"),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_trailing_punct() {
        assert_eq!(
            extract_doi("10.1145/3442381.3450048."),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_none() {
        assert_eq!(extract_doi("No DOI here"), None);
    }

    #[test]
    fn test_extract_doi_with_balanced_parentheses() {
        assert_eq!(
            extract_doi("10.1016/0021-9681(87)90171-8"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_extract_doi_with_unbalanced_trailing_paren() {
        assert_eq!(
            extract_doi("(doi: 10.1016/0021-9681(87)90171-8)"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_extract_doi_url_with_parentheses() {
        assert_eq!(
            extract_doi("https://doi.org/10.1016/0021-9681(87)90171-8"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_extract_doi_url_with_unbalanced_paren() {
        assert_eq!(
            extract_doi("(https://doi.org/10.1016/0021-9681(87)90171-8)"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_clean_doi_no_parens() {
        assert_eq!(
            clean_doi("10.1145/3442381.3450048"),
            "10.1145/3442381.3450048"
        );
    }

    #[test]
    fn test_clean_doi_balanced_parens() {
        assert_eq!(
            clean_doi("10.1016/0021-9681(87)90171-8"),
            "10.1016/0021-9681(87)90171-8"
        );
    }

    #[test]
    fn test_clean_doi_unbalanced_trailing_paren() {
        assert_eq!(
            clean_doi("10.1016/0021-9681(87)90171-8)"),
            "10.1016/0021-9681(87)90171-8"
        );
    }

    #[test]
    fn test_clean_doi_unbalanced_trailing_bracket() {
        assert_eq!(clean_doi("10.1234/test[1]extra]"), "10.1234/test[1]extra");
    }

    #[test]
    fn test_clean_doi_trailing_punct_after_paren() {
        assert_eq!(
            clean_doi("10.1016/0021-9681(87)90171-8)."),
            "10.1016/0021-9681(87)90171-8"
        );
    }

    #[test]
    fn test_extract_arxiv_new_format() {
        assert_eq!(
            extract_arxiv_id("arXiv:2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_with_version() {
        assert_eq!(
            extract_arxiv_id("arXiv:2301.12345v2"),
            Some("2301.12345v2".into())
        );
    }

    #[test]
    fn test_extract_arxiv_url() {
        assert_eq!(
            extract_arxiv_id("arxiv.org/abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_old_format() {
        assert_eq!(
            extract_arxiv_id("arXiv:hep-th/9901001"),
            Some("hep-th/9901001".into())
        );
    }

    #[test]
    fn test_extract_arxiv_split() {
        assert_eq!(
            extract_arxiv_id("arXiv:2301.\n12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_none() {
        assert_eq!(extract_arxiv_id("No arXiv here"), None);
    }

    #[test]
    fn test_extract_arxiv_doi_format() {
        // arXiv DOI format: 10.48550/arXiv.YYMM.NNNNN
        assert_eq!(
            extract_arxiv_id("10.48550/arXiv.2510.14861"),
            Some("2510.14861".into())
        );
        assert_eq!(
            extract_arxiv_id("https://doi.org/10.48550/arXiv.2301.12345v2"),
            Some("2301.12345v2".into())
        );
    }

    #[test]
    fn test_extract_arxiv_corr_format() {
        // CoRR format: CoRR, abs/YYMM.NNNNN
        assert_eq!(
            extract_arxiv_id("CoRR, abs/2503.19786"),
            Some("2503.19786".into())
        );
        assert_eq!(
            extract_arxiv_id("CoRR abs/2301.12345v1"),
            Some("2301.12345v1".into())
        );
        // With colon separator
        assert_eq!(
            extract_arxiv_id("CoRR: abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_corr_old_format() {
        // CoRR old format: CoRR, abs/category/YYMMNNN
        assert_eq!(
            extract_arxiv_id("CoRR, abs/cs/0701001"),
            Some("cs/0701001".into())
        );
        assert_eq!(
            extract_arxiv_id("CoRR abs/hep-th/9901001v2"),
            Some("hep-th/9901001v2".into())
        );
    }

    #[test]
    fn test_get_query_words_basic() {
        let words = get_query_words("Detecting Hallucinated References in Academic Papers", 6);
        assert_eq!(words.len(), 5); // "in" is a stop word, so only 5 significant words
        assert!(!words.contains(&"in".to_string()));
    }

    #[test]
    fn test_get_query_words_technical() {
        let words = get_query_words("L2 Regularization for 3D Models", 6);
        assert!(words.contains(&"L2".to_string()));
        assert!(words.contains(&"3D".to_string()));
    }

    #[test]
    fn test_get_query_words_short_title() {
        let words = get_query_words("A B C", 6);
        // Less than 3 significant words, falls back to all_words
        assert_eq!(words, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_get_query_words_bibtex_braces() {
        let words = get_query_words("{BERT}: Pre-training of Deep Bidirectional Transformers", 6);
        assert!(words.contains(&"BERT".to_string()));
    }

    #[test]
    fn test_get_query_words_bibtex_partial_braces() {
        let words = get_query_words("{M}ixup Training for Robust Models", 6);
        assert!(words.contains(&"Mixup".to_string()));
    }

    #[test]
    fn test_get_query_words_bibtex_hyphenated() {
        let words = get_query_words("{COVID}-19 Detection with Deep Learning", 6);
        assert!(words.contains(&"COVID-19".to_string()));
    }

    // ── extract_urls tests ──

    #[test]
    fn test_extract_urls_basic() {
        let urls = extract_urls("See https://github.com/user/repo for details.");
        assert_eq!(urls, vec!["https://github.com/user/repo"]);
    }

    #[test]
    fn test_extract_urls_multiple() {
        let urls = extract_urls(
            "Code at https://github.com/user/repo and docs at https://example.com/docs",
        );
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://github.com/user/repo".to_string()));
        assert!(urls.contains(&"https://example.com/docs".to_string()));
    }

    #[test]
    fn test_extract_urls_trailing_punct() {
        let urls = extract_urls("Visit https://github.com/repo.");
        assert_eq!(urls, vec!["https://github.com/repo"]);

        let urls2 = extract_urls("(see https://example.com/page)");
        assert_eq!(urls2, vec!["https://example.com/page"]);
    }

    #[test]
    fn test_extract_urls_broken_prefix() {
        // PDF extraction sometimes adds spaces in "http://"
        let urls = extract_urls("See http : //github.com/repo for code.");
        assert_eq!(urls, vec!["http://github.com/repo"]);

        let urls2 = extract_urls("Visit ht tp://example.com/page.");
        assert_eq!(urls2, vec!["http://example.com/page"]);
    }

    #[test]
    fn test_extract_urls_excludes_academic() {
        // Academic URLs should be excluded (handled by dedicated backends)
        let urls = extract_urls(
            "Paper at https://arxiv.org/abs/2301.12345 and code at https://github.com/user/repo",
        );
        assert_eq!(urls, vec!["https://github.com/user/repo"]);

        // doi.org should be excluded
        let urls2 = extract_urls("https://doi.org/10.1234/test");
        assert!(urls2.is_empty());

        // Other academic domains
        let urls3 = extract_urls("https://semanticscholar.org/paper/123 https://dblp.org/rec/test");
        assert!(urls3.is_empty());
    }

    #[test]
    fn test_extract_urls_deduplicates() {
        let urls = extract_urls("https://github.com/repo and again https://github.com/repo");
        assert_eq!(urls.len(), 1);
    }

    #[test]
    fn test_extract_urls_none() {
        let urls = extract_urls("No URLs in this text.");
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_urls_strips_trailing_curly_quotes() {
        // NDSS 2026 f182 refs 56–61 pattern: the reference ends with a
        // URL followed by a closing curly quote (`.\u{201D}`). Without
        // stripping, that byte sequence URL-encodes into the path
        // (`...%E2%80%9D`) and URL Check 404s the legitimate repo.
        let text = "M. Smith, \u{201C}Title\u{201D}, https://github.com/microsoft/SEAL.\u{201D}";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://github.com/microsoft/SEAL".to_string()]);

        // Leading stray open-quote (U+201C) at the head would be part
        // of narrative, not a URL, so only the trailing case matters —
        // but the strip list must cover both marks plus the single
        // curly-quote pair for robustness.
        let text2 = "See https://example.org/page\u{2019}";
        let urls2 = extract_urls(text2);
        assert_eq!(urls2, vec!["https://example.org/page".to_string()]);
    }

    #[test]
    fn test_extract_urls_rejoins_host_internal_space() {
        // NDSS 2026 f1059 ref 28: PDF extraction split the hostname
        // mid-token across a line break (`multim\nedia.3m.com`). The
        // host-space fix should rejoin before URL_RE truncates at the
        // whitespace.
        let text = "B. Honan, Visual Data Security White Paper, July 2012, https://multim edia.3m.com/mws/media/950026O/secure-white-paper.pdf";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 1, "got {:?}", urls);
        assert!(
            urls[0].starts_with("https://multimedia.3m.com/"),
            "expected rejoined hostname, got {:?}",
            urls[0]
        );
    }

    #[test]
    fn test_extract_urls_multi_slash_path_fully_collapsed() {
        // NDSS 2026 f106 ref 23: raw PDF renders every path separator
        // as `word / word`. A single `SPACED_SLASH.replace_all` pass
        // consumes the trailing word of each match, so alternating
        // boundaries (`/a /`) stay unfixed and URL_RE truncates at
        // the first unfixed space. The fixed-point loop must keep
        // iterating until every slash boundary is tight.
        let text = "Source, Title, 2024, https://www.dennemeyer.com/ fileadmin / a / media - library / reports / cybersecurity in mobility 2024 - 05.pdf.";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 1, "got {:?}", urls);
        assert!(
            urls[0].contains("/fileadmin/a/media-library/reports/"),
            "path should be fully joined, got {:?}",
            urls[0]
        );
        // Filename-space recovery with trailing narrative period also
        // kicks in: the internal-space filename becomes underscored.
        assert!(
            urls[0].ends_with("cybersecurity_in_mobility_2024-05.pdf"),
            "filename underscores should be restored, got {:?}",
            urls[0]
        );
    }

    #[test]
    fn test_extract_urls_filename_trailing_narrative_period() {
        // A multi-word filename URL that ends with a narrative period
        // (as in `.pdf.` at end of a sentence) must still trigger the
        // filename-underscore-recovery rule. Previously the trailer
        // only accepted `,`, `;`, `)`, or end-of-string, so a trailing
        // `.` blocked the match.
        let text = "See https://example.org/path/my report final.pdf. End of citation.";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 1, "got {:?}", urls);
        assert!(
            urls[0].ends_with("my_report_final.pdf"),
            "got {:?}",
            urls[0]
        );
    }

    #[test]
    fn test_extract_urls_host_space_fix_leaves_narrative_alone() {
        // A complete hostname (`example.com`) followed by narrative
        // must NOT be joined into the next word — `[\w\-]+` stops at
        // the `.`, `\s+` then fails because the next char is `.`, not
        // whitespace, so the regex can't anchor. Protects against
        // collapsing `example.com See more at github.com` into one
        // mangled URL.
        let text = "See https://example.com Read more at https://github.com/user";
        let urls = extract_urls(text);
        // Both URLs extracted independently, neither mangled.
        assert!(
            urls.contains(&"https://example.com".to_string()),
            "got {:?}",
            urls
        );
        assert!(
            urls.contains(&"https://github.com/user".to_string()),
            "got {:?}",
            urls
        );
    }

    #[test]
    fn test_extract_urls_with_path() {
        let urls = extract_urls("https://github.com/user/repo/blob/main/README.md");
        assert_eq!(
            urls,
            vec!["https://github.com/user/repo/blob/main/README.md"]
        );
    }

    #[test]
    fn test_extract_urls_edudata_case() {
        // Real-world case: citation with GitHub URL
        let text = "BigData Lab @USTC. 2021. EduData. Online, accessed February 5, 2025. https://github.com/bigdata-ustc/EduData";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://github.com/bigdata-ustc/EduData"]);
    }

    #[test]
    fn test_extract_urls_pdf_broken_colon_space() {
        // PDF extraction often produces "https: //" with space after colon
        let text = "Online. https: //www.example.org/page";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://www.example.org/page"]);
    }

    #[test]
    fn test_extract_urls_space_in_domain() {
        // PDF extraction can split domain across lines creating spaces
        // Now fixed! Spaces within URL regions are removed
        let text = "See https://www.cs. cmu.edu/paper.pdf for details";
        let urls = extract_urls(text);
        // URL is now properly reconstructed
        assert_eq!(urls, vec!["https://www.cs.cmu.edu/paper.pdf"]);
    }

    #[test]
    fn test_extract_urls_real_pdf_cases() {
        // Real cases from 300_Transparency Practices.pdf
        let text1 = "https: / / cra . org / wp - content / uploads / 2024 / 07 / Report.pdf";
        let urls1 = extract_urls(text1);
        assert_eq!(
            urls1,
            vec!["https://cra.org/wp-content/uploads/2024/07/Report.pdf"]
        );

        // Space in path with hyphens
        let text2 =
            "https : / / www . usenix . org / conference/usenixsecurity25/call- for- papers";
        let urls2 = extract_urls(text2);
        // usenix.org is academic, so should be excluded
        assert!(urls2.is_empty());

        // chi2024 with space after domain
        let text3 = "https://chi2024. acm.org/2024/02/08/artifacts-at-chi-2024/";
        let urls3 = extract_urls(text3);
        // acm.org is academic, so should be excluded
        assert!(urls3.is_empty());

        // go-fair.org - not academic, should be extracted
        // Note: need space before (visited) for proper parsing
        let text4 = "URL: https://www.go-fair.org/fair-principles/ (visited on...)";
        let urls4 = extract_urls(text4);
        assert_eq!(urls4, vec!["https://www.go-fair.org/fair-principles/"]);

        // cos.io - not academic, should be extracted
        let text5 = "URL: https: //www.cos.io/initiatives/top-guidelines";
        let urls5 = extract_urls(text5);
        assert_eq!(urls5, vec!["https://www.cos.io/initiatives/top-guidelines"]);

        // icpsr.umich.edu - not academic (data repository), should be extracted
        let text6 = "URL: https://www.icpsr.umich.edu/files/deposit/ data.pdf";
        let urls6 = extract_urls(text6);
        // Note: space in path gets fixed
        assert_eq!(
            urls6,
            vec!["https://www.icpsr.umich.edu/files/deposit/data.pdf"]
        );
    }

    #[test]
    fn test_extract_urls_extreme_spacing() {
        // Extreme case: spaces between all URL parts (using non-academic domain)
        let text = "URL: https : / / www . github . com / user / repo";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://www.github.com/user/repo"]);

        // Also test academic domain - should be excluded (handled by dedicated backends)
        let text2 = "URL: https : / / www . acm . org / publications / policies/test";
        let urls2 = extract_urls(text2);
        assert!(
            urls2.is_empty(),
            "acm.org should be excluded as academic domain"
        );
    }

    #[test]
    fn test_extract_urls_space_after_domain() {
        // Space after domain, before path - now fixed!
        let text = "URL: https://www.sigsac.org/ ccs/CCS2024/call-for-papers.html";
        let urls = extract_urls(text);
        // Path is now properly joined
        assert_eq!(
            urls,
            vec!["https://www.sigsac.org/ccs/CCS2024/call-for-papers.html"]
        );
    }

    #[test]
    fn test_extract_urls_line_break_patterns() {
        // Pattern 1: Protocol split after colon
        let text1 = "[97] Wappalyzer. n.d.. Find Out. https:\n//www.wappalyzer.com Accessed";
        let urls1 = extract_urls(text1);
        assert_eq!(urls1, vec!["https://www.wappalyzer.com"]);

        // Pattern 2: Domain split mid-word
        let text2 =
            "[63] Python. 2025. Download Python. https://www.python.o\nrg/downloads Accessed";
        let urls2 = extract_urls(text2);
        assert_eq!(urls2, vec!["https://www.python.org/downloads"]);

        // Pattern 3: Domain split mid-word (longer domain)
        let text3 = "[96] Julien. n.d.. Reverse-Engineering. https://www.julien\nverneaut.com/en/experiments Accessed";
        let urls3 = extract_urls(text3);
        assert_eq!(urls3, vec!["https://www.julienverneaut.com/en/experiments"]);
    }

    // ── Filename-lost-underscores recovery (2026-s820-paper ref 21) ──

    #[test]
    fn test_extract_urls_filename_lost_underscores() {
        // Regression test for NDSS 2026 f168/s820 pattern: some PDF fonts
        // render `_` inside a URL path as literal whitespace, so the source
        // URL `.../fuzzing/cjson_read_fuzzer.c` comes through as
        // `.../fuzzing/cjson read fuzzer.c`. Combined with a line break after
        // `github.com/`, extract_urls previously truncated at the first
        // internal space. The post-fix pass should restore the trailing
        // filename so URL Check can verify the link.
        let text = r#""cjson read fuzzer.c." [Online]. Available: https://github.com/ DaveGamble/cJSON/blob/master/fuzzing/cjson read fuzzer.c"#;
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://github.com/DaveGamble/cJSON/blob/master/fuzzing/cjson_read_fuzzer.c"
                    .to_string()
            ]
        );
    }

    #[test]
    fn test_extract_urls_multi_word_filename() {
        // Same pattern with three internal spaces, all lost underscores.
        let text = "see https://example.com/path/very long file.py";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/path/very_long_file.py"]);
    }

    #[test]
    fn test_extract_urls_hyphenated_filename_lost_underscores() {
        // NDSS 2026 f468 ref 2 / f94 ref 38: filename that contains
        // hyphens AND had its internal underscores PDF-rendered as
        // spaces. Requires FILENAME_LOST_UNDERSCORES's token class to
        // include `-` (otherwise the hyphen breaks the greedy match
        // and the rule can't anchor on the `.pdf` tail).
        let text = "Available: https://www.m3aawg.org/sites/default/files/document/M3AAWG Hosting Abuse BCPs-2015-03.pdf";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://www.m3aawg.org/sites/default/files/document/M3AAWG_Hosting_Abuse_BCPs-2015-03.pdf"
            ]
        );
    }

    #[test]
    fn test_extract_urls_fragment_after_whitespace() {
        // NDSS 2026 s1381 ref 20: `.../run/\n#example-join-another-…`
        // where `#` sits immediately after a PDF line break. Neither
        // SLASH_SPACE (`\w` doesn't match `#`) nor PATH_SPLIT (needs
        // `/` continuation) closes the gap. SPACE_BEFORE_HASH glues
        // them so the fragment survives as part of the URL.
        let text = "docs: https://docs.docker.com/reference/cli/docker/container/run/ #example-join-another-containers-pid-namespace";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://docs.docker.com/reference/cli/docker/container/run/#example-join-another-containers-pid-namespace"
            ]
        );
    }

    #[test]
    fn test_extract_urls_fragment_after_newline() {
        // Newline variant of the same shape — SPACE_BEFORE_HASH uses
        // `\s+` so newlines collapse too.
        let text = "docs: https://docs.docker.com/reference/cli/docker/container/run/\n#example-join-another-containers-pid-namespace";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://docs.docker.com/reference/cli/docker/container/run/#example-join-another-containers-pid-namespace"
            ]
        );
    }

    #[test]
    fn test_extract_urls_narrative_after_short_url_untouched() {
        // Guard: a URL with no file-extension suffix followed by narrative
        // text must not have its trailing words collapsed into the path.
        // Here `"foo for details"` has no `.ext`, so the restoration rule
        // does not fire; extract_urls still stops at the first space.
        let text = "Code at https://github.com/user/repo for details.";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://github.com/user/repo"]);
    }

    #[test]
    fn test_extract_urls_url_then_filename_elsewhere_not_mangled_by_new_rule() {
        // Guard: the filename-lost-underscores rule must NOT merge trailing
        // narrative into the URL. The rule requires the match to start at
        // `/`; `page.` contains no later slash that the rewrite could
        // anchor on, so the new rule does not fire.
        //
        // (Separately, the pre-existing SPACED_DOT rule collapses
        // `page. Download` to `page.Download`; this test only asserts the
        // NEW rule behaves, not the pre-existing space-around-dot rule.)
        let text = "See https://example.com/page. Download file.pdf.";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 1);
        let url = &urls[0];
        // Critical post-condition: "file.pdf" must NOT have been pulled
        // into the URL as "file_pdf" or similar; the new rule stays off.
        assert!(!url.contains("_pdf"), "unexpected mangle: {}", url);
        assert!(
            !url.contains("file.pdf"),
            "trailing narrative pulled in: {}",
            url
        );
    }

    #[test]
    fn test_extract_urls_nested_slash_filename_middle_segments_joined() {
        // Regression: middle path segments that contain internal spaces now
        // get their spaces restored as underscores by the MIDDLE_SPACED_SEGMENT
        // rule. Previous behavior (pre-B.1) truncated at the first space
        // because only the tail filename was rewritten; the NDSS 2026
        // corpus analysis showed real PDFs do produce this shape (notably
        // the Linux kernelctf CVE writeup URLs in f1725), so the
        // conservative guard was blocking more fixes than it protected.
        //
        // Safety is now carried by the `/…/` brackets in the B.1 pattern
        // plus the narrow token class, which together refuse to fire on
        // narrative text. See
        // test_extract_urls_narrative_after_short_url_untouched and
        // test_extract_urls_filename_recovery_survives_narrative_guard.
        let text = "https://example.com/some dir/other dir/real_file.py";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://example.com/some_dir/other_dir/real_file.py"]
        );
    }

    // ── Next-reference-marker `[N` glue (NDSS 2026 f2926/f700/f94/f106) ──

    #[test]
    fn test_extract_urls_strips_next_ref_bracket_marker() {
        // When a PDF collapses whitespace between consecutive bibliography
        // entries, the next entry's "[N]" numeric marker fuses onto the tail
        // of the current URL. URL_RE previously excluded only `]`, so it
        // kept eating through `[N`, producing URLs like "url/page[42".
        // Excluding `[` as well stops at the right place.
        let text = "See https://www.cve.org/about/Metrics[2] (2025) MITRE ATT&CK Framework";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://www.cve.org/about/Metrics"]);
    }

    #[test]
    fn test_extract_urls_strips_next_ref_bracket_after_period() {
        // Trailing "." before "[N" (very common shape in the NDSS 2026
        // corpus, e.g. f700 ref [6]: "Yore-Wednesday.pdf.[6"). After `[`
        // is excluded, URL_RE stops at `[` and the existing trailing-punct
        // trim strips the `.`.
        let text = "Blah. https://i.blackhat.com/BH-US-24/Presentations/US24-Sialveras-Bugs-Of-Yore-Wednesday.pdf.[6] next ref";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://i.blackhat.com/BH-US-24/Presentations/US24-Sialveras-Bugs-Of-Yore-Wednesday.pdf"
            ]
        );
    }

    // ── Backslash escape artifacts (NDSS 2026 f700) ─────────────────────

    #[test]
    fn test_extract_urls_strips_backslash_hyphen() {
        // Some PDFs leak LaTeX `\-` soft-hyphen escapes into the text layer,
        // producing URLs with a literal backslash before a hyphen. Backslash
        // is never valid inside a URL, so dropping it restores the intended
        // path. Real case from NDSS 2026 f700 ref [7].
        let text = r"Available: https://i.blackhat.com/Asia-24/Presentations/Asia-24-Jiang-URB-Excalibur-The-New-VMware-All-Platform-VM\-Escapes.pdf";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://i.blackhat.com/Asia-24/Presentations/Asia-24-Jiang-URB-Excalibur-The-New-VMware-All-Platform-VM-Escapes.pdf"
            ]
        );
    }

    #[test]
    fn test_extract_urls_strips_backslash_before_word() {
        // Variant where the backslash sits directly in front of a word
        // rather than a hyphen (f700 ref [41] DEFCON32 URL).
        let text = r"https://media.defcon.org/DEFCON32/DEFCON32-\JiaQingHuang-Bug.pdf";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://media.defcon.org/DEFCON32/DEFCON32-JiaQingHuang-Bug.pdf"]
        );
    }

    // ── Tilde operator (U+223C) in URL regions (NDSS 2026 f328/f1725) ────

    #[test]
    fn test_extract_urls_tilde_operator_enables_filename_recovery() {
        // Some PDF fonts render the URL tilde `~` as U+223C ∼ (TILDE
        // OPERATOR). URL_REGION's char class previously didn't include ∼,
        // so `fix_url_spacing` — and therefore the filename-lost-underscores
        // recovery — skipped any URL containing `∼user/`. Real case from
        // NDSS 2026 f328 ref [73]: the source URL ends
        // `chi2010_tabbedbrowsing.pdf`, rendered in the PDF as
        // `chi2010 tabbedbrowsing.pdf` with a trailing `, 2010` citation
        // year (which is what forced the companion change to
        // FILENAME_LOST_UNDERSCORES' anchor).
        let text =
            "P. Dubroy, https://www.dgp.toronto.edu/∼ravin/papers/chi2010 tabbedbrowsing.pdf, 2010";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://www.dgp.toronto.edu/∼ravin/papers/chi2010_tabbedbrowsing.pdf"]
        );
    }

    #[test]
    fn test_extract_urls_filename_recovery_survives_narrative_guard() {
        // Guard: FILENAME_LOST_UNDERSCORES still refuses to join `real
        // file.pdf` into a filename when narrative follows without a
        // citation delimiter — the rule's trailer `\s*(?:$|[,;)])` fails
        // on `\s+right`. The middle-segment `/some dir/` *is* rewritten
        // to `/some_dir/` by the B.1 rule (that one only needs `/` on
        // both sides, which is safe), so the extracted URL extends one
        // more path level compared to the pre-B.1 behavior, but the
        // filename-level narrative guard still holds.
        let text = "See https://example.com/some dir/real file.pdf right there for the explanation";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/some_dir/real"]);
    }

    // ── Middle-segment underscore recovery (NDSS 2026 f1725, f131, …) ────

    #[test]
    fn test_extract_urls_middle_segment_kernelctf_cve() {
        // NDSS 2026 f1725 pattern: the kernelctf exploit writeup URL
        // `...kernelctf/CVE-2024-26581_lts_cos_mitigation/docs/exploit.md`
        // comes through PDF as a path segment with internal spaces. The
        // B.1 rule rewrites the spaced `[A-Za-z0-9-]+` tokens sandwiched
        // between two `/`s as underscored.
        let text = "https://github.com/google/security-research/blob/master/pocs/linux/kernelctf/CVE-2024-26581 lts cos mitigation/docs/exploit.md";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://github.com/google/security-research/blob/master/pocs/linux/kernelctf/CVE-2024-26581_lts_cos_mitigation/docs/exploit.md"
            ]
        );
    }

    #[test]
    fn test_extract_urls_middle_segment_with_hyphenated_filename() {
        // NDSS 2026 f131 ref [4]: `en_US/apple-platform-security-guide.pdf`
        // shows up with the locale segment spaced (`en US`) and a filename
        // that contains hyphens. B.1 joins `/en US/` to `/en_US/`; the
        // URL then passes through URL_RE without truncation.
        let text = "https://help.apple.com/pdf/security/en US/apple-platform-security-guide.pdf";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec!["https://help.apple.com/pdf/security/en_US/apple-platform-security-guide.pdf"]
        );
    }

    #[test]
    fn test_extract_urls_middle_segment_two_passes() {
        // The fixed-point loop is required because `replace_all` consumes
        // the trailing `/` of a match, so the first pass on
        // `/foo bar/baz qux/` only rewrites `/foo_bar/`. A second pass
        // catches `/baz_qux/`.
        let text = "https://example.com/foo bar/baz qux/end";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/foo_bar/baz_qux/end"]);
    }

    // ── Sentence boundary not confused with URL-internal dot (bluesky ref 3) ──

    #[test]
    fn test_extract_urls_sentence_period_not_joined_into_url() {
        // Real case from bluesky-blocks-paper ref 3: the ref text is
        // `... /mrd0ll4r/bluesky_downloader. GitHub repository`. The
        // period-space ends the URL's sentence; `GitHub` starts the
        // note field. The SPACED_DOT rule used to see `r. G` as a URL-
        // internal spaced dot and collapse it, producing the bogus
        // URL `.../bluesky_downloader.GitHub` which 404s on GitHub and
        // made URL Check report "not found". Requiring lowercase-or-
        // digit after the dot leaves sentence breaks alone.
        let text = "Leonhard Balduf. 2024. bluesky_downloader. https://github.com/mrd0ll4r/bluesky_downloader. GitHub repository";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://github.com/mrd0ll4r/bluesky_downloader"]);
    }

    #[test]
    fn test_extract_urls_lowercase_after_dot_still_collapses() {
        // Regression: the SPACED_DOT tightening must not break the
        // legitimate case of spaced dots inside a URL where the next
        // token is lowercase (domain labels, filename extensions, path
        // segments). This is the existing test_extract_urls_space_in_domain
        // behavior pinned down as a guard for future edits to SPACED_DOT.
        let text = "https://www.cs. cmu.edu/paper.pdf";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://www.cs.cmu.edu/paper.pdf"]);
    }

    #[test]
    fn test_extract_urls_middle_segment_refuses_across_urls() {
        // Critical safety: if URL_REGION's greedy match stretches across
        // two URLs (because everything between them happens to be in its
        // char class), the B.1 rule must NOT join tokens from one URL
        // onto the other. The token class excludes `:`, so `/repo See
        // https/` fails at the `:` right after `https`. The extractor
        // produces two separate URLs, not one mangled one.
        let text = "https://x.com/repo See https://y.com/page, 2024";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://x.com/repo".to_string()));
        assert!(urls.contains(&"https://y.com/page".to_string()));
    }
}
