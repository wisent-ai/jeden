"""The checker itself: read the documents, hold the schema to the contract,
validate the golden envelopes against it, and read the SDK surfaces."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from .documents import _load_documents
from .schema_checks import _check_golden, _check_schema_contract
from .sdks import _check_sdks, _manifest


def check(root: Path) -> tuple[list[str], dict[str, Any]]:
    root = root.resolve()
    errors: list[str] = []
    documents = _load_documents(root / "protocol" / "schema" / "v1", errors)
    schemas = _check_schema_contract(documents, errors) if documents else {}
    if documents:
        _check_golden(documents, schemas, errors)
    groups = _check_sdks(root, errors)
    manifest = _manifest(root, documents, groups)
    return sorted(set(errors)), manifest

