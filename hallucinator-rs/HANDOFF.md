# Handoff: Architectural Refactor — Fix Inverted Core/PDF Dependency

**Branch**: `feat/cache-doi-improvements`
**Status**: `cargo check` passes cleanly. NOT yet end-to-end tested.

---

## What Was Done

Full 3-phase architectural refactor to fix the inverted `hallucinator-core → hallucinator-pdf`
dependency, isolate mupdf (AGPL), and add archive support to the CLI.

### Phase 1 — Types + utilities to core, dep direction reversed

- `Reference`, `ExtractionResult`, `SkipStats` moved from `hallucinator-pdf` → `hallucinator-core`
- `get_query_words`, `extract_doi`, `extract_arxiv_id` moved from `hallucinator-pdf/src/identifiers.rs`
  → new file `hallucinator-core/src/text_utils.rs`
- `hallucinator-core/Cargo.toml`: removed `hallucinator-pdf` dep (the inversion)
- `hallucinator-pdf/Cargo.toml`: added `hallucinator-core` dep
- `hallucinator-pdf/src/lib.rs`: re-exports types from core (backward compat)
- `hallucinator-pdf/src/identifiers.rs`: re-exports text utils from core (backward compat)
- `hallucinator-bbl`: switched from `hallucinator-pdf` dep to `hallucinator-core`
- 7 DB backends in `hallucinator-core/src/db/`: updated `get_query_words` import
- TUI (`tui_event.rs`, `backend.rs`), web (`stream.rs`): updated type imports

### Phase 2 — New `hallucinator-ingest` crate (unified dispatch + archive)

New crate at `crates/hallucinator-ingest/`:
- `src/lib.rs`: `extract_references(path)` dispatches on extension (bbl/bib/pdf)
- `src/archive.rs`: archive streaming logic (ZIP + tar.gz) moved from `hallucinator-pdf`
- `pdf` feature (default): gates mupdf via `hallucinator-pdf-mupdf` optional dep

CLI (`hallucinator-cli/src/main.rs`):
- Archive detection: `hallucinator_ingest::is_archive_path()` checked before single-file path
- `run_archive_check()`: streams archive entries, runs validation on each PDF inside
- `--output <PATH>` flag: write results to file (per-file headers for archives)
- Single-file dispatch: replaced manual bbl/bib/pdf if-chain with `hallucinator_ingest::extract_references()`

TUI:
- `backend.rs`: replaced 3-way dispatch with `hallucinator_ingest::extract_references()`
- `app.rs`: archive imports switched to `hallucinator_ingest::*`

### Phase 3 — `PdfBackend` trait + `hallucinator-pdf-mupdf` isolation

- `hallucinator-pdf/src/backend.rs` (new): `PdfBackend: Send + Sync` trait
- `hallucinator-pdf/src/extractor.rs`: added `extract_references_via_backend(&dyn PdfBackend)`;
  removed old `#[cfg(feature = "pdf")]` gated methods that called the deleted `extract.rs`
- `hallucinator-pdf/Cargo.toml`: mupdf removed from prod deps; `pdf = []` kept as no-op feature
  for backward compat; `mupdf` added to **dev-deps** (needed by `bbl_ground_truth.rs` test)
- New crate `crates/hallucinator-pdf-mupdf/`: sole holder of mupdf dep; `MupdfBackend` implements
  `PdfBackend`
- `hallucinator-pdf/tests/bbl_ground_truth.rs`: defines local `LocalMupdfBackend` (can't use
  `hallucinator-pdf-mupdf` directly — that would be a circular dep)

**Verified**:
```
cargo tree -p hallucinator-pdf --no-dev-dependencies | grep mupdf
# (empty — mupdf not in hallucinator-pdf prod deps)

cargo tree -p hallucinator-pdf-mupdf | grep mupdf
# shows mupdf only here
```

---

## Testing Needed (not yet done)

### 1. Single PDF (regression)

```bash
cargo build --bin hallucinator-cli
./target/debug/hallucinator-cli check ../test-data/hallucinated.pdf
```

Expected: same output as before the refactor — references checked, verified/not-found results printed.

### 2. Archive extraction (new functionality)

```bash
./target/debug/hallucinator-cli check ../test-data/1.zip
```

Expected: CLI detects `.zip`, streams contained PDFs, prints per-file headers and results.

```bash
./target/debug/hallucinator-cli check ../test-data/1.zip --output results.txt
cat results.txt
```

Expected: same output written to file.

Also test with `.tar.gz` if available in test-data.

### 3. BBL / BIB files (regression)

```bash
./target/debug/hallucinator-cli check some_paper.bbl
./target/debug/hallucinator-cli check some_paper.bib
```

### 4. Dry-run mode

```bash
./target/debug/hallucinator-cli check --dry-run ../test-data/hallucinated.pdf
```

This exercises `dry_run_pdf()` which now uses `MupdfBackend.extract_text()` directly. Ensure it
still prints the raw segmented references without running validation.

### 5. TUI regression

```
cargo run --bin hallucinator-tui
```

- Load a PDF via the file picker → should process normally
- Load a `.zip` via the file picker → should queue all PDFs inside
- Load a `.bbl` file → should use BBL parser

### 6. Ground truth test (slow, requires test-data)

```bash
cargo test -p hallucinator-pdf --test bbl_ground_truth -- --ignored --nocapture
```

---

## Known Issues / Things to Watch

- **`test-data/1.zip`** was the original motivation for this whole refactor — never got to
  actually run it end-to-end. This is the first thing to verify.
- The `bbl_ground_truth.rs` test has a `LocalMupdfBackend` defined inline. If mupdf API changes,
  update both that test and `hallucinator-pdf-mupdf/src/lib.rs`.
- `hallucinator-web` is excluded from the workspace (in `Cargo.toml` `exclude`) — not checked
  in this build. Its `stream.rs` was updated to remove the `use hallucinator_pdf::ExtractionResult`
  import but not cargo-checked independently.
- The `pdf = []` feature on `hallucinator-pdf` is now a pure no-op (backward compat shim).
  Consumers specifying `features = ["pdf"]` won't get errors but also won't get mupdf — they need
  to depend on `hallucinator-pdf-mupdf` directly or use `hallucinator-ingest` (which pulls it in).

---

## Dep Graph After Refactor

```
hallucinator-core          ← Reference, ExtractionResult, SkipStats,
                               get_query_words, extract_doi, extract_arxiv_id
hallucinator-bbl           ← depends on core only
hallucinator-pdf           ← depends on core; owns parsing pipeline + PdfBackend trait;
                               NO mupdf prod dep
hallucinator-pdf-mupdf     ← thin mupdf impl of PdfBackend (sole AGPL island)
hallucinator-ingest        ← unified dispatch (pdf/bbl/bib/archive);
                               pdf feature (default) pulls in hallucinator-pdf-mupdf
hallucinator-cli/tui       ← depend on hallucinator-ingest
```
