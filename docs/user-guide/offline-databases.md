# Offline Databases

Hallucinator supports offline copies of DBLP and ACL Anthology for faster lookups, reduced API dependence, and better reliability. Offline databases are queried inline by the coordinator task (< 1ms), before any remote API calls.

## Why Use Offline Databases?

- **Speed** — SQLite FTS5 lookups complete in under 1ms vs. 100ms–5s for HTTP APIs
- **Reliability** — No network dependency, no rate limiting, no timeouts
- **Early exit** — If a reference is found locally, all remote DB queries are skipped
- **API savings** — Fewer remote calls means you stay within rate limits and API quotas

The tradeoff is disk space and a one-time build step.

## DBLP Offline

### What It Contains

The DBLP database indexes publications, authors, and URLs from [dblp.org](https://dblp.org). This covers computer science conferences and journals comprehensively — over 7 million publications.

### Building

```bash
hallucinator-cli update-dblp /path/to/dblp.db
```

This will:

1. Download `dblp.xml.gz` from dblp.org (~4.6GB compressed, ~16GB uncompressed)
2. Parse the XML to extract publications, authors, and URLs
3. Build a SQLite database with FTS5 full-text search index
4. Compact the database with VACUUM

**Time:** 20–30 minutes on a modern machine (mostly download + parse time)

**Disk space:** ~2–3GB for the final SQLite database

The build process supports **conditional download** — if the database already exists and the server reports the file hasn't changed (304 Not Modified), the download is skipped.

### Using

```bash
# CLI flag
hallucinator-cli check --dblp-offline /path/to/dblp.db paper.pdf

# Or set in config file
# [databases]
# dblp_offline_path = "/path/to/dblp.db"

# Or environment variable
DBLP_OFFLINE_PATH=/path/to/dblp.db hallucinator-cli check paper.pdf
```

### Staleness Warning

If the database is older than 30 days, a warning is printed. To refresh:

```bash
hallucinator-cli update-dblp /path/to/dblp.db
```

The update is incremental via conditional HTTP (ETag/If-Modified-Since), so if the upstream data hasn't changed, it completes instantly.

## ACL Anthology Offline

### What It Contains

The ACL Anthology database indexes papers from computational linguistics and NLP venues (ACL, EMNLP, NAACL, EACL, CoNLL, etc.) — tens of thousands of publications.

### Building

```bash
hallucinator-cli update-acl /path/to/acl.db
```

This will:

1. Download the ACL Anthology XML data from GitHub
2. Extract and parse XML files
3. Build a SQLite database with FTS5 full-text search index

**Time:** A few minutes (much smaller than DBLP)

**Disk space:** ~50–100MB for the final database

The build process tracks the GitHub commit SHA and skips the download if nothing has changed.

### Using

```bash
# CLI flag
hallucinator-cli check --acl-offline /path/to/acl.db paper.pdf

# Or set in config file
# [databases]
# acl_offline_path = "/path/to/acl.db"

# Or environment variable
ACL_OFFLINE_PATH=/path/to/acl.db hallucinator-cli check paper.pdf
```

## Recommended Setup

Store both databases in your platform config directory for automatic detection:

```bash
mkdir -p ~/.config/hallucinator

# Build databases
hallucinator-cli update-dblp ~/.config/hallucinator/dblp.db
hallucinator-cli update-acl ~/.config/hallucinator/acl.db

# Configure paths
cat > ~/.config/hallucinator/config.toml << 'EOF'
[databases]
dblp_offline_path = "~/.config/hallucinator/dblp.db"
acl_offline_path = "~/.config/hallucinator/acl.db"
cache_path = "~/.config/hallucinator/cache.db"
EOF
```

## Maintenance Schedule

| Database | Recommended refresh | Why |
|----------|-------------------|-----|
| DBLP | Monthly | New publications indexed regularly |
| ACL | Before conference deadlines | New proceedings added after each conference |

Both update commands are safe to run against existing databases — they rebuild in-place.

## Combining with Online Databases

Offline and online databases complement each other:

1. Local databases are queried first (< 1ms)
2. If verified locally, remote queries are skipped entirely
3. If not found locally, remote databases are queried in parallel
4. Having both reduces total validation time and improves coverage

This means you get the speed of local lookups for common CS and NLP papers, with full coverage from 10+ remote databases for everything else.
