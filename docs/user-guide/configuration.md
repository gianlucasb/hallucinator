# Configuration Reference

Hallucinator can be configured via CLI flags, environment variables, and TOML config files. This page documents all options.

## Precedence

Configuration is resolved in this order (highest wins):

1. **CLI flags** — `--num-workers 8`, `--openalex-key KEY`
2. **Environment variables** — `OPENALEX_KEY`, `DB_TIMEOUT`
3. **CWD config** — `.hallucinator.toml` in the current working directory
4. **Platform config** — `~/.config/hallucinator/config.toml` (Linux/macOS) or `%APPDATA%\hallucinator\config.toml` (Windows)
5. **Defaults**

CWD config overlays platform config field-by-field. This lets you keep API keys in the global config and override settings per-project.

## Config File Format

Both config file locations use the same TOML format:

```toml
[api_keys]
openalex_key = "your-openalex-key"
s2_api_key = "your-semantic-scholar-key"
crossref_mailto = "you@example.com"

[databases]
dblp_offline_path = "/path/to/dblp.db"
acl_offline_path = "/path/to/acl.db"
cache_path = "/path/to/cache.db"
searxng_url = "http://localhost:8080"
disabled = ["NeurIPS", "SSRN"]

[concurrency]
num_workers = 4
db_timeout_secs = 10
db_timeout_short_secs = 5
max_rate_limit_retries = 3
max_archive_size_mb = 500

[display]
theme = "hacker"
fps = 30
```

All fields are optional. Omitted fields use defaults.

## Full Option Reference

### API Keys

| Option | CLI Flag | Env Var | TOML Key | Description |
|--------|----------|---------|----------|-------------|
| OpenAlex key | `--openalex-key KEY` | `OPENALEX_KEY` | `api_keys.openalex_key` | Enables OpenAlex database queries |
| Semantic Scholar key | `--s2-api-key KEY` | `S2_API_KEY` | `api_keys.s2_api_key` | Higher S2 rate limit (100/s vs 1/s) |
| CrossRef mailto | — | `CROSSREF_MAILTO` | `api_keys.crossref_mailto` | CrossRef polite pool (3/s vs 1/s) |

### Databases

| Option | CLI Flag | Env Var | TOML Key | Default |
|--------|----------|---------|----------|---------|
| DBLP offline path | `--dblp-offline PATH` | `DBLP_OFFLINE_PATH` | `databases.dblp_offline_path` | None |
| ACL offline path | `--acl-offline PATH` | `ACL_OFFLINE_PATH` | `databases.acl_offline_path` | None |
| Cache path | `--cache-path PATH` | `HALLUCINATOR_CACHE_PATH` | `databases.cache_path` | None |
| SearxNG URL | `--searxng` (flag) | `SEARXNG_URL` | `databases.searxng_url` | `http://localhost:8080` |
| Disabled DBs | `--disable-dbs A,B` | — | `databases.disabled` | `[]` |

**Notes:**
- `--searxng` is a boolean flag on the CLI. The actual URL comes from the env var or config file, defaulting to `http://localhost:8080`.
- `--disable-dbs` accepts a comma-separated list. Database names are case-sensitive: `CrossRef`, `arXiv`, `DBLP`, `Semantic Scholar`, `OpenAlex`, `Europe PMC`, `PubMed`, `ACL Anthology`, `NeurIPS`, `DOI`, `SSRN`, `Web Search`.

### Concurrency

| Option | CLI Flag | Env Var | TOML Key | Default |
|--------|----------|---------|----------|---------|
| Worker count | `--num-workers N` | — | `concurrency.num_workers` | 4 |
| DB timeout | — | `DB_TIMEOUT` | `concurrency.db_timeout_secs` | 10 |
| Short timeout | — | `DB_TIMEOUT_SHORT` | `concurrency.db_timeout_short_secs` | 5 |
| Max 429 retries | `--max-rate-limit-retries N` | — | `concurrency.max_rate_limit_retries` | 3 |
| Max archive size | — | — | `concurrency.max_archive_size_mb` | 500 |

### Display (TUI only)

| Option | TOML Key | Default | Values |
|--------|----------|---------|--------|
| Theme | `display.theme` | `hacker` | `hacker`, `modern`, `gnr` |
| FPS | `display.fps` | 30 | 1–120 |

### Other CLI Flags

| Flag | Description |
|------|-------------|
| `--no-color` | Disable colored output |
| `-o, --output PATH` | Write results to file |
| `--dry-run` | Extract and print references without querying databases |
| `--check-openalex-authors` | Flag author mismatches from OpenAlex (skipped by default) |
| `--clear-cache` | Clear the entire query cache and exit |
| `--clear-not-found` | Clear only not-found entries from cache and exit |
| `--config PATH` | Path to config file (overrides auto-detection) |
| `--log PATH` | Write tracing/debug logs to file |

## CLI Commands

```
hallucinator-cli check <file>         # Check a PDF, BBL, or BIB file
hallucinator-cli update-dblp <path>   # Download and build offline DBLP database
hallucinator-cli update-acl <path>    # Download and build offline ACL database
```

## Cache Configuration

The query cache stores database responses to avoid redundant API calls across runs.

- **Positive TTL** (found entries): 7 days
- **Negative TTL** (not-found entries): 24 hours
- **Storage:** SQLite with WAL mode + in-memory DashMap

To enable caching, set `cache_path` in your config or use `--cache-path`:

```bash
hallucinator-cli check --cache-path ~/.hallucinator/cache.db paper.pdf
```

Cache maintenance:

```bash
# Clear everything
hallucinator-cli check --cache-path ~/.hallucinator/cache.db --clear-cache

# Clear only not-found entries (useful after DB outages)
hallucinator-cli check --cache-path ~/.hallucinator/cache.db --clear-not-found
```

## Auto-detection

The TUI and CLI auto-detect offline database paths from well-known locations on your system. If you place `dblp.db` or `acl.db` in your platform config directory (`~/.config/hallucinator/` on Linux/macOS), they may be found automatically. Explicit paths in the config file or CLI flags always take precedence.

## Example Configurations

### Minimal (API keys only)

```toml
[api_keys]
crossref_mailto = "researcher@university.edu"
```

### Full Setup

```toml
[api_keys]
openalex_key = "your-key"
s2_api_key = "your-key"
crossref_mailto = "researcher@university.edu"

[databases]
dblp_offline_path = "~/.hallucinator/dblp.db"
acl_offline_path = "~/.hallucinator/acl.db"
cache_path = "~/.hallucinator/cache.db"

[concurrency]
num_workers = 8
db_timeout_secs = 15

[display]
theme = "modern"
```

### CI / Scripting

```toml
[databases]
cache_path = "/tmp/hallucinator-cache.db"
disabled = ["OpenAlex", "NeurIPS", "SSRN"]

[concurrency]
num_workers = 2
db_timeout_secs = 5
max_rate_limit_retries = 1
```
