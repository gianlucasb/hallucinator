import logging
import os
import shutil
import tempfile
import tarfile
import urllib.parse
import zipfile
from flask import Flask, render_template, request, jsonify

from check_hallucinated_references import (
    extract_references_with_titles_and_authors,
    query_crossref,
    query_arxiv,
    query_dblp,
    query_openalex,
    query_openreview,
    query_semantic_scholar,
    validate_authors,
)

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(message)s',
    datefmt='%Y-%m-%d %H:%M:%S'
)
logger = logging.getLogger(__name__)

app = Flask(__name__)

# Security limits for archive processing
MAX_FILES_IN_ARCHIVE = 50
MAX_EXTRACTED_SIZE_MB = 500


def get_file_type(filename):
    """Detect file type from extension."""
    lower = filename.lower()
    if lower.endswith('.pdf'):
        return 'pdf'
    elif lower.endswith('.zip'):
        return 'zip'
    elif lower.endswith('.tar.gz') or lower.endswith('.tgz'):
        return 'tar.gz'
    return None


def safe_filename(filename):
    """Check if filename is safe (no path traversal, no hidden files, no __MACOSX)."""
    # Normalize path separators
    normalized = filename.replace('\\', '/')

    # Skip hidden files and __MACOSX
    parts = normalized.split('/')
    for part in parts:
        if part.startswith('.') or part == '__MACOSX':
            return None

    # Check for path traversal
    if '..' in normalized or normalized.startswith('/'):
        return None

    return normalized


def is_valid_pdf(file_path):
    """Check if file has PDF magic bytes."""
    try:
        with open(file_path, 'rb') as f:
            header = f.read(5)
            return header == b'%PDF-'
    except Exception:
        return False


def extract_pdfs_from_archive(archive_path, file_type, extract_dir):
    """Extract PDFs from archive with security limits.

    Returns list of (original_name, extracted_path) tuples, or raises ValueError on error.
    """
    pdf_files = []
    total_size = 0
    max_size_bytes = MAX_EXTRACTED_SIZE_MB * 1024 * 1024

    logger.info(f"Extracting {file_type} archive...")

    try:
        if file_type == 'zip':
            with zipfile.ZipFile(archive_path, 'r') as zf:
                # Check for zip bomb
                for info in zf.infolist():
                    total_size += info.file_size
                    if total_size > max_size_bytes:
                        logger.error(f"Archive too large: {total_size / 1024 / 1024:.1f}MB exceeds {MAX_EXTRACTED_SIZE_MB}MB limit")
                        raise ValueError(f"Archive exceeds maximum extracted size ({MAX_EXTRACTED_SIZE_MB}MB)")

                logger.info(f"Archive total uncompressed size: {total_size / 1024 / 1024:.1f}MB")

                # Extract PDFs
                for info in zf.infolist():
                    if info.is_dir():
                        continue

                    safe_name = safe_filename(info.filename)
                    if safe_name is None:
                        logger.debug(f"Skipping unsafe path: {info.filename}")
                        continue

                    if not safe_name.lower().endswith('.pdf'):
                        continue

                    if len(pdf_files) >= MAX_FILES_IN_ARCHIVE:
                        logger.error(f"Too many PDFs: limit is {MAX_FILES_IN_ARCHIVE}")
                        raise ValueError(f"Too many PDF files in archive (max {MAX_FILES_IN_ARCHIVE})")

                    # Extract to flat structure with unique names
                    basename = os.path.basename(safe_name)
                    extract_path = os.path.join(extract_dir, f"{len(pdf_files)}_{basename}")

                    with zf.open(info) as src, open(extract_path, 'wb') as dst:
                        dst.write(src.read())

                    if is_valid_pdf(extract_path):
                        pdf_files.append((basename, extract_path))
                        logger.info(f"  Extracted: {basename}")
                    else:
                        logger.warning(f"  Skipping invalid PDF: {basename}")
                        os.unlink(extract_path)

        elif file_type == 'tar.gz':
            with tarfile.open(archive_path, 'r:gz') as tf:
                # Check sizes and extract PDFs
                for member in tf.getmembers():
                    if not member.isfile():
                        continue

                    total_size += member.size
                    if total_size > max_size_bytes:
                        logger.error(f"Archive too large: {total_size / 1024 / 1024:.1f}MB exceeds {MAX_EXTRACTED_SIZE_MB}MB limit")
                        raise ValueError(f"Archive exceeds maximum extracted size ({MAX_EXTRACTED_SIZE_MB}MB)")

                    safe_name = safe_filename(member.name)
                    if safe_name is None:
                        logger.debug(f"Skipping unsafe path: {member.name}")
                        continue

                    if not safe_name.lower().endswith('.pdf'):
                        continue

                    if len(pdf_files) >= MAX_FILES_IN_ARCHIVE:
                        logger.error(f"Too many PDFs: limit is {MAX_FILES_IN_ARCHIVE}")
                        raise ValueError(f"Too many PDF files in archive (max {MAX_FILES_IN_ARCHIVE})")

                    # Extract to flat structure with unique names
                    basename = os.path.basename(safe_name)
                    extract_path = os.path.join(extract_dir, f"{len(pdf_files)}_{basename}")

                    with tf.extractfile(member) as src, open(extract_path, 'wb') as dst:
                        dst.write(src.read())

                    if is_valid_pdf(extract_path):
                        pdf_files.append((basename, extract_path))
                        logger.info(f"  Extracted: {basename}")
                    else:
                        logger.warning(f"  Skipping invalid PDF: {basename}")
                        os.unlink(extract_path)

        logger.info(f"Extracted {len(pdf_files)} PDF(s) from archive")

    except zipfile.BadZipFile:
        logger.error("Invalid or corrupted ZIP file")
        raise ValueError("Invalid or corrupted ZIP file")
    except tarfile.TarError as e:
        logger.error(f"Invalid or corrupted tar.gz file: {e}")
        raise ValueError("Invalid or corrupted tar.gz file")

    return pdf_files


