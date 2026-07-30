#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["openpyxl>=3.1", "requests>=2.31"]
# ///
"""Capture the official tool's summary sidecar (`_meta.json`) for a workbook.

Seeds the running tool's working store with `Tool.seed` (the same
`/addtblfile` + `/addmltpldata` steps the golden captures in
fixtures/golden/README.md document), then POSTs `/fetchMeta`, which recomputes
the summary from the working file and writes the `..._meta.json` sidecar
beside it. The sidecar is copied to --out verbatim; the HTTP response carries
the same payload inside the tool's success envelope and is used only to
confirm the recompute ran.

No turnover is supplied, so no gt/cur_gt keys land in the meta.

    ./scripts/capture_summary_meta.py --app-dir ~/Downloads/gst/gst-offline-tool-unix/app \\
        --out fixtures/golden/gstr1-062025-meta.json
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import requests

from validation_differential import REPO, Tool


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--app-dir", required=True, type=Path,
                    help="the running offline tool's app directory")
    ap.add_argument("--workbook", type=Path,
                    default=REPO / "fixtures/gstr1/demo-workbook.xlsx")
    ap.add_argument("--gstin", default="27AAPFU0939F1ZV")
    ap.add_argument("--period", default="062025")
    ap.add_argument("--fy", default="2025-26")
    ap.add_argument("--month", default="June")
    ap.add_argument("--port", type=int, default=3010)
    ap.add_argument("--quarterly", action="store_true",
                    help="capture as a quarterly (TPQ) filer — an IFF in months 1-2")
    ap.add_argument("--out", required=True, type=Path,
                    help="where to write the captured meta, verbatim")
    args = ap.parse_args()

    try:
        requests.get(f"http://localhost:{args.port}/health", timeout=5)
    except requests.RequestException:
        sys.exit(f"no offline tool on port {args.port} — start it with `node app.js`")

    tool = Tool(args.app_dir, args.gstin, args.period, args.fy, args.month, args.port,
                is_tpq=args.quarterly)
    try:
        resp = tool.seed(args.workbook)
    except (requests.RequestException, ValueError):
        sys.exit("import hung or returned nothing")
    rejected = {k: v for k, v in resp.items() if k != "cache_key" and v}
    if rejected:
        sys.exit(f"the tool rejected rows — the capture would be partial: {rejected}")
    if not tool.work_file.exists():
        sys.exit("no working file was written — the seeding failed")

    fetched = requests.post(
        f"http://localhost:{args.port}/fetchMeta",
        json={
            "fName": str(tool.work_file.relative_to(args.app_dir / "public").with_suffix("")),
            "form": "GSTR1",
            "isTPQ": args.quarterly,
        },
        timeout=60,
    ).json()
    counts = (fetched.get("data") or fetched).get("counts")
    if not counts:
        sys.exit(f"/fetchMeta returned no counts: {json.dumps(fetched)[:200]}")

    meta_file = tool.work_file.with_name(tool.work_file.stem + "_meta.json")
    args.out.write_bytes(meta_file.read_bytes())
    print(f"{args.out}: {len(counts)} summary row(s) captured")
    return 0


if __name__ == "__main__":
    sys.exit(main())
