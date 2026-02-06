"""Tests for analysis cancellation functionality."""

import threading
import pytest
from unittest.mock import patch, MagicMock


def _make_db_result(status='verified', source='CrossRef', found_authors=None, failed_dbs=None):
    """Create a mock return value for query_all_databases_concurrent."""
    return {
        'status': status,
        'source': source,
        'found_authors': found_authors or ['Author A'],
        'paper_url': 'http://example.com',
        'error_type': None if status == 'verified' else status,
        'failed_dbs': failed_dbs or [],
    }


def _make_refs(count):
    """Create dummy reference tuples."""
    return [
        (f"Title of Paper Number {i}", ["Author A", "Author B"], None, None)
        for i in range(count)
    ]


class TestCheckReferencesCancellation:
    """Tests for cancel_event in check_references()."""

    @patch('check_hallucinated_references.query_all_databases_concurrent')
    def test_cancel_before_start_returns_empty(self, mock_query):
        """Cancelling before check_references starts should return immediately."""
        from check_hallucinated_references import check_references

        cancel_event = threading.Event()
        cancel_event.set()  # Already cancelled

        refs = _make_refs(5)
        results, stats = check_references(refs, cancel_event=cancel_event)

        # Should not have queried any databases
        mock_query.assert_not_called()
        assert len(results) == 0
        assert stats['retried_count'] == 0
        assert stats['retry_successes'] == 0

    @patch('check_hallucinated_references.query_all_databases_concurrent')
    def test_cancel_stops_processing_remaining_refs(self, mock_query):
        """Setting cancel_event mid-run should stop processing remaining refs."""
        from check_hallucinated_references import check_references

        cancel_event = threading.Event()
        call_count = 0

        def slow_query(title, ref_authors, **kwargs):
            nonlocal call_count
            call_count += 1
            # Cancel after first reference is processed
            if call_count >= 1:
                cancel_event.set()
            return _make_db_result()

        mock_query.side_effect = slow_query

        refs = _make_refs(10)
        results, stats = check_references(
            refs, cancel_event=cancel_event, max_concurrent_refs=1
        )

        # Should have processed far fewer than all 10
        assert call_count < 10
        assert stats['retried_count'] == 0

    @patch('check_hallucinated_references.query_all_databases_concurrent')
    def test_cancel_skips_retry_pass(self, mock_query):
        """Cancellation should skip the retry pass entirely."""
        from check_hallucinated_references import check_references

        cancel_event = threading.Event()

        def mock_query_fn(title, ref_authors, **kwargs):
            cancel_event.set()
            return _make_db_result(
                status='not_found', source=None,
                found_authors=[], failed_dbs=['CrossRef', 'DBLP']
            )

        mock_query.side_effect = mock_query_fn

        refs = _make_refs(1)
        results, stats = check_references(
            refs, cancel_event=cancel_event, max_concurrent_refs=1
        )

        # Retries should have been skipped
        assert stats['retried_count'] == 0
        assert stats['retry_successes'] == 0

    @patch('check_hallucinated_references.query_all_databases_concurrent')
    def test_no_cancel_event_works_normally(self, mock_query):
        """When cancel_event is None, check_references works as before."""
        from check_hallucinated_references import check_references

        mock_query.return_value = _make_db_result()

        refs = _make_refs(3)
        results, stats = check_references(refs, cancel_event=None, max_concurrent_refs=1)

        assert len(results) == 3
        assert all(r['status'] == 'verified' for r in results)
        assert mock_query.call_count == 3

    @patch('check_hallucinated_references.query_all_databases_concurrent')
    def test_cancel_filters_none_results(self, mock_query):
        """Cancelled run should filter out None entries from results."""
        from check_hallucinated_references import check_references

        cancel_event = threading.Event()
        processed = 0

        def mock_query_fn(title, ref_authors, **kwargs):
            nonlocal processed
            processed += 1
            if processed >= 2:
                cancel_event.set()
            return _make_db_result()

        mock_query.side_effect = mock_query_fn

        refs = _make_refs(5)
        results, stats = check_references(
            refs, cancel_event=cancel_event, max_concurrent_refs=1
        )

        # All returned results should be non-None dicts
        assert all(r is not None for r in results)
        assert all(isinstance(r, dict) for r in results)

    @patch('check_hallucinated_references.query_all_databases_concurrent')
    def test_cancel_with_progress_callback(self, mock_query):
        """Cancel should work correctly with on_progress callback."""
        from check_hallucinated_references import check_references

        cancel_event = threading.Event()
        progress_events = []

        def on_progress(event_type, data):
            progress_events.append((event_type, data))

        def mock_query_fn(title, ref_authors, **kwargs):
            cancel_event.set()
            return _make_db_result()

        mock_query.side_effect = mock_query_fn

        refs = _make_refs(3)
        results, stats = check_references(
            refs, cancel_event=cancel_event, on_progress=on_progress,
            max_concurrent_refs=1
        )

        # Should have some progress events but not for all refs
        checking_events = [e for e in progress_events if e[0] == 'checking']
        assert len(checking_events) < 3


class TestAnalyzePdfCancellation:
    """Tests for cancel_event in analyze_pdf()."""

    @patch('app.check_references')
    @patch('app.extract_references_with_titles_and_authors')
    def test_cancel_event_passed_to_check_references(self, mock_extract, mock_check):
        """analyze_pdf should pass cancel_event to check_references."""
        from app import analyze_pdf

        mock_extract.return_value = (
            [("A Title of a Test Paper", ["A. Author"], None, None)],
            {'total_raw': 1, 'skipped_url': 0, 'skipped_short_title': 0, 'skipped_no_authors': 0}
        )
        mock_check.return_value = (
            [{'title': 'Test', 'status': 'verified', 'source': 'CrossRef',
              'ref_authors': [], 'found_authors': [], 'error_type': None}],
            {'total_timeouts': 0, 'retried_count': 0, 'retry_successes': 0}
        )

        cancel_event = threading.Event()
        analyze_pdf('/fake/path.pdf', cancel_event=cancel_event)

        # Verify cancel_event was passed through
        mock_check.assert_called_once()
        call_kwargs = mock_check.call_args[1]
        assert call_kwargs.get('cancel_event') is cancel_event


class TestCancelButtonInUI:
    """Tests for the cancel button in the web UI."""

    def test_cancel_button_exists_in_template(self):
        """Verify cancel button is present in the HTML template."""
        from pathlib import Path
        template = Path('/home/user/hallucinator/templates/index.html').read_text()
        assert 'id="cancelBtn"' in template
        assert 'Cancel Analysis' in template

    def test_cancel_button_has_confirmation(self):
        """Verify cancel button triggers a confirmation dialog."""
        from pathlib import Path
        template = Path('/home/user/hallucinator/templates/index.html').read_text()
        assert 'confirm(' in template
        assert 'Are you sure' in template

    def test_abort_controller_used(self):
        """Verify AbortController is used for cancellation."""
        from pathlib import Path
        template = Path('/home/user/hallucinator/templates/index.html').read_text()
        assert 'AbortController' in template
        assert 'currentAbortController.abort()' in template

    def test_cancelled_event_handled(self):
        """Verify the 'cancelled' SSE event is handled in the UI."""
        from pathlib import Path
        template = Path('/home/user/hallucinator/templates/index.html').read_text()
        assert 'cancelled' in template
        assert 'Analysis cancelled' in template
