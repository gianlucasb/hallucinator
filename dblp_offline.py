"""
Offline DBLP database support.

Downloads and indexes the DBLP N-Triples dump into a SQLite database
for fast local lookups without API rate limiting.
"""

import gzip
import os
import re
import sqlite3
import time
import urllib.request
from datetime import datetime, timezone

# DBLP RDF predicates we care about
DBLP_TITLE = "https://dblp.org/rdf/schema#title"
DBLP_AUTHORED_BY = "https://dblp.org/rdf/schema#authoredBy"
DBLP_PRIMARY_NAME = "https://dblp.org/rdf/schema#primaryCreatorName"

# Daily dump URL
DBLP_DUMP_URL = "https://dblp.org/rdf/dblp.nt.gz"

# Staleness threshold in days
STALENESS_THRESHOLD_DAYS = 30


def parse_ntriples_line(line):
    """Parse a single N-Triples line into (subject, predicate, object).

    N-Triples format: <subject> <predicate> <object> .
    Objects can be URIs (<...>) or literals ("..." or "..."^^type or "..."@lang)
    """
    line = line.strip()
    if not line or line.startswith('#'):
        return None, None, None

    # Match URI pattern: <...>
    uri_pattern = r'<([^>]+)>'
    # Match literal pattern: "..." with optional ^^type or @lang
    literal_pattern = r'"((?:[^"\\]|\\.)*)(?:"(?:\^\^<[^>]+>|@[a-z]+)?)?'

    parts = []
    pos = 0

    for i in range(3):
        # Skip whitespace
        while pos < len(line) and line[pos] in ' \t':
            pos += 1

        if pos >= len(line):
            return None, None, None

        if line[pos] == '<':
            # URI
            match = re.match(uri_pattern, line[pos:])
            if match:
                parts.append(match.group(1))
                pos += match.end()
            else:
                return None, None, None
        elif line[pos] == '"':
            # Literal
            match = re.match(literal_pattern, line[pos:])
            if match:
                # Unescape common escape sequences
                value = match.group(1)
                value = value.replace('\\n', '\n').replace('\\t', '\t')
                value = value.replace('\\"', '"').replace('\\\\', '\\')
                parts.append(value)
                # Find end of literal (including type/lang suffix)
                end_quote = line.find('"', pos + 1)
                while end_quote > 0 and line[end_quote - 1] == '\\':
                    end_quote = line.find('"', end_quote + 1)
                if end_quote > 0:
                    # Skip past optional ^^<type> or @lang
                    pos = end_quote + 1
                    if pos < len(line) and line[pos:pos+2] == '^^':
                        type_match = re.match(r'\^\^<[^>]+>', line[pos:])
                        if type_match:
                            pos += type_match.end()
                    elif pos < len(line) and line[pos] == '@':
                        lang_match = re.match(r'@[a-z]+', line[pos:])
                        if lang_match:
                            pos += lang_match.end()
                else:
                    return None, None, None
            else:
                return None, None, None
        else:
            return None, None, None

    if len(parts) == 3:
        return parts[0], parts[1], parts[2]
    return None, None, None


def download_dblp_dump(output_path, on_progress=None):
    """Download the latest DBLP N-Triples dump.

    Args:
        output_path: Where to save the .nt.gz file
        on_progress: Optional callback(bytes_downloaded, total_bytes)

    Returns:
        Path to downloaded file
    """
    print(f"Downloading DBLP dump from {DBLP_DUMP_URL}...")
    print("This is ~4.6GB and may take a while.")

    # Get file size first
    req = urllib.request.Request(DBLP_DUMP_URL, method='HEAD')
    with urllib.request.urlopen(req) as response:
        total_size = int(response.headers.get('Content-Length', 0))

    # Download with progress
    downloaded = 0
    last_report = 0
    chunk_size = 1024 * 1024  # 1MB chunks

    req = urllib.request.Request(DBLP_DUMP_URL)
    with urllib.request.urlopen(req) as response:
        with open(output_path, 'wb') as f:
            while True:
                chunk = response.read(chunk_size)
                if not chunk:
                    break
                f.write(chunk)
                downloaded += len(chunk)

                # Report progress every 50MB
                if downloaded - last_report >= 50 * 1024 * 1024:
                    if total_size:
                        pct = 100 * downloaded / total_size
                        print(f"  Downloaded {downloaded / (1024*1024):.0f}MB / {total_size / (1024*1024):.0f}MB ({pct:.1f}%)")
                    else:
                        print(f"  Downloaded {downloaded / (1024*1024):.0f}MB")
                    last_report = downloaded

                if on_progress:
                    on_progress(downloaded, total_size)

    print(f"Download complete: {output_path}")
    return output_path


