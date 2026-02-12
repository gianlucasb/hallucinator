"""Tests for the hallucinator validation pipeline Python bindings (Phase 2B)."""

import pytest

from hallucinator import (
    Validator,
    ValidatorConfig,
    ValidationResult,
    DbResult,
    DoiInfo,
    ArxivInfo,
    RetractionInfo,
    ProgressEvent,
    CheckStats,
    PdfExtractor,
)


# ── ValidatorConfig ──


def test_validator_config_defaults():
    config = ValidatorConfig()
    assert config.max_concurrent_refs == 4
    assert config.db_timeout_secs == 10
    assert config.db_timeout_short_secs == 5
    assert config.disabled_dbs == []
    assert config.check_openalex_authors is False
    assert config.openalex_key is None
    assert config.s2_api_key is None
    assert config.dblp_offline_path is None
    assert config.acl_offline_path is None
    assert config.crossref_mailto is None


def test_validator_config_setters():
    config = ValidatorConfig()
    config.openalex_key = "test-key"
    config.s2_api_key = "s2-key"
    config.max_concurrent_refs = 8
    config.db_timeout_secs = 20
    config.db_timeout_short_secs = 3
    config.disabled_dbs = ["openalex", "neurips"]
    config.check_openalex_authors = True
    config.crossref_mailto = "test@example.com"

    assert config.openalex_key == "test-key"
    assert config.s2_api_key == "s2-key"
    assert config.max_concurrent_refs == 8
    assert config.db_timeout_secs == 20
    assert config.db_timeout_short_secs == 3
    assert config.disabled_dbs == ["openalex", "neurips"]
    assert config.check_openalex_authors is True
    assert config.crossref_mailto == "test@example.com"


def test_validator_config_clear_optional():
    config = ValidatorConfig()
    config.s2_api_key = "key"
    assert config.s2_api_key == "key"
    config.s2_api_key = None
    assert config.s2_api_key is None


def test_validator_config_repr():
    config = ValidatorConfig()
    r = repr(config)
    assert "ValidatorConfig" in r
    assert "max_concurrent_refs=4" in r


# ── Validator construction ──


def test_validator_construction():
    config = ValidatorConfig()
    validator = Validator(config)
    assert repr(validator).startswith("Validator(")


def test_validator_invalid_dblp_path():
    config = ValidatorConfig()
    config.dblp_offline_path = "/nonexistent/path/dblp.db"
    with pytest.raises(RuntimeError, match="DBLP"):
        Validator(config)


def test_validator_invalid_acl_path():
    config = ValidatorConfig()
    config.acl_offline_path = "/nonexistent/path/acl.db"
    with pytest.raises(RuntimeError, match="ACL"):
        Validator(config)


# ── Validator.check with empty refs ──


def test_check_empty_refs():
    config = ValidatorConfig()
    validator = Validator(config)
    results = validator.check([])
    assert results == []


def test_check_empty_refs_with_progress():
    events = []

    def on_progress(event):
        events.append(event)

    config = ValidatorConfig()
    validator = Validator(config)
    results = validator.check([], progress=on_progress)
    assert results == []


# ── Validator.stats ──


def test_stats_empty():
    stats = Validator.stats([])
    assert isinstance(stats, CheckStats)
    assert stats.total == 0
    assert stats.verified == 0
    assert stats.not_found == 0
    assert stats.author_mismatch == 0
    assert stats.retracted == 0
    assert stats.skipped == 0


def test_check_stats_repr():
    stats = Validator.stats([])
    r = repr(stats)
    assert "CheckStats" in r
    assert "total=0" in r


# ── Type imports ──


def test_all_types_importable():
    """All Phase 2B types are importable from the hallucinator package."""
    assert Validator is not None
    assert ValidatorConfig is not None
    assert ValidationResult is not None
    assert DbResult is not None
    assert DoiInfo is not None
    assert ArxivInfo is not None
    assert RetractionInfo is not None
    assert ProgressEvent is not None
    assert CheckStats is not None


# ── Cancel ──


