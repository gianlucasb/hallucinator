# Python Bindings

Hallucinator provides Python bindings via PyO3, offering the full validation pipeline as a native Python package with pre-compiled wheels.

## Installation

```bash
pip install hallucinator
```

Pre-compiled wheels are available for major platforms (Linux x86_64, macOS x86_64/ARM64, Windows x86_64). To build from source:

```bash
cd hallucinator-rs/crates/hallucinator-python
pip install maturin
maturin develop --release
```

## What's Available

The Python bindings expose:

- **`PdfExtractor`** — Extract references from PDFs with configurable parsing
- **`Validator` + `ValidatorConfig`** — Validate references against academic databases
- **`ValidationResult`** — Per-reference results with status, source, authors, per-DB details
- **`ProgressEvent`** — Real-time progress callbacks
- **`ArchiveIterator`** — Stream PDFs from tar.gz/zip archives
- **Custom segmentation strategies** — Pass Python callables for reference segmentation

## Quick Example

```python
from hallucinator import PdfExtractor, Validator, ValidatorConfig

# Extract
extractor = PdfExtractor()
result = extractor.extract("paper.pdf")

# Validate
config = ValidatorConfig()
config.crossref_mailto = "you@example.com"
config.num_workers = 4
config.db_timeout_secs = 10
validator = Validator(config)

def on_progress(event):
    if event.type == "result":
        print(f"  [{event.status}] {event.title}")

results = validator.check(result.references, progress=on_progress)
```

## Full Documentation

The complete Python API reference — including all configuration options, custom extraction strategies, progress event types, and result inspection — is in:

**[PYTHON_BINDINGS.md](https://github.com/gianlucasb/hallucinator/blob/main/hallucinator-rs/PYTHON_BINDINGS.md)**

This covers:

- Installation (wheels vs. source build)
- PDF extraction API and configuration
- Custom segmentation strategies (Python callables)
- Validator configuration options with defaults
- Progress callbacks and event types
- Per-database result inspection
- DOI, arXiv, and retraction information
- Archive processing
- Complete API reference tables
- End-to-end examples
