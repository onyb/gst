"""Shared access to the GSTR-1 row specs in spec/gstr1/.

The spec directory is resolved relative to this file, so the scripts that
import this module work whether they are run from the repo root or from
scripts/.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SPEC_DIR = Path(__file__).resolve().parent.parent / "spec" / "gstr1"


def load_specs() -> dict[str, dict]:
    """The row specs — every spec/gstr1/*.json with a "fields" key (which
    excludes upload-envelope.json) — keyed by section code."""
    specs: dict[str, dict] = {}
    for path in sorted(SPEC_DIR.glob("*.json")):
        spec = json.loads(path.read_text())
        if "section" in spec and "fields" in spec:
            specs[spec["section"]] = spec
    return specs


def load_spec(section: str) -> dict:
    """One row spec by section code; exits with the known codes otherwise."""
    specs = load_specs()
    if section not in specs:
        sys.exit(f"error: unknown section '{section}' (have: {', '.join(sorted(specs))})")
    return specs[section]


def sorted_fields(spec: dict) -> list[dict]:
    return sorted(spec["fields"], key=lambda f: f["order"])