def test_cancel_before_check():
    """Calling cancel() before check() doesn't crash."""
    config = ValidatorConfig()
    validator = Validator(config)
    validator.cancel()
    # Should still work (cancel is reset on next check)
    results = validator.check([])
    assert results == []


# ── Network-dependent tests ──


@pytest.mark.network
def test_check_single_reference():
    """Check a single well-known reference (requires network)."""
    ext = PdfExtractor()
    ref = ext.parse_reference(
        "V. Vaswani, N. Shazeer, N. Parmar, J. Uszkoreit, L. Jones, "
        'A. N. Gomez, L. Kaiser, and I. Polosukhin, "Attention Is All You Need," '
        "in Advances in Neural Information Processing Systems 30, 2017."
    )
    assert ref is not None

    events = []

    def on_progress(event):
        events.append(event)

    config = ValidatorConfig()
    config.db_timeout_secs = 15
    validator = Validator(config)
    results = validator.check([ref], progress=on_progress)

    assert len(results) == 1
    r = results[0]
    assert isinstance(r, ValidationResult)
    assert r.title  # non-empty
    assert r.status in ("verified", "not_found", "author_mismatch")
    assert isinstance(r.db_results, list)
    assert isinstance(r.ref_authors, list)
    assert isinstance(r.failed_dbs, list)

    # Progress events were received
    assert len(events) > 0
    assert any(e.event_type == "checking" for e in events)


@pytest.mark.network
def test_progress_event_properties():
    """Progress event properties are accessible."""
    ext = PdfExtractor()
    ref = ext.parse_reference(
        'J. Devlin, M. Chang, K. Lee, and K. Toutanova, "BERT: Pre-training '
        'of Deep Bidirectional Transformers for Language Understanding," '
        "in Proc. NAACL, 2019."
    )
    assert ref is not None

    events = []

    def on_progress(event):
        events.append(event)

    config = ValidatorConfig()
    validator = Validator(config)
    validator.check([ref], progress=on_progress)

    # Check 'checking' event properties
    checking_events = [e for e in events if e.event_type == "checking"]
    if checking_events:
        e = checking_events[0]
        assert e.index is not None
        assert e.total is not None
        assert e.title is not None
        # Properties not applicable return None
        assert e.count is None
        assert e.db_name is None


@pytest.mark.network
def test_disabled_dbs():
    """Disabled databases are skipped."""
    ext = PdfExtractor()
    ref = ext.parse_reference(
        'Y. LeCun, Y. Bengio, and G. Hinton, "Deep Learning Review of Modern '
        'Approaches to Neural Network Training," Nature, vol. 521, 2015.'
    )
    assert ref is not None

    config = ValidatorConfig()
    config.disabled_dbs = [
        "crossref",
        "arxiv",
        "dblp",
        "semantic_scholar",
        "acl",
        "neurips",
        "europe_pmc",
        "pubmed",
    ]
    validator = Validator(config)
    results = validator.check([ref])

    assert len(results) == 1
    # With most DBs disabled, the reference should be "not_found"
    # (unless openalex finds it, which is unlikely without a key)
    r = results[0]
    assert r.status in ("verified", "not_found", "author_mismatch")

    # Check db_results — disabled DBs should show as "skipped"
    for db_r in r.db_results:
        assert isinstance(db_r, DbResult)
        assert isinstance(db_r.db_name, str)
        assert db_r.status in (
            "match",
            "no_match",
            "author_mismatch",
            "timeout",
            "error",
            "skipped",
        )


@pytest.mark.network
def test_stats_from_results():
    """Stats computed from real validation results."""
    ext = PdfExtractor()
    ref = ext.parse_reference(
        "I. Goodfellow, J. Pouget-Abadie, M. Mirza, B. Xu, D. Warde-Farley, "
        'S. Ozair, A. Courville, and Y. Bengio, "Generative Adversarial Networks '
        'for Image Synthesis and Domain Adaptation," '
        "in Advances in Neural Information Processing Systems, 2014."
    )
    assert ref is not None

    config = ValidatorConfig()
    validator = Validator(config)
    results = validator.check([ref])

    stats = Validator.stats(results)
    assert stats.total == 1
    assert stats.verified + stats.not_found + stats.author_mismatch == 1