def build_sqlite_db(ntriples_path, db_path, on_progress=None):
    """Build SQLite database from N-Triples dump.

    Args:
        ntriples_path: Path to .nt.gz file
        db_path: Output SQLite database path
        on_progress: Optional callback(triples_processed, publications_found)

    This is a two-pass process:
    1. First pass: collect all publications with titles and their author links
    2. Second pass: resolve author names
    Then write to SQLite with FTS index.
    """
    print(f"Building SQLite database from {ntriples_path}...")
    print("This may take 10-20 minutes for the full dump.")

    # Data structures for first pass
    pub_titles = {}      # pub_uri -> title
    pub_authors = {}     # pub_uri -> [author_uri, ...]
    author_names = {}    # author_uri -> name

    # First pass: collect everything
    start_time = time.time()
    triple_count = 0

    opener = gzip.open if ntriples_path.endswith('.gz') else open

    print("Pass 1: Extracting triples...")
    with opener(ntriples_path, 'rt', encoding='utf-8', errors='replace') as f:
        for line in f:
            subj, pred, obj = parse_ntriples_line(line)
            if not subj:
                continue

            triple_count += 1

            if pred == DBLP_TITLE:
                pub_titles[subj] = obj
            elif pred == DBLP_AUTHORED_BY:
                if subj not in pub_authors:
                    pub_authors[subj] = []
                pub_authors[subj].append(obj)
            elif pred == DBLP_PRIMARY_NAME:
                author_names[subj] = obj

            if triple_count % 5_000_000 == 0:
                elapsed = time.time() - start_time
                print(f"  Processed {triple_count / 1_000_000:.1f}M triples ({elapsed:.0f}s)")
                if on_progress:
                    on_progress(triple_count, len(pub_titles))

    print(f"  Total: {triple_count:,} triples")
    print(f"  Found {len(pub_titles):,} publications with titles")
    print(f"  Found {len(author_names):,} author names")

    # Build database
    print("Pass 2: Building SQLite database...")

    # Remove existing db
    if os.path.exists(db_path):
        os.remove(db_path)

    conn = sqlite3.connect(db_path)
    cur = conn.cursor()

    # Create tables
    cur.execute('''
        CREATE TABLE publications (
            id INTEGER PRIMARY KEY,
            uri TEXT UNIQUE,
            title TEXT,
            authors TEXT,
            url TEXT
        )
    ''')

    cur.execute('''
        CREATE VIRTUAL TABLE publications_fts USING fts5(
            title,
            content='publications',
            content_rowid='id'
        )
    ''')

    # Store metadata
    cur.execute('''
        CREATE TABLE metadata (
            key TEXT PRIMARY KEY,
            value TEXT
        )
    ''')
    cur.execute(
        'INSERT INTO metadata (key, value) VALUES (?, ?)',
        ('build_date', datetime.now(timezone.utc).isoformat())
    )
    cur.execute(
        'INSERT INTO metadata (key, value) VALUES (?, ?)',
        ('triple_count', str(triple_count))
    )
    cur.execute(
        'INSERT INTO metadata (key, value) VALUES (?, ?)',
        ('publication_count', str(len(pub_titles)))
    )

    # Insert publications
    insert_count = 0
    batch = []
    batch_size = 10000

    for pub_uri, title in pub_titles.items():
        # Resolve author names
        author_uris = pub_authors.get(pub_uri, [])
        names = []
        for uri in author_uris:
            name = author_names.get(uri)
            if name:
                names.append(name)

        authors_str = "; ".join(names) if names else ""

        # Use the publication URI as the URL (DBLP URIs are web-accessible)
        url = pub_uri

        batch.append((pub_uri, title, authors_str, url))

        if len(batch) >= batch_size:
            cur.executemany(
                'INSERT INTO publications (uri, title, authors, url) VALUES (?, ?, ?, ?)',
                batch
            )
            insert_count += len(batch)
            batch = []

            if insert_count % 100000 == 0:
                print(f"  Inserted {insert_count:,} publications...")

    # Insert remaining
    if batch:
        cur.executemany(
            'INSERT INTO publications (uri, title, authors, url) VALUES (?, ?, ?, ?)',
            batch
        )
        insert_count += len(batch)

    print(f"  Total: {insert_count:,} publications inserted")

    # Build FTS index
    print("Building full-text search index...")
    cur.execute('''
        INSERT INTO publications_fts (rowid, title)
        SELECT id, title FROM publications
    ''')

    conn.commit()

    # Create regular index on title for exact matches
    print("Creating indexes...")
    cur.execute('CREATE INDEX idx_title ON publications(title)')

    conn.close()

    elapsed = time.time() - start_time
    db_size = os.path.getsize(db_path) / (1024 * 1024)
    print(f"Database built in {elapsed:.0f}s: {db_path} ({db_size:.0f}MB)")

    return db_path