def analyze_pdf(pdf_path, openalex_key=None):
    """Analyze PDF and return structured results.

    Returns (results, skip_stats) where results is a list of dicts with keys:
        - title: reference title
        - status: 'verified' | 'not_found' | 'author_mismatch'
        - error_type: None | 'not_found' | 'author_mismatch'
        - source: database where found (if any)
        - ref_authors: authors from the PDF
        - found_authors: authors from the database (if found)
    """
    logger.info("Extracting references from PDF...")
    refs, skip_stats = extract_references_with_titles_and_authors(pdf_path, return_stats=True)
    logger.info(f"Found {len(refs)} references to check (skipped {skip_stats['skipped_url']} URLs, {skip_stats['skipped_short_title']} short titles)")
    results = []

    for i, (title, ref_authors) in enumerate(refs, 1):
        short_title = title[:60] + '...' if len(title) > 60 else title
        logger.info(f"[{i}/{len(refs)}] Checking: {short_title}")

        result = {
            'title': title,
            'status': 'verified',
            'error_type': None,
            'source': None,
            'ref_authors': ref_authors,
            'found_authors': [],
        }

        # Helper: check authors (skip validation if no ref_authors)
        def check_and_set_result(source, found_authors):
            if not ref_authors or validate_authors(ref_authors, found_authors):
                result['status'] = 'verified'
                result['source'] = source
            else:
                result['status'] = 'author_mismatch'
                result['error_type'] = 'author_mismatch'
                result['source'] = source
                result['found_authors'] = found_authors

        # 1. OpenAlex (if API key provided)
        # Note: OpenAlex sometimes returns incorrect authors, so on mismatch we check other sources
        if openalex_key:
            logger.info(f"     Querying OpenAlex...")
            found_title, found_authors = query_openalex(title, openalex_key)
            if found_title and found_authors:
                if not ref_authors or validate_authors(ref_authors, found_authors):
                    result['status'] = 'verified'
                    result['source'] = 'OpenAlex'
                    logger.info(f"     -> FOUND & VERIFIED (OpenAlex)")
                    results.append(result)
                    continue
                logger.info(f"     -> Found but author mismatch, checking other sources...")

        # 2. CrossRef
        logger.info(f"     Querying CrossRef...")
        found_title, found_authors = query_crossref(title)
        if found_title:
            check_and_set_result('CrossRef', found_authors)
            logger.info(f"     -> FOUND - {result['status'].upper()} (CrossRef)")
            results.append(result)
            continue
        logger.info(f"     -> Not in CrossRef")

        # 3. arXiv
        logger.info(f"     Querying arXiv...")
        found_title, found_authors = query_arxiv(title)
        if found_title:
            check_and_set_result('arXiv', found_authors)
            logger.info(f"     -> FOUND - {result['status'].upper()} (arXiv)")
            results.append(result)
            continue
        logger.info(f"     -> Not in arXiv")

        # 4. DBLP
        logger.info(f"     Querying DBLP...")
        found_title, found_authors = query_dblp(title)
        if found_title:
            check_and_set_result('DBLP', found_authors)
            logger.info(f"     -> FOUND - {result['status'].upper()} (DBLP)")
            results.append(result)
            continue
        logger.info(f"     -> Not in DBLP")

        # 5. OpenReview (last resort for conference papers)
        logger.info(f"     Querying OpenReview...")
        found_title, found_authors = query_openreview(title)
        if found_title:
            check_and_set_result('OpenReview', found_authors)
            logger.info(f"     -> FOUND - {result['status'].upper()} (OpenReview)")
            results.append(result)
            continue
        logger.info(f"     -> Not in OpenReview")

        # 6. Semantic Scholar (aggregates Academia.edu, SSRN, PubMed, etc.)
        logger.info(f"     Querying Semantic Scholar...")
        found_title, found_authors = query_semantic_scholar(title)
        if found_title:
            check_and_set_result('Semantic Scholar', found_authors)
            logger.info(f"     -> FOUND - {result['status'].upper()} (Semantic Scholar)")
            results.append(result)
            continue
        logger.info(f"     -> Not in Semantic Scholar")

        # Not found in any database
        result['status'] = 'not_found'
        result['error_type'] = 'not_found'
        logger.warning(f"     => NOT FOUND in any database!")
        results.append(result)

    verified = sum(1 for r in results if r['status'] == 'verified')
    not_found = sum(1 for r in results if r['status'] == 'not_found')
    mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')
    logger.info(f"Analysis complete: {verified} verified, {not_found} not found, {mismatched} mismatched")

    return results, skip_stats


