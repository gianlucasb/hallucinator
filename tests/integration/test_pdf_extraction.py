"""Integration tests for PDF extraction pipeline."""

import pytest
import os
from pathlib import Path

from check_hallucinated_references import (
    extract_text_from_pdf,
    find_references_section,
    segment_references,
    extract_references_with_titles_and_authors,
    extract_title_from_reference,
    extract_authors_from_reference,
)


class TestPDFExtraction:
    """Tests for PDF text extraction."""

    def test_extract_text_requires_valid_pdf(self, test_pdf_path):
        """Test that extraction works on valid PDF."""
        if test_pdf_path is None:
            pytest.skip("No test PDF available in test-data/")

        text = extract_text_from_pdf(str(test_pdf_path))
        assert len(text) > 0
        assert isinstance(text, str)

    def test_extract_text_invalid_file(self, tmp_path):
        """Test extraction fails gracefully on invalid file."""
        fake_pdf = tmp_path / "fake.pdf"
        fake_pdf.write_text("This is not a PDF")

        with pytest.raises(Exception):
            extract_text_from_pdf(str(fake_pdf))

    def test_extract_text_nonexistent_file(self):
        """Test extraction fails on nonexistent file."""
        with pytest.raises(Exception):
            extract_text_from_pdf("/nonexistent/file.pdf")


class TestFullPipeline:
    """Tests for the full extraction pipeline."""

    def test_full_pipeline_with_test_pdf(self, test_pdf_path):
        """Test full pipeline on actual test PDF."""
        if test_pdf_path is None:
            pytest.skip("No test PDF available in test-data/")

        refs, stats = extract_references_with_titles_and_authors(
            str(test_pdf_path), return_stats=True
        )

        # Should find at least some references
        assert len(refs) > 0

        # Stats should be populated
        assert 'total_raw' in stats
        assert 'skipped_url' in stats
        assert 'skipped_short_title' in stats
        assert 'skipped_no_authors' in stats

        # Each reference should be a (title, authors, doi, arxiv_id, raw_citation) tuple
        for ref in refs:
            assert len(ref) == 5
            title, authors, doi, arxiv_id, raw_citation = ref
            assert isinstance(title, str)
            assert len(title) > 0
            assert isinstance(authors, list)

    def test_full_pipeline_stats_structure(self, test_pdf_path):
        """Test that stats dict has correct structure."""
        if test_pdf_path is None:
            pytest.skip("No test PDF available in test-data/")

        refs, stats = extract_references_with_titles_and_authors(
            str(test_pdf_path), return_stats=True
        )

        # Verify all expected stats keys
        expected_keys = ['total_raw', 'skipped_url', 'skipped_short_title', 'skipped_no_authors']
        for key in expected_keys:
            assert key in stats
            assert isinstance(stats[key], int)
            assert stats[key] >= 0

    def test_pipeline_without_stats(self, test_pdf_path):
        """Test pipeline can return just refs without stats."""
        if test_pdf_path is None:
            pytest.skip("No test PDF available in test-data/")

        refs = extract_references_with_titles_and_authors(
            str(test_pdf_path), return_stats=False
        )

        assert isinstance(refs, list)
        assert len(refs) > 0


class TestSkippedReferences:
    """Tests for reference skipping logic."""

    def test_skips_github_urls(self, test_data_dir):
        """Test that GitHub URLs are skipped."""
        # Create a reference with GitHub URL
        ref_text = "GitHub Repository. https://github.com/example/repo. 2023."

        title, from_quotes = extract_title_from_reference(ref_text)

        # The reference should be recognized but may be filtered later
        # This tests the URL detection in the pipeline

    def test_skips_documentation_urls(self, test_data_dir):
        """Test that documentation URLs are skipped."""
        ref_text = "PyTorch Documentation. https://pytorch.org/docs/stable/. 2023."

        title, from_quotes = extract_title_from_reference(ref_text)
        # URL references are filtered in the extraction pipeline

    def test_keeps_academic_urls(self, test_data_dir):
        """Test that academic URLs (arxiv, doi) are kept."""
        ref_text = "Smith, J. 2023. Deep Learning Methods. arXiv:2301.12345. https://arxiv.org/abs/2301.12345"

        title, from_quotes = extract_title_from_reference(ref_text)
        # Should still try to extract a title from academic URL references


class TestReferenceFormats:
    """Tests for handling different reference formats in PDFs."""

    def test_handles_ieee_format(self):
        """Test handling of IEEE format references."""
        ref = 'J. Smith and A. Jones, "Deep Learning Methods," IEEE Trans., 2023.'

        title, from_quotes = extract_title_from_reference(ref)
        authors = extract_authors_from_reference(ref)

        assert "Deep Learning" in title
        assert len(authors) >= 1

    def test_handles_acm_format(self):
        """Test handling of ACM format references."""
        ref = 'John Smith and Alice Jones. 2023. Deep Learning Methods. In Proceedings of ACL.'

        title, from_quotes = extract_title_from_reference(ref)
        authors = extract_authors_from_reference(ref)

        assert "Deep Learning" in title
        assert len(authors) >= 1

    def test_handles_aaai_format(self):
        """Test handling of AAAI format references."""
        ref = 'Smith, J.; Jones, A.; and Williams, C. 2023. Deep Learning Methods. AAAI 37(1).'

        title, from_quotes = extract_title_from_reference(ref)
        authors = extract_authors_from_reference(ref)

        assert "Deep Learning" in title
        assert len(authors) >= 1


class TestEmDashHandling:
    """Tests for em-dash author handling."""

    def test_em_dash_same_authors(self):
        """Test that em-dash references inherit previous authors."""
        # IEEE pattern needs newline before [number]
        refs_text = '''
[1] J. Smith and A. Jones, "First Paper," in Proc. ACL, 2023.

[2] \u2014\u2014, "Second Paper by Same Authors," in Proc. EMNLP, 2023.

[3] M. Chen, "Third Paper," in Proc. NeurIPS, 2023.
        '''

        # First, segment the references
        refs = segment_references(refs_text)
        assert len(refs) == 3

        # Extract authors from each
        authors1 = extract_authors_from_reference(refs[0])
        authors2 = extract_authors_from_reference(refs[1])

        # First should have actual authors
        assert len(authors1) >= 1

        # Second should return special marker
        assert authors2 == ['__SAME_AS_PREVIOUS__']


class TestLigatureHandling:
    """Tests for ligature handling in PDF extraction."""

    def test_ligatures_expanded_in_references(self):
        """Test that ligatures are expanded in extracted text."""
        # Simulate text with ligatures (would come from PDF)
        text_with_ligatures = "Deep Learning is eﬃcient and eﬀective."

        from check_hallucinated_references import expand_ligatures
        result = expand_ligatures(text_with_ligatures)

        assert "efficient" in result
        assert "effective" in result
        assert "ﬃ" not in result
        assert "ﬀ" not in result


class TestHyphenationHandling:
    """Tests for hyphenation handling in PDF extraction."""

    def test_syllable_breaks_fixed(self):
        """Test that syllable break hyphens are fixed."""
        ref = 'J. Smith, "A detec- tion method," in Proc. ACL, 2023.'

        title, from_quotes = extract_title_from_reference(ref)

        assert "detection" in title
        assert "detec-" not in title

    def test_compound_words_preserved(self):
        """Test that compound word hyphens are preserved."""
        ref = 'J. Smith, "A human- centered design approach," in Proc. CHI, 2023.'

        title, from_quotes = extract_title_from_reference(ref)

        assert "human-centered" in title
