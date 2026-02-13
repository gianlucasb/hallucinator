"""Flask web app using Rust bindings for reference extraction and validation.

This is a drop-in replacement for app.py that uses the hallucinator-rs Rust
engine via Python bindings instead of the pure Python implementation.

Requirements:
    pip install flask
    pip install hallucinator  # from hallucinator-rs/python/

Usage:
    python app-rs.py
    # or with environment variables:
    FLASK_DEBUG=1 S2_API_KEY=xxx OPENALEX_KEY=xxx python app-rs.py

The web interface is identical to app.py - same routes, same SSE events.
"""

import json
import logging
import os
import queue
import shutil
import tarfile
import tempfile
import threading
import zipfile
from typing import Optional

from flask import Flask, Response, jsonify, render_template, request, stream_with_context

# Import Rust bindings
from hallucinator import (
    CheckStats,
    PdfExtractor,
    ProgressEvent,
    ValidationResult,
    Validator,
    ValidatorConfig,
)

# Configure logging
log_level = logging.DEBUG if os.environ.get("FLASK_DEBUG", "").lower() in ("1", "true") else logging.INFO
logging.basicConfig(
    level=log_level,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
logger = logging.getLogger(__name__)

app = Flask(__name__, template_folder="templates", static_folder="static")

# Security limits for archive processing
MAX_FILES_IN_ARCHIVE = 50
MAX_EXTRACTED_SIZE_MB = 500

# Database name mapping (Rust uses slightly different names)
ALL_DATABASES = {
    "crossref",
    "arxiv",
    "dblp",
    "semantic_scholar",
    "acl",
    "neurips",
    "europe_pmc",
    "pubmed",
    "openalex",
    "ssrn",
}

# Offline database paths (optional)
DBLP_OFFLINE_PATH = os.environ.get("DBLP_OFFLINE_PATH")
ACL_OFFLINE_PATH = os.environ.get("ACL_OFFLINE_PATH")


def get_file_type(filename: str) -> Optional[str]:
    """Detect file type from extension."""
    lower = filename.lower()
    if lower.endswith(".pdf"):
        return "pdf"
    elif lower.endswith(".zip"):
        return "zip"
    elif lower.endswith(".tar.gz") or lower.endswith(".tgz"):
        return "tar.gz"
    return None


def safe_filename(filename: str) -> Optional[str]:
    """Check if filename is safe (no path traversal, no hidden files, no __MACOSX)."""
    normalized = filename.replace("\\", "/")
    parts = normalized.split("/")
    for part in parts:
        if part.startswith(".") or part == "__MACOSX":
            return None
    if ".." in normalized or normalized.startswith("/"):
        return None
    return normalized


def is_valid_pdf(file_path: str) -> bool:
    """Check if file has PDF magic bytes."""
    try:
        with open(file_path, "rb") as f:
            header = f.read(5)
            return header == b"%PDF-"
    except Exception:
        return False


def extract_pdfs_from_archive(archive_path: str, file_type: str, extract_dir: str) -> list:
    """Extract PDFs from archive with security limits.

    Returns list of (original_name, extracted_path) tuples.
    """
    pdf_files = []
    total_size = 0
    max_size_bytes = MAX_EXTRACTED_SIZE_MB * 1024 * 1024

    logger.info(f"Extracting {file_type} archive...")

    try:
        if file_type == "zip":
            with zipfile.ZipFile(archive_path, "r") as zf:
                for info in zf.infolist():
                    total_size += info.file_size
                    if total_size > max_size_bytes:
                        raise ValueError(f"Archive exceeds maximum extracted size ({MAX_EXTRACTED_SIZE_MB}MB)")

                for info in zf.infolist():
                    if info.is_dir():
                        continue

                    safe_name = safe_filename(info.filename)
                    if safe_name is None or not safe_name.lower().endswith(".pdf"):
                        continue

                    if len(pdf_files) >= MAX_FILES_IN_ARCHIVE:
                        raise ValueError(f"Too many PDF files in archive (max {MAX_FILES_IN_ARCHIVE})")

                    basename = os.path.basename(safe_name)
                    extract_path = os.path.join(extract_dir, f"{len(pdf_files)}_{basename}")

                    with zf.open(info) as src, open(extract_path, "wb") as dst:
                        dst.write(src.read())

                    if is_valid_pdf(extract_path):
                        pdf_files.append((basename, extract_path))
                        logger.info(f"  Extracted: {basename}")
                    else:
                        os.unlink(extract_path)

        elif file_type == "tar.gz":
            with tarfile.open(archive_path, "r:gz") as tf:
                for member in tf.getmembers():
                    if not member.isfile():
                        continue

                    total_size += member.size
                    if total_size > max_size_bytes:
                        raise ValueError(f"Archive exceeds maximum extracted size ({MAX_EXTRACTED_SIZE_MB}MB)")

                    safe_name = safe_filename(member.name)
                    if safe_name is None or not safe_name.lower().endswith(".pdf"):
                        continue

                    if len(pdf_files) >= MAX_FILES_IN_ARCHIVE:
                        raise ValueError(f"Too many PDF files in archive (max {MAX_FILES_IN_ARCHIVE})")

                    basename = os.path.basename(safe_name)
                    extract_path = os.path.join(extract_dir, f"{len(pdf_files)}_{basename}")

                    with tf.extractfile(member) as src, open(extract_path, "wb") as dst:
                        dst.write(src.read())

                    if is_valid_pdf(extract_path):
                        pdf_files.append((basename, extract_path))
                        logger.info(f"  Extracted: {basename}")
                    else:
                        os.unlink(extract_path)

        logger.info(f"Extracted {len(pdf_files)} PDF(s) from archive")

    except zipfile.BadZipFile:
        raise ValueError("Invalid or corrupted ZIP file")
    except tarfile.TarError as e:
        raise ValueError(f"Invalid or corrupted tar.gz file: {e}")

    return pdf_files


def create_validator_config(
    openalex_key: Optional[str] = None,
    s2_api_key: Optional[str] = None,
    enabled_dbs: Optional[set] = None,
    check_openalex_authors: bool = False,
) -> ValidatorConfig:
    """Create a ValidatorConfig with the given settings."""
    config = ValidatorConfig()

    if openalex_key:
        config.openalex_key = openalex_key
    if s2_api_key:
        config.s2_api_key = s2_api_key
    if DBLP_OFFLINE_PATH:
        config.dblp_offline_path = DBLP_OFFLINE_PATH
    if ACL_OFFLINE_PATH:
        config.acl_offline_path = ACL_OFFLINE_PATH

    # Set disabled databases
    if enabled_dbs is not None:
        disabled = ALL_DATABASES - enabled_dbs
        if disabled:
            config.disabled_dbs = list(disabled)

    config.check_openalex_authors = check_openalex_authors

    return config


def validation_result_to_dict(r: ValidationResult) -> dict:
    """Convert a ValidationResult to a dict matching the Python app.py format."""
    result = {
        "title": r.title,
        "raw_citation": r.raw_citation,
        "status": r.status,
        "error_type": r.status if r.status != "verified" else None,
        "source": r.source,
        "ref_authors": list(r.ref_authors) if r.ref_authors else [],
        "found_authors": list(r.found_authors) if r.found_authors else [],
        "paper_url": r.paper_url,
        "failed_dbs": list(r.failed_dbs) if r.failed_dbs else [],
    }

    # Add DOI info if present
    if r.doi_info:
        result["doi_info"] = {
            "doi": r.doi_info.doi,
            "valid": r.doi_info.valid,
            "error": r.doi_info.error,
        }

    # Add arXiv info if present
    if r.arxiv_info:
        result["arxiv_info"] = {
            "arxiv_id": r.arxiv_info.arxiv_id,
            "valid": r.arxiv_info.valid,
            "error": r.arxiv_info.error,
        }

    # Add retraction info if present
    if r.retraction_info:
        result["retraction_info"] = {
            "is_retracted": r.retraction_info.is_retracted,
            "retraction_doi": r.retraction_info.retraction_doi,
            "retraction_type": r.retraction_info.retraction_type,
            "retraction_source": r.retraction_info.retraction_source,
        }
        if r.retraction_info.is_retracted:
            result["status"] = "retracted"
            result["error_type"] = "retracted"

    return result


def analyze_pdf(
    pdf_path: str,
    config: ValidatorConfig,
    on_progress=None,
) -> tuple:
    """Analyze PDF and return (results, skip_stats).

    Uses Rust bindings for extraction and validation.
    """
    logger.info("Extracting references from PDF...")

    # Extract references using Rust
    extractor = PdfExtractor()
    extraction_result = extractor.extract(pdf_path)
    refs = extraction_result.references
    stats = extraction_result.skip_stats

    logger.info(
        f"Found {len(refs)} references to check "
        f"(skipped {stats.url_only} URLs, {stats.short_title} short titles)"
    )

    skip_stats = {
        "total_raw": stats.total_raw,
        "skipped_url": stats.url_only,
        "skipped_short_title": stats.short_title,
        "skipped_no_authors": stats.no_authors,
    }

    # Notify extraction complete
    if on_progress:
        on_progress("extraction_complete", {
            "total_refs": len(refs),
            "skip_stats": skip_stats,
        })

    if not refs:
        return [], skip_stats

    # Create progress callback for validation
    def rust_progress_callback(event: ProgressEvent):
        if event.event_type == "checking":
            short_title = event.title[:60] + "..." if len(event.title) > 60 else event.title
            logger.info(f"[{event.index + 1}/{event.total}] Checking: {short_title}")
            if on_progress:
                on_progress("checking", {
                    "index": event.index,
                    "total": event.total,
                    "title": event.title,
                })
        elif event.event_type == "result":
            r = event.result
            status = r.status.upper()
            source = f" ({r.source})" if r.source else ""
            logger.info(f"[{event.index + 1}/{event.total}] -> {status}{source}")
            if on_progress:
                result_dict = validation_result_to_dict(r)
                result_dict["index"] = event.index
                result_dict["total"] = event.total
                on_progress("result", result_dict)
        elif event.event_type == "retry_pass":
            logger.info(f"Retrying {event.count} unresolved references...")
            if on_progress:
                on_progress("retry_pass", {"count": event.count})

    # Validate references using Rust
    validator = Validator(config)
    results = validator.check(refs, progress=rust_progress_callback)

    # Convert results to dicts
    result_dicts = [validation_result_to_dict(r) for r in results]

    # Get check stats
    check_stats = Validator.stats(results)

    verified = check_stats.verified
    not_found = check_stats.not_found
    mismatched = check_stats.author_mismatch
    logger.info(f"Analysis complete: {verified} verified, {not_found} not found, {mismatched} mismatched")

    # Add check stats to skip_stats
    skip_stats["total_timeouts"] = 0  # TODO: expose from Rust
    skip_stats["retried_count"] = 0
    skip_stats["retry_successes"] = 0

    return result_dicts, skip_stats


@app.route("/")
def index():
    dblp_offline_path = os.environ.get("DBLP_OFFLINE_PATH", "")
    return render_template("index.html", dblp_offline_path=dblp_offline_path)


@app.route("/analyze", methods=["POST"])
def analyze():
    """Synchronous analysis endpoint (for backward compatibility)."""
    if "pdf" not in request.files:
        return jsonify({"error": "No file provided"}), 400

    uploaded_file = request.files["pdf"]
    if uploaded_file.filename == "":
        return jsonify({"error": "No file selected"}), 400

    file_type = get_file_type(uploaded_file.filename)
    if file_type is None:
        return jsonify({"error": "File must be a PDF, ZIP, or tar.gz archive"}), 400

    openalex_key = request.form.get("openalex_key", "").strip() or None
    s2_api_key = request.form.get("s2_api_key", "").strip() or None
    check_openalex_authors = request.form.get("check_openalex_authors") == "true"

    disabled_dbs_raw = request.form.get("disabled_dbs", "").strip()
    if disabled_dbs_raw:
        disabled_set = set(json.loads(disabled_dbs_raw))
        enabled_dbs = ALL_DATABASES - disabled_set
    else:
        enabled_dbs = None

    logger.info(f"=== New analysis request: {uploaded_file.filename} (type: {file_type}) ===")

    config = create_validator_config(
        openalex_key=openalex_key,
        s2_api_key=s2_api_key,
        enabled_dbs=enabled_dbs,
        check_openalex_authors=check_openalex_authors,
    )

    temp_dir = tempfile.mkdtemp()
    try:
        if file_type == "pdf":
            temp_path = os.path.join(temp_dir, "upload.pdf")
            uploaded_file.save(temp_path)

            results, skip_stats = analyze_pdf(temp_path, config)

            verified = sum(1 for r in results if r["status"] == "verified")
            not_found = sum(1 for r in results if r["status"] == "not_found")
            mismatched = sum(1 for r in results if r["status"] == "author_mismatch")
            total_skipped = skip_stats["skipped_url"] + skip_stats["skipped_short_title"]

            return jsonify({
                "success": True,
                "summary": {
                    "total_raw": skip_stats["total_raw"],
                    "total": len(results),
                    "verified": verified,
                    "not_found": not_found,
                    "mismatched": mismatched,
                    "skipped": total_skipped,
                    "skipped_url": skip_stats["skipped_url"],
                    "skipped_short_title": skip_stats["skipped_short_title"],
                    "title_only": skip_stats["skipped_no_authors"],
                },
                "results": results,
            })

        else:
            # Archive handling
            suffix = ".zip" if file_type == "zip" else ".tar.gz"
            archive_path = os.path.join(temp_dir, f"archive{suffix}")
            uploaded_file.save(archive_path)

            extract_dir = os.path.join(temp_dir, "extracted")
            os.makedirs(extract_dir)

            try:
                pdf_files = extract_pdfs_from_archive(archive_path, file_type, extract_dir)
            except ValueError as e:
                return jsonify({"error": str(e)}), 400

            if not pdf_files:
                return jsonify({"error": "No PDF files found in archive"}), 400

            file_results = []
            for idx, (filename, pdf_path) in enumerate(pdf_files, 1):
                logger.info(f"=== File {idx}/{len(pdf_files)}: {filename} ===")
                try:
                    results, skip_stats = analyze_pdf(pdf_path, config)
                    verified = sum(1 for r in results if r["status"] == "verified")
                    not_found = sum(1 for r in results if r["status"] == "not_found")
                    mismatched = sum(1 for r in results if r["status"] == "author_mismatch")
                    total_skipped = skip_stats["skipped_url"] + skip_stats["skipped_short_title"]

                    file_results.append({
                        "filename": filename,
                        "success": True,
                        "summary": {
                            "total_raw": skip_stats["total_raw"],
                            "total": len(results),
                            "verified": verified,
                            "not_found": not_found,
                            "mismatched": mismatched,
                            "skipped": total_skipped,
                            "skipped_url": skip_stats["skipped_url"],
                            "skipped_short_title": skip_stats["skipped_short_title"],
                            "title_only": skip_stats["skipped_no_authors"],
                        },
                        "results": results,
                    })
                except Exception as e:
                    logger.error(f"Error processing {filename}: {e}")
                    file_results.append({
                        "filename": filename,
                        "success": False,
                        "error": str(e),
                        "results": [],
                    })

            # Aggregate summary
            agg_summary = {
                "total_raw": 0,
                "total": 0,
                "verified": 0,
                "not_found": 0,
                "mismatched": 0,
                "skipped": 0,
                "skipped_url": 0,
                "skipped_short_title": 0,
                "title_only": 0,
            }
            all_results = []
            for fr in file_results:
                if fr["success"]:
                    for key in agg_summary:
                        agg_summary[key] += fr["summary"].get(key, 0)
                    all_results.extend(fr["results"])

            return jsonify({
                "success": True,
                "file_count": len(pdf_files),
                "files": file_results,
                "summary": agg_summary,
                "results": all_results,
            })

    except Exception as e:
        logger.exception(f"Unexpected error: {e}")
        return jsonify({"error": str(e)}), 500
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


@app.route("/analyze/stream", methods=["POST"])
def analyze_stream():
    """SSE endpoint for streaming analysis progress."""
    if "pdf" not in request.files:
        return jsonify({"error": "No file provided"}), 400

    uploaded_file = request.files["pdf"]
    if uploaded_file.filename == "":
        return jsonify({"error": "No file selected"}), 400

    file_type = get_file_type(uploaded_file.filename)
    if file_type is None:
        return jsonify({"error": "File must be a PDF, ZIP, or tar.gz archive"}), 400

    openalex_key = request.form.get("openalex_key", "").strip() or None
    s2_api_key = request.form.get("s2_api_key", "").strip() or None
    check_openalex_authors = request.form.get("check_openalex_authors") == "true"

    disabled_dbs_raw = request.form.get("disabled_dbs", "").strip()
    if disabled_dbs_raw:
        disabled_set = set(json.loads(disabled_dbs_raw))
        enabled_dbs = ALL_DATABASES - disabled_set
    else:
        enabled_dbs = None

    logger.info(f"=== New streaming analysis: {uploaded_file.filename} (type: {file_type}) ===")

    config = create_validator_config(
        openalex_key=openalex_key,
        s2_api_key=s2_api_key,
        enabled_dbs=enabled_dbs,
        check_openalex_authors=check_openalex_authors,
    )

    # Save file to temp directory
    temp_dir = tempfile.mkdtemp()
    pdf_files = []

    try:
        if file_type == "pdf":
            temp_path = os.path.join(temp_dir, "upload.pdf")
            uploaded_file.save(temp_path)
            pdf_files = [(uploaded_file.filename, temp_path)]
        else:
            suffix = ".zip" if file_type == "zip" else ".tar.gz"
            archive_path = os.path.join(temp_dir, f"archive{suffix}")
            uploaded_file.save(archive_path)

            extract_dir = os.path.join(temp_dir, "extracted")
            os.makedirs(extract_dir)

            try:
                pdf_files = extract_pdfs_from_archive(archive_path, file_type, extract_dir)
            except ValueError as e:
                shutil.rmtree(temp_dir, ignore_errors=True)
                return jsonify({"error": str(e)}), 400

            if not pdf_files:
                shutil.rmtree(temp_dir, ignore_errors=True)
                return jsonify({"error": "No PDF files found in archive"}), 400

    except Exception as e:
        shutil.rmtree(temp_dir, ignore_errors=True)
        return jsonify({"error": str(e)}), 500

    is_archive = len(pdf_files) > 1

    def generate():
        """Generator for SSE events."""
        event_queue = queue.Queue()
        all_file_results = []

        def on_progress(event_type, data):
            event_queue.put((event_type, data))

        def run_analysis():
            try:
                if is_archive:
                    event_queue.put(("archive_start", {"file_count": len(pdf_files)}))

                for file_idx, (filename, pdf_path) in enumerate(pdf_files):
                    event_queue.put(("file_start", {
                        "file_index": file_idx,
                        "file_count": len(pdf_files),
                        "filename": filename,
                    }))

                    try:
                        results, skip_stats = analyze_pdf(pdf_path, config, on_progress=on_progress)

                        verified = sum(1 for r in results if r["status"] == "verified")
                        not_found = sum(1 for r in results if r["status"] == "not_found")
                        mismatched = sum(1 for r in results if r["status"] == "author_mismatch")
                        total_skipped = skip_stats.get("skipped_url", 0) + skip_stats.get("skipped_short_title", 0)

                        file_result = {
                            "filename": filename,
                            "success": True,
                            "summary": {
                                "total_raw": skip_stats.get("total_raw", 0),
                                "total": len(results),
                                "verified": verified,
                                "not_found": not_found,
                                "mismatched": mismatched,
                                "skipped": total_skipped,
                                "skipped_url": skip_stats.get("skipped_url", 0),
                                "skipped_short_title": skip_stats.get("skipped_short_title", 0),
                                "title_only": skip_stats.get("skipped_no_authors", 0),
                            },
                            "results": results,
                        }
                        all_file_results.append(file_result)
                        event_queue.put(("file_complete", file_result))

                    except Exception as e:
                        logger.error(f"Error processing {filename}: {e}")
                        file_result = {
                            "filename": filename,
                            "success": False,
                            "error": str(e),
                            "results": [],
                        }
                        all_file_results.append(file_result)
                        event_queue.put(("file_complete", file_result))

                event_queue.put(("analysis_done", None))

            except Exception as e:
                event_queue.put(("error", {"message": str(e)}))
            finally:
                event_queue.put(("done", None))

        # Start analysis in background thread
        analysis_thread = threading.Thread(target=run_analysis)
        analysis_thread.start()

        try:
            while True:
                try:
                    event_type, data = event_queue.get(timeout=30)
                except queue.Empty:
                    yield b": keepalive\n\n"
                    continue

                if event_type == "done":
                    break
                elif event_type == "error":
                    yield f"event: error\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                    break
                elif event_type == "archive_start":
                    yield f"event: archive_start\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "file_start":
                    yield f"event: file_start\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "file_complete":
                    yield f"event: file_complete\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "extraction_complete":
                    yield f"event: extraction_complete\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "retry_pass":
                    yield f"event: retry_pass\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "checking":
                    yield f"event: checking\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "result":
                    yield f"event: result\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "warning":
                    yield f"event: warning\ndata: {json.dumps(data)}\n\n".encode("utf-8")
                elif event_type == "analysis_done":
                    # Aggregate stats
                    agg_summary = {
                        "total_raw": 0,
                        "total": 0,
                        "verified": 0,
                        "not_found": 0,
                        "mismatched": 0,
                        "skipped": 0,
                        "skipped_url": 0,
                        "skipped_short_title": 0,
                        "title_only": 0,
                    }
                    all_results = []
                    for fr in all_file_results:
                        if fr.get("success"):
                            for key in agg_summary:
                                agg_summary[key] += fr["summary"].get(key, 0)
                            all_results.extend(fr["results"])

                    complete_data = {
                        "summary": agg_summary,
                        "results": all_results,
                    }
                    if is_archive:
                        complete_data["file_count"] = len(pdf_files)
                        complete_data["files"] = all_file_results

                    yield f"event: complete\ndata: {json.dumps(complete_data)}\n\n".encode("utf-8")

        finally:
            analysis_thread.join(timeout=5)
            shutil.rmtree(temp_dir, ignore_errors=True)

    response = Response(
        stream_with_context(generate()),
        mimetype="text/event-stream",
        headers={
            "Cache-Control": "no-cache, no-store, must-revalidate",
            "Pragma": "no-cache",
            "Expires": "0",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )
    response.direct_passthrough = True
    return response


@app.route("/retry", methods=["POST"])
def retry_reference():
    """Retry querying specific databases for a reference that timed out.

    Note: The Rust bindings don't currently support creating Reference objects
    directly from Python, so this endpoint is not fully implemented.
    The retry functionality would need to be added to the Rust bindings.

    For now, this returns an error indicating the limitation.
    """
    data = request.get_json()
    if not data:
        return jsonify({"error": "No data provided"}), 400

    title = data.get("title")
    failed_dbs = data.get("failed_dbs", [])

    if not title:
        return jsonify({"error": "Title is required"}), 400
    if not failed_dbs:
        return jsonify({"error": "No databases to retry"}), 400

    logger.info(f"Retry request for: {title[:50]}... (DBs: {', '.join(failed_dbs)})")

    # TODO: The Rust bindings need to expose either:
    # 1. A Reference constructor for Python
    # 2. A Validator.retry_single(title, authors, dbs) method
    #
    # For now, return an error explaining the limitation
    return jsonify({
        "error": "Retry not yet supported with Rust backend. "
                 "The hallucinator-rs Python bindings need to expose a retry API.",
        "suggestion": "Use the original app.py for retry functionality, or wait for "
                      "hallucinator-rs to add Reference construction support.",
    }), 501  # 501 Not Implemented


if __name__ == "__main__":
    debug = os.environ.get("FLASK_DEBUG", "").lower() in ("1", "true")
    logger.info(f"Starting Hallucinated Reference Checker (Rust backend) on port 5001 (debug={debug})")
    logger.info("Using hallucinator-rs Python bindings for extraction and validation")
    app.run(host="0.0.0.0", port=5001, debug=debug)