def get_db_metadata(db_path):
    """Get metadata from the database.

    Returns dict with 'build_date', 'triple_count', 'publication_count'
    """
    if not os.path.exists(db_path):
        return None

    try:
        conn = sqlite3.connect(db_path)
        cur = conn.cursor()
        cur.execute('SELECT key, value FROM metadata')
        metadata = dict(cur.fetchall())
        conn.close()
        return metadata
    except Exception:
        return None


def get_db_age_days(db_path):
    """Get age of database in days, or None if can't determine."""
    metadata = get_db_metadata(db_path)
    if not metadata or 'build_date' not in metadata:
        return None

    try:
        build_date = datetime.fromisoformat(metadata['build_date'])
        now = datetime.now(timezone.utc)
        age = now - build_date
        return age.days
    except Exception:
        return None


def check_staleness(db_path):
    """Check if database is stale and return warning message if so."""
    age = get_db_age_days(db_path)
    if age is None:
        return "Could not determine DBLP database age."

    if age > STALENESS_THRESHOLD_DAYS:
        return f"Your DBLP database is {age} days old. Run with --update-dblp to refresh."

    return None


def query_offline(title, db_path):
    """Query the offline DBLP database for a title.

    Args:
        title: Title to search for
        db_path: Path to SQLite database

    Returns:
        (found_title, authors_list, url) or (None, [], None)
    """
    if not os.path.exists(db_path):
        raise FileNotFoundError(f"DBLP database not found: {db_path}")

    # Import here to avoid circular dependency
    from check_hallucinated_references import normalize_title, get_query_words
    from rapidfuzz import fuzz

    conn = sqlite3.connect(db_path)
    cur = conn.cursor()

    # Use FTS to find candidates
    words = get_query_words(title, 6)
    query = ' '.join(words)

    # FTS5 query - search for publications containing these words
    cur.execute('''
        SELECT p.title, p.authors, p.url
        FROM publications_fts fts
        JOIN publications p ON fts.rowid = p.id
        WHERE publications_fts MATCH ?
        LIMIT 20
    ''', (query,))

    results = cur.fetchall()
    conn.close()

    # Find best fuzzy match
    normalized_input = normalize_title(title)

    for found_title, authors_str, url in results:
        if fuzz.ratio(normalized_input, normalize_title(found_title)) >= 95:
            # Parse authors string back to list
            authors = [a.strip() for a in authors_str.split(';') if a.strip()]
            return found_title, authors, url

    return None, [], None


def update_dblp_db(db_path, keep_download=False):
    """Download latest DBLP dump and build/update the database.

    Args:
        db_path: Path for SQLite database
        keep_download: If True, keep the .nt.gz file after building

    Returns:
        Path to built database
    """
    import tempfile

    # Download to temp location
    db_dir = os.path.dirname(db_path) or '.'
    os.makedirs(db_dir, exist_ok=True)

    if keep_download:
        download_path = os.path.join(db_dir, 'dblp.nt.gz')
    else:
        # Use temp file
        fd, download_path = tempfile.mkstemp(suffix='.nt.gz')
        os.close(fd)

    try:
        download_dblp_dump(download_path)
        build_sqlite_db(download_path, db_path)
        return db_path
    finally:
        if not keep_download and os.path.exists(download_path):
            os.remove(download_path)


if __name__ == '__main__':
    # Simple test/demo
    import sys

    if len(sys.argv) < 2:
        print("Usage: python dblp_offline.py <command> [args]")
        print("Commands:")
        print("  build <ntriples.nt.gz> <output.db>  - Build DB from N-Triples")
        print("  update <output.db>                  - Download and build DB")
        print("  query <db.db> <title>               - Query for a title")
        print("  info <db.db>                        - Show DB info")
        sys.exit(1)

    cmd = sys.argv[1]

    if cmd == 'build' and len(sys.argv) >= 4:
        build_sqlite_db(sys.argv[2], sys.argv[3])
    elif cmd == 'update' and len(sys.argv) >= 3:
        update_dblp_db(sys.argv[2])
    elif cmd == 'query' and len(sys.argv) >= 4:
        db = sys.argv[2]
        title = ' '.join(sys.argv[3:])
        found, authors, url = query_offline(title, db)
        if found:
            print(f"Found: {found}")
            print(f"Authors: {authors}")
            print(f"URL: {url}")
        else:
            print("Not found")
    elif cmd == 'info' and len(sys.argv) >= 3:
        meta = get_db_metadata(sys.argv[2])
        if meta:
            print(f"Build date: {meta.get('build_date', 'unknown')}")
            print(f"Publications: {meta.get('publication_count', 'unknown')}")
            print(f"Triples: {meta.get('triple_count', 'unknown')}")
            age = get_db_age_days(sys.argv[2])
            if age is not None:
                print(f"Age: {age} days")
                warning = check_staleness(sys.argv[2])
                if warning:
                    print(f"Warning: {warning}")
        else:
            print("Could not read database metadata")
    else:
        print(f"Unknown command or missing args: {cmd}")
        sys.exit(1)
