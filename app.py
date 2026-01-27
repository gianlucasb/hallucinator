import os
import tempfile
import urllib.parse
from flask import Flask, render_template, request, jsonify

from check_hallucinated_references import (
    extract_references_with_titles_and_authors,
    query_crossref,
    query_arxiv,
    query_dblp,
    query_openalex,
    query_openreview,
    validate_authors,
)

app = Flask(__name__)


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
    refs, skip_stats = extract_references_with_titles_and_authors(pdf_path, return_stats=True)
    results = []

    for title, ref_authors in refs:
        result = {
            'title': title,
            'status': 'verified',
            'error_type': None,
            'source': None,
            'ref_authors': ref_authors,
            'found_authors': [],
        }

        # 1. OpenAlex (if API key provided)
        if openalex_key:
            found_title, found_authors = query_openalex(title, openalex_key)
            if found_title and found_authors:
                if validate_authors(ref_authors, found_authors):
                    result['status'] = 'verified'
                    result['source'] = 'OpenAlex'
                else:
                    result['status'] = 'author_mismatch'
                    result['error_type'] = 'author_mismatch'
                    result['source'] = 'OpenAlex'
                    result['found_authors'] = found_authors
                results.append(result)
                continue

        # 2. CrossRef
        found_title, found_authors = query_crossref(title)
        if found_title:
            if validate_authors(ref_authors, found_authors):
                result['status'] = 'verified'
                result['source'] = 'CrossRef'
            else:
                result['status'] = 'author_mismatch'
                result['error_type'] = 'author_mismatch'
                result['source'] = 'CrossRef'
                result['found_authors'] = found_authors
            results.append(result)
            continue

        # 3. arXiv
        found_title, found_authors = query_arxiv(title)
        if found_title:
            if validate_authors(ref_authors, found_authors):
                result['status'] = 'verified'
                result['source'] = 'arXiv'
            else:
                result['status'] = 'author_mismatch'
                result['error_type'] = 'author_mismatch'
                result['source'] = 'arXiv'
                result['found_authors'] = found_authors
            results.append(result)
            continue

        # 4. DBLP
        found_title, found_authors = query_dblp(title)
        if found_title:
            if validate_authors(ref_authors, found_authors):
                result['status'] = 'verified'
                result['source'] = 'DBLP'
            else:
                result['status'] = 'author_mismatch'
                result['error_type'] = 'author_mismatch'
                result['source'] = 'DBLP'
                result['found_authors'] = found_authors
            results.append(result)
            continue

        # 5. OpenReview (last resort for conference papers)
        found_title, found_authors = query_openreview(title)
        if found_title:
            if validate_authors(ref_authors, found_authors):
                result['status'] = 'verified'
                result['source'] = 'OpenReview'
            else:
                result['status'] = 'author_mismatch'
                result['error_type'] = 'author_mismatch'
                result['source'] = 'OpenReview'
                result['found_authors'] = found_authors
            results.append(result)
            continue

        # Not found in any database
        result['status'] = 'not_found'
        result['error_type'] = 'not_found'
        results.append(result)

    return results, skip_stats


@app.route('/')
def index():
    return render_template('index.html')


@app.route('/analyze', methods=['POST'])
def analyze():
    if 'pdf' not in request.files:
        return jsonify({'error': 'No PDF file provided'}), 400

    pdf_file = request.files['pdf']
    if pdf_file.filename == '':
        return jsonify({'error': 'No file selected'}), 400

    if not pdf_file.filename.lower().endswith('.pdf'):
        return jsonify({'error': 'File must be a PDF'}), 400

    openalex_key = request.form.get('openalex_key', '').strip() or None

    # Save to temp file
    fd, temp_path = tempfile.mkstemp(suffix='.pdf')
    try:
        os.close(fd)
        pdf_file.save(temp_path)

        results, skip_stats = analyze_pdf(temp_path, openalex_key=openalex_key)

        # Calculate summary stats
        verified = sum(1 for r in results if r['status'] == 'verified')
        not_found = sum(1 for r in results if r['status'] == 'not_found')
        mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')
        total_skipped = skip_stats['skipped_url'] + skip_stats['skipped_short_title'] + skip_stats['skipped_no_authors']

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
                'skipped_no_authors': skip_stats['skipped_no_authors'],
            },
            'results': results,
        })
    except Exception as e:
        return jsonify({'error': str(e)}), 500
    finally:
        # Cleanup temp file
        if os.path.exists(temp_path):
            os.unlink(temp_path)


if __name__ == '__main__':
    app.run(debug=True, port=5001)
