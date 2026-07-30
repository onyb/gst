#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["openpyxl>=3.1", "requests>=2.31"]
# ///
"""Capture the official tool's summary sidecar (`_meta.json`) for a workbook.

Seeds the running tool's working store over HTTP exactly as the upload-golden
capture does (steps 1-2 in fixtures/golden/README.md: `/addtblfile` then
`/addmltpldata`), then POSTs `/fetchMeta`, which recomputes the summary from
the working file and writes the `..._meta.json` sidecar beside it. The sidecar
is copied to --out verbatim; the HTTP response carries the same payload inside
the tool's success envelope and is used only to confirm the recompute ran.

No turnover is supplied, so no gt/cur_gt keys land in the meta.

    ./scripts/capture_summary_meta.py --app-dir ~/Downloads/gst/gst-offline-tool-unix/app \\
        --out fixtures/golden/gstr1-062025-meta.json
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

import requests

from validation_differential import REPO, share_data


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
    ap.add_argument("--out", required=True, type=Path,
                    help="where to write the captured meta, verbatim")
    args = ap.parse_args()

    base = f"http://localhost:{args.port}"
    try:
        requests.get(f"{base}/health", timeout=5)
    except requests.RequestException:
        sys.exit(f"no offline tool on port {args.port} — start it with `node app.js`")

    rel = f"userData/{args.gstin}/GSTR1/{args.fy}/{args.month}"
    work_dir = args.app_dir / "public" / rel
    stem = f"GSTR1_{args.gstin}_{args.fy}_{args.month}"
    meta_file = work_dir / f"{stem}_meta.json"

    # A clean slate, so the meta reflects only this workbook.
    shutil.rmtree(work_dir, ignore_errors=True)

    with args.workbook.open("rb") as fh:
        resp = requests.post(
            f"{base}/addtblfile",
            files={"file": (args.workbook.name, fh)},
            data={"shareData": json.dumps(share_data(args.gstin, args.period, args.fy))},
            timeout=60,
        ).json()
    rejected = {k: v for k, v in resp.items() if k != "cache_key" and v}
    if rejected:
        sys.exit(f"the tool rejected rows — the capture would be partial: {rejected}")

    requests.post(
        f"{base}/addmltpldata",
        json={
            "gstin": args.gstin, "form": "GSTR1", "fy": args.fy, "month": args.month,
            "fp": args.period, "gt": "", "cur_gt": "", "type": "",
            "tbl_data": {"cache_key": resp["cache_key"]},
        },
        timeout=60,
    )
    if not (work_dir / f"{stem}.json").exists():
        sys.exit("no working file was written — the seeding failed")

    fetched = requests.post(
        f"{base}/fetchMeta",
        json={"fName": f"{rel}/{stem}", "form": "GSTR1", "isTPQ": False},
        timeout=60,
    ).json()
    counts = (fetched.get("data") or fetched).get("counts")
    if not counts:
        sys.exit(f"/fetchMeta returned no counts: {json.dumps(fetched)[:200]}")

    args.out.write_bytes(meta_file.read_bytes())
    print(f"{args.out}: {len(counts)} summary row(s) captured")
    return 0


if __name__ == "__main__":
    sys.exit(main())