@app.route('/')
def index():
    return render_template('index.html')


def analyze_single_pdf(pdf_path, filename, openalex_key=None):
    """Analyze a single PDF and return a file result dict."""
    logger.info(f"--- Processing: {filename} ---")
    try:
        results, skip_stats = analyze_pdf(pdf_path, openalex_key=openalex_key)

        verified = sum(1 for r in results if r['status'] == 'verified')
        not_found = sum(1 for r in results if r['status'] == 'not_found')
        mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')
        total_skipped = skip_stats['skipped_url'] + skip_stats['skipped_short_title']

        return {
            'filename': filename,
            'success': True,
            'summary': {
                'total_raw': skip_stats['total_raw'],
                'total': len(results),
                'verified': verified,
                'not_found': not_found,
                'mismatched': mismatched,
                'skipped': total_skipped,
                'skipped_url': skip_stats['skipped_url'],
                'skipped_short_title': skip_stats['skipped_short_title'],
                'title_only': skip_stats['skipped_no_authors'],
            },
            'results': results,
        }
    except Exception as e:
        logger.error(f"Error processing {filename}: {e}")
        return {
            'filename': filename,
            'success': False,
            'error': str(e),
            'results': [],
        }


@app.route('/analyze', methods=['POST'])
def analyze():
    if 'pdf' not in request.files:
        logger.warning("Request received with no file")
        return jsonify({'error': 'No file provided'}), 400

    uploaded_file = request.files['pdf']
    if uploaded_file.filename == '':
        logger.warning("Request received with empty filename")
        return jsonify({'error': 'No file selected'}), 400

    file_type = get_file_type(uploaded_file.filename)
    if file_type is None:
        logger.warning(f"Unsupported file type: {uploaded_file.filename}")
        return jsonify({'error': 'File must be a PDF, ZIP, or tar.gz archive'}), 400

    openalex_key = request.form.get('openalex_key', '').strip() or None

    logger.info(f"=== New analysis request: {uploaded_file.filename} (type: {file_type}) ===")
    if openalex_key:
        logger.info("OpenAlex API key provided")

    # Create temp directory for all operations
    temp_dir = tempfile.mkdtemp()
    try:
        if file_type == 'pdf':
            # Single PDF - preserve backward compatible response
            temp_path = os.path.join(temp_dir, 'upload.pdf')
            uploaded_file.save(temp_path)
            logger.info(f"Processing single PDF: {uploaded_file.filename}")

            results, skip_stats = analyze_pdf(temp_path, openalex_key=openalex_key)

            verified = sum(1 for r in results if r['status'] == 'verified')
            not_found = sum(1 for r in results if r['status'] == 'not_found')
            mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')
            total_skipped = skip_stats['skipped_url'] + skip_stats['skipped_short_title']

            logger.info(f"=== Analysis complete: {verified} verified, {not_found} not found, {mismatched} mismatched ===")
            return jsonify({
                'success': True,
                'summary': {
                    'total_raw': skip_stats['total_raw'],
                    'total': len(results),
                    'verified': verified,
                    'not_found': not_found,
                    'mismatched': mismatched,
                    'skipped': total_skipped,
                    'skipped_url': skip_stats['skipped_url'],
                    'skipped_short_title': skip_stats['skipped_short_title'],
                    'title_only': skip_stats['skipped_no_authors'],
                },
                'results': results,
            })

        else:
            # Archive - extract and process multiple PDFs
            suffix = '.zip' if file_type == 'zip' else '.tar.gz'
            archive_path = os.path.join(temp_dir, f'archive{suffix}')
            uploaded_file.save(archive_path)

            extract_dir = os.path.join(temp_dir, 'extracted')
            os.makedirs(extract_dir)

            try:
                pdf_files = extract_pdfs_from_archive(archive_path, file_type, extract_dir)
            except ValueError as e:
                return jsonify({'error': str(e)}), 400

            if not pdf_files:
                logger.warning("No PDF files found in archive")
                return jsonify({'error': 'No PDF files found in archive'}), 400

            # Process each PDF
            logger.info(f"Processing {len(pdf_files)} PDF(s) from archive...")
            file_results = []
            for idx, (filename, pdf_path) in enumerate(pdf_files, 1):
                logger.info(f"=== File {idx}/{len(pdf_files)}: {filename} ===")
                file_result = analyze_single_pdf(pdf_path, filename, openalex_key)
                file_results.append(file_result)

            # Aggregate summary across all files
            agg_summary = {
                'total_raw': 0,
                'total': 0,
                'verified': 0,
                'not_found': 0,
                'mismatched': 0,
                'skipped': 0,
                'skipped_url': 0,
                'skipped_short_title': 0,
                'title_only': 0,
            }

            all_results = []
            for fr in file_results:
                if fr['success']:
                    for key in agg_summary:
                        agg_summary[key] += fr['summary'].get(key, 0)
                    all_results.extend(fr['results'])

            successful = sum(1 for fr in file_results if fr['success'])
            failed = len(file_results) - successful
            logger.info(f"=== Archive analysis complete: {successful} files processed, {failed} failed ===")
            logger.info(f"    Total: {agg_summary['verified']} verified, {agg_summary['not_found']} not found, {agg_summary['mismatched']} mismatched")

            return jsonify({
                'success': True,
                'file_count': len(pdf_files),
                'files': file_results,
                'summary': agg_summary,
                'results': all_results,  # Flattened for backward compatibility
            })

    except Exception as e:
        logger.exception(f"Unexpected error during analysis: {e}")
        return jsonify({'error': str(e)}), 500
    finally:
        # Cleanup temp directory
        shutil.rmtree(temp_dir, ignore_errors=True)


if __name__ == '__main__':
    debug = os.environ.get('FLASK_DEBUG', '').lower() in ('1', 'true')
    logger.info(f"Starting Hallucinated Reference Checker on port 5001 (debug={debug})")
    app.run(host='0.0.0.0', port=5001, debug=debug)
