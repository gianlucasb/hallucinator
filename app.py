import logging
import os
import shutil
import tempfile
import tarfile
import urllib.parse
import zipfile
import json
import queue
import threading
import sys
from flask import Flask, render_template, request, jsonify, Response, stream_with_context

from check_hallucinated_references import (
    extract_references_with_titles_and_authors,
    query_crossref,
    query_arxiv,
    query_dblp,
    query_openalex,
    query_openreview,
    query_semantic_scholar,
    validate_authors,
    check_references,
    query_all_databases_concurrent,
)

# Configure logging
log_level = logging.DEBUG if os.environ.get('FLASK_DEBUG', '').lower() in ('1', 'true') else logging.INFO
logging.basicConfig(
    level=log_level,
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


def analyze_pdf(pdf_path, openalex_key=None, s2_api_key=None, on_progress=None):
    """Analyze PDF and return structured results.

    Args:
        pdf_path: Path to PDF file
        openalex_key: Optional OpenAlex API key
        s2_api_key: Optional Semantic Scholar API key
        on_progress: Optional callback function(event_type, data)
            event_type can be: 'extraction_complete', 'checking', 'result', 'warning'

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

    # Notify extraction complete
    if on_progress:
        on_progress('extraction_complete', {
            'total_refs': len(refs),
            'skip_stats': skip_stats,
        })

    # Progress wrapper that also logs
    def progress_wrapper(event_type, data):
        if event_type == 'checking':
            short_title = data['title'][:60] + '...' if len(data['title']) > 60 else data['title']
            logger.info(f"[{data['index']+1}/{data['total']}] Checking: {short_title}")
        elif event_type == 'result':
            status = data['status'].upper()
            source = f" ({data['source']})" if data['source'] else ""
            logger.info(f"[{data['index']+1}/{data['total']}] -> {status}{source}")
        elif event_type == 'warning':
            logger.warning(f"[{data['index']+1}/{data['total']}] {data['message']}")

        if on_progress:
            on_progress(event_type, data)

    # Use concurrent checking
    results, check_stats = check_references(
        refs,
        sleep_time=1.0,
        openalex_key=openalex_key,
        s2_api_key=s2_api_key,
        on_progress=progress_wrapper
    )

    verified = sum(1 for r in results if r['status'] == 'verified')
    not_found = sum(1 for r in results if r['status'] == 'not_found')
    mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')
    logger.info(f"Analysis complete: {verified} verified, {not_found} not found, {mismatched} mismatched")

    # Merge check_stats into skip_stats for convenience
    skip_stats['total_timeouts'] = check_stats['total_timeouts']
    skip_stats['retried_count'] = check_stats['retried_count']
    skip_stats['retry_successes'] = check_stats['retry_successes']

    return results, skip_stats


@app.route('/')
def index():
    return render_template('index.html')


def analyze_single_pdf(pdf_path, filename, openalex_key=None, s2_api_key=None):
    """Analyze a single PDF and return a file result dict."""
    logger.info(f"--- Processing: {filename} ---")
    try:
        results, skip_stats = analyze_pdf(pdf_path, openalex_key=openalex_key, s2_api_key=s2_api_key)

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
    s2_api_key = request.form.get('s2_api_key', '').strip() or None

    logger.info(f"=== New analysis request: {uploaded_file.filename} (type: {file_type}) ===")
    if openalex_key:
        logger.info("OpenAlex API key provided")
    if s2_api_key:
        logger.info("Semantic Scholar API key provided")

    # Create temp directory for all operations
    temp_dir = tempfile.mkdtemp()
    try:
        if file_type == 'pdf':
            # Single PDF - preserve backward compatible response
            temp_path = os.path.join(temp_dir, 'upload.pdf')
            uploaded_file.save(temp_path)
            logger.info(f"Processing single PDF: {uploaded_file.filename}")

            results, skip_stats = analyze_pdf(temp_path, openalex_key=openalex_key, s2_api_key=s2_api_key)

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
                file_result = analyze_single_pdf(pdf_path, filename, openalex_key, s2_api_key)
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


@app.route('/analyze/stream', methods=['POST'])
def analyze_stream():
    """SSE endpoint for streaming analysis progress (supports single PDFs and archives)."""
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
    s2_api_key = request.form.get('s2_api_key', '').strip() or None

    logger.info(f"=== New streaming analysis request: {uploaded_file.filename} (type: {file_type}) ===")

    # Create temp directory and save file
    temp_dir = tempfile.mkdtemp()
    pdf_files = []  # List of (filename, path) tuples

    try:
        if file_type == 'pdf':
            temp_path = os.path.join(temp_dir, 'upload.pdf')
            uploaded_file.save(temp_path)
            pdf_files = [(uploaded_file.filename, temp_path)]
        else:
            # Archive handling
            suffix = '.zip' if file_type == 'zip' else '.tar.gz'
            archive_path = os.path.join(temp_dir, f'archive{suffix}')
            uploaded_file.save(archive_path)

            extract_dir = os.path.join(temp_dir, 'extracted')
            os.makedirs(extract_dir)

            try:
                pdf_files = extract_pdfs_from_archive(archive_path, file_type, extract_dir)
            except ValueError as e:
                shutil.rmtree(temp_dir, ignore_errors=True)
                return jsonify({'error': str(e)}), 400

            if not pdf_files:
                shutil.rmtree(temp_dir, ignore_errors=True)
                return jsonify({'error': 'No PDF files found in archive'}), 400
    except Exception as e:
        shutil.rmtree(temp_dir, ignore_errors=True)
        return jsonify({'error': str(e)}), 500

    is_archive = len(pdf_files) > 1

    def generate():
        """Generator for SSE events."""
        event_queue = queue.Queue()
        all_file_results = []  # List of per-file result dicts
        current_file_results = []
        current_skip_stats = [None]
        current_filename = [None]

        def on_progress(event_type, data):
            logger.debug(f"SSE: Queueing event {event_type}")
            # Add current filename context for archive mode
            if is_archive and current_filename[0]:
                data = dict(data) if data else {}
                data['filename'] = current_filename[0]
            event_queue.put((event_type, data))

        def run_analysis():
            try:
                # Send archive_start event if processing multiple files
                if is_archive:
                    event_queue.put(('archive_start', {'file_count': len(pdf_files)}))

                for file_idx, (filename, pdf_path) in enumerate(pdf_files):
                    current_filename[0] = filename
                    current_file_results.clear()

                    # Send file_start event
                    event_queue.put(('file_start', {
                        'file_index': file_idx,
                        'file_count': len(pdf_files),
                        'filename': filename,
                    }))

                    try:
                        results, skip_stats = analyze_pdf(pdf_path, openalex_key=openalex_key, s2_api_key=s2_api_key, on_progress=on_progress)
                        current_skip_stats[0] = skip_stats
                        current_file_results.extend(results)

                        # Calculate file summary
                        verified = sum(1 for r in results if r['status'] == 'verified')
                        not_found = sum(1 for r in results if r['status'] == 'not_found')
                        mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')
                        total_skipped = skip_stats.get('skipped_url', 0) + skip_stats.get('skipped_short_title', 0)

                        file_result = {
                            'filename': filename,
                            'success': True,
                            'summary': {
                                'total_raw': skip_stats.get('total_raw', 0),
                                'total': len(results),
                                'verified': verified,
                                'not_found': not_found,
                                'mismatched': mismatched,
                                'skipped': total_skipped,
                                'skipped_url': skip_stats.get('skipped_url', 0),
                                'skipped_short_title': skip_stats.get('skipped_short_title', 0),
                                'title_only': skip_stats.get('skipped_no_authors', 0),
                                'total_timeouts': skip_stats.get('total_timeouts', 0),
                                'retried_count': skip_stats.get('retried_count', 0),
                                'retry_successes': skip_stats.get('retry_successes', 0),
                            },
                            'results': results,
                        }
                        all_file_results.append(file_result)

                        # Send file_complete event
                        event_queue.put(('file_complete', file_result))

                    except Exception as e:
                        logger.error(f"Error processing {filename}: {e}")
                        file_result = {
                            'filename': filename,
                            'success': False,
                            'error': str(e),
                            'results': [],
                        }
                        all_file_results.append(file_result)
                        event_queue.put(('file_complete', file_result))

                event_queue.put(('analysis_done', None))
            except Exception as e:
                event_queue.put(('error', {'message': str(e)}))
            finally:
                event_queue.put(('done', None))

        # Start analysis in background thread
        analysis_thread = threading.Thread(target=run_analysis)
        analysis_thread.start()

        try:
            while True:
                try:
                    event_type, data = event_queue.get(timeout=30)
                except queue.Empty:
                    # Send keepalive
                    yield b": keepalive\n\n"
                    continue

                if event_type == 'done':
                    logger.debug("SSE: Done signal received")
                    break
                elif event_type == 'error':
                    logger.debug(f"SSE: Sending error event")
                    yield f"event: error\ndata: {json.dumps(data)}\n\n".encode('utf-8')
                    break
                elif event_type == 'archive_start':
                    logger.debug(f"SSE: Sending archive_start event")
                    yield f"event: archive_start\ndata: {json.dumps(data)}\n\n".encode('utf-8')
                elif event_type == 'file_start':
                    logger.debug(f"SSE: Sending file_start event for {data.get('filename')}")
                    yield f"event: file_start\ndata: {json.dumps(data)}\n\n".encode('utf-8')
                elif event_type == 'file_complete':
                    logger.debug(f"SSE: Sending file_complete event for {data.get('filename')}")
                    yield f"event: file_complete\ndata: {json.dumps(data)}\n\n".encode('utf-8')
                elif event_type == 'extraction_complete':
                    logger.debug(f"SSE: Sending extraction_complete event")
                    yield f"event: extraction_complete\ndata: {json.dumps(data)}\n\n".encode('utf-8')
                elif event_type == 'retry_pass':
                    logger.debug(f"SSE: Sending retry_pass event")
                    yield f"event: retry_pass\ndata: {json.dumps(data)}\n\n".encode('utf-8')
                elif event_type == 'checking':
                    logger.debug(f"SSE: Sending checking event for index {data.get('index')}")
                    yield f"event: checking\ndata: {json.dumps(data)}\n\n".encode('utf-8')
                elif event_type == 'result':
                    # Include full result data
                    result_data = {
                        'index': data['index'],
                        'total': data['total'],
                        'title': data['title'],
                        'status': data['status'],
                        'source': data['source'],
                    }
                    if 'filename' in data:
                        result_data['filename'] = data['filename']
                    logger.debug(f"SSE: Sending result event for index {data.get('index')}")
                    yield f"event: result\ndata: {json.dumps(result_data)}\n\n".encode('utf-8')
                elif event_type == 'warning':
                    warning_data = {
                        'index': data['index'],
                        'total': data['total'],
                        'title': data['title'],
                        'failed_dbs': data['failed_dbs'],
                        'message': data['message'],
                    }
                    if 'filename' in data:
                        warning_data['filename'] = data['filename']
                    logger.debug(f"SSE: Sending warning event for index {data.get('index')}")
                    yield f"event: warning\ndata: {json.dumps(warning_data)}\n\n".encode('utf-8')
                elif event_type == 'analysis_done':
                    # Send complete event with aggregated summary
                    logger.debug("SSE: Sending complete event")

                    # Aggregate stats across all files
                    agg_summary = {
                        'total_raw': 0, 'total': 0, 'verified': 0, 'not_found': 0,
                        'mismatched': 0, 'skipped': 0, 'skipped_url': 0,
                        'skipped_short_title': 0, 'title_only': 0,
                        'total_timeouts': 0, 'retried_count': 0, 'retry_successes': 0,
                    }
                    all_results = []
                    for fr in all_file_results:
                        if fr.get('success'):
                            for key in agg_summary:
                                agg_summary[key] += fr['summary'].get(key, 0)
                            all_results.extend(fr['results'])

                    complete_data = {
                        'summary': agg_summary,
                        'results': all_results,
                    }
                    if is_archive:
                        complete_data['file_count'] = len(pdf_files)
                        complete_data['files'] = all_file_results

                    yield f"event: complete\ndata: {json.dumps(complete_data)}\n\n".encode('utf-8')

        finally:
            analysis_thread.join(timeout=1)
            shutil.rmtree(temp_dir, ignore_errors=True)

    response = Response(
        stream_with_context(generate()),
        mimetype='text/event-stream',
        headers={
            'Cache-Control': 'no-cache, no-store, must-revalidate',
            'Pragma': 'no-cache',
            'Expires': '0',
            'Connection': 'keep-alive',
            'X-Accel-Buffering': 'no',
        }
    )
    response.direct_passthrough = True
    return response


if __name__ == '__main__':
    debug = os.environ.get('FLASK_DEBUG', '').lower() in ('1', 'true')
    logger.info(f"Starting Hallucinated Reference Checker on port 5001 (debug={debug})")
    app.run(host='0.0.0.0', port=5001, debug=debug)
