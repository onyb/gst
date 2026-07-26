#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["sarvamai>=0.1.28"]
# ///
"""Extract GSTR-1 section rows from invoice PDFs using Sarvam AI.

Two-stage pipeline built on the Sarvam AI platform (https://docs.sarvam.ai):

  1. Document Digitization (Sarvam Vision) turns each PDF into markdown.
  2. A sarvam chat completion structures that markdown into rows matching
     the target section's spec (spec/gstr1/<section>.json), which is the
     same single source of truth the `gst` CLI validates against.

The result is a CSV that drops straight into the existing offline pipeline:

    uv run scripts/sarvam_extract.py invoice.pdf --section b2b -o b2b.csv
    gst validate b2b.csv --section b2b --gstin 27AAACR5055K1Z5 --period 072017

This script is a companion tool: the `gst` CLI itself stays fully offline
and never talks to any network. Requires SARVAM_API_KEY in the environment
(create one at https://dashboard.sarvam.ai).
"""

from __future__ import annotations

import argparse
import csv
import io
import json
import os
import re
import sys
import tempfile
import zipfile
from pathlib import Path

from _spec import load_spec, sorted_fields

DEFAULT_MODEL = "sarvam-105b"
DIGITIZE_TIMEOUT_SECS = 600

CONVENTIONS = """\
Value conventions (follow exactly):
- Dates: DD-MMM-YY, e.g. 14-Jul-17.
- Place Of Supply: 'NN-State Name' with the two-digit GST state code, e.g. 27-Maharashtra.
- Amounts: plain numbers only - no currency symbols, no thousands separators.
- One output row per distinct tax rate within an invoice. An invoice with line
  items at 18% and 5% becomes two rows; invoice-level values (number, date,
  total invoice value, recipient details) are repeated on every row.
- Rate is the combined GST percent for the line: IGST rate, or CGST + SGST
  summed (e.g. CGST 9% + SGST 9% -> 18).
- Taxable Value is the sum of taxable values (excluding tax) of all line items
  at that rate.
- Leave optional or unknown values as an empty string. Never invent data that
  is not present in the document.
- Write every value exactly as it would appear in the GSTN Excel template
  column, not as a derived code or factor. In particular:
  - Reverse Charge: the single letter Y or N.
  - Invoice Type: the exact template label - 'Regular B2B',
    'SEZ supplies with payment', 'SEZ supplies without payment', 'Deemed Exp',
    or 'Intra-State supplies attracting IGST'.
  - Applicable % of Tax Rate: blank, unless the invoice explicitly invokes the
    65% transitional provision, in which case '65'.
"""


def spec_columns(spec: dict) -> list[str]:
    return [f["column"] for f in sorted_fields(spec)]


def build_system_prompt(spec: dict) -> str:
    lines = [
        "You are a meticulous data-entry clerk preparing India GSTR-1 return data.",
        "You will be given the text of one or more tax invoices extracted from PDFs.",
        f"Produce rows for the GSTR-1 '{spec['section']}' section: {spec['title']}.",
        "",
        "Columns (name | type | required | meaning):",
    ]
    for f in sorted_fields(spec):
        req = "required" if f.get("required") else "optional"
        desc = f.get("description", "")
        lines.append(f"- {f['column']} | {f['type']} | {req} | {desc}")
    lines += [
        "",
        CONVENTIONS,
        "Respond with ONLY a JSON array of objects. Each object's keys must be",
        "exactly the column names listed above, and every value must be a string.",
        "No markdown fences, no commentary, no trailing text.",
    ]
    return "\n".join(lines)


def digitize(client, pdf: Path, language: str) -> str:
    """Run one PDF through Sarvam Document Digitization; return markdown."""
    job = client.document_intelligence.create_job(language=language, output_format="md")
    print(f"  docs-ai job {job.job_id}: uploading {pdf.name}", file=sys.stderr)
    job.upload_file(str(pdf))
    job.start()
    status = job.wait_until_complete(timeout=DIGITIZE_TIMEOUT_SECS)
    state = getattr(status, "job_state", None)
    if state not in ("Completed", "PartiallyCompleted"):
        sys.exit(f"error: digitization job for {pdf.name} ended in state {state}")

    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "output.bin"
        job.download_output(str(out))
        data = out.read_bytes()

    if zipfile.is_zipfile(io.BytesIO(data)):
        parts = []
        with zipfile.ZipFile(io.BytesIO(data)) as zf:
            for name in sorted(zf.namelist()):
                if name.endswith((".md", ".html", ".txt")):
                    parts.append(zf.read(name).decode("utf-8", errors="replace"))
        if not parts:
            sys.exit(f"error: digitization output for {pdf.name} had no text files")
        return "\n\n".join(parts)
    return data.decode("utf-8", errors="replace")


def parse_rows(raw: str, columns: list[str]) -> list[dict]:
    """Parse the model reply into row dicts, tolerating fences and chatter."""
    text = raw.strip()
    text = re.sub(r"^```(?:json)?\s*|\s*```$", "", text, flags=re.MULTILINE).strip()
    start, end = text.find("["), text.rfind("]")
    if start == -1 or end <= start:
        sys.exit(f"error: model reply contained no JSON array:\n{raw[:500]}")
    rows = json.loads(text[start : end + 1])
    if not isinstance(rows, list):
        sys.exit("error: model reply was not a JSON array")
    return [{col: str(row.get(col, "") or "") for col in columns} for row in rows]


def structure(client, model: str, system_prompt: str, markdown: str, columns: list[str]) -> list[dict]:
    response = client.chat.completions(
        model=model,
        temperature=0.1,
        max_tokens=4096,  # starter-tier ceiling; the 2048 default truncates mid-reasoning
        reasoning_effort="low",  # keep the thinking phase from eating the token budget
        messages=[
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": f"Invoice document text:\n\n{markdown}"},
        ],
    )
    choice = response.choices[0]
    if choice.message.content is None:
        sys.exit(
            f"error: {model} returned no answer (finish_reason={choice.finish_reason}); "
            "try again or raise max_tokens in structure()"
        )
    return parse_rows(choice.message.content, columns)


def write_csv(path: Path, columns: list[str], rows: list[dict]) -> None:
    with path.open("w", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=columns)
        writer.writeheader()
        writer.writerows(rows)


def main() -> None:
    ap = argparse.ArgumentParser(
        description="Extract a GSTR-1 section CSV from invoice PDFs via Sarvam AI."
    )
    ap.add_argument("pdfs", nargs="+", type=Path, help="invoice PDF(s), max 10 pages each")
    ap.add_argument("--section", default="b2b", help="GSTR-1 section spec to target (default: b2b)")
    ap.add_argument("-o", "--output", type=Path, help="output CSV path (default: <section>.csv)")
    ap.add_argument("--language", default="en-IN", help="document language, BCP-47 (default: en-IN)")
    ap.add_argument("--model", default=DEFAULT_MODEL, help=f"chat model (default: {DEFAULT_MODEL})")
    ap.add_argument("--keep-markdown", action="store_true", help="save digitized markdown next to the CSV")
    ap.add_argument("--dry-run", action="store_true", help="print the generated prompt and exit (no API calls)")
    args = ap.parse_args()

    spec = load_spec(args.section)
    columns = spec_columns(spec)
    system_prompt = build_system_prompt(spec)

    if args.dry_run:
        print(system_prompt)
        return

    for pdf in args.pdfs:
        if not pdf.exists():
            sys.exit(f"error: no such file: {pdf}")

    api_key = os.environ.get("SARVAM_API_KEY")
    if not api_key:
        sys.exit("error: SARVAM_API_KEY is not set (get one at https://dashboard.sarvam.ai)")

    from sarvamai import SarvamAI

    client = SarvamAI(api_subscription_key=api_key)
    output = args.output or Path(f"{args.section}.csv")

    all_rows: list[dict] = []
    for pdf in args.pdfs:
        print(f"[1/2] digitizing {pdf.name} (Sarvam Docs AI)", file=sys.stderr)
        markdown = digitize(client, pdf, args.language)
        if args.keep_markdown:
            md_path = output.with_name(f"{output.stem}-{pdf.stem}.md")
            md_path.write_text(markdown)
            print(f"  markdown saved to {md_path}", file=sys.stderr)
        print(f"[2/2] structuring rows with {args.model}", file=sys.stderr)
        rows = structure(client, args.model, system_prompt, markdown, columns)
        print(f"  {pdf.name}: {len(rows)} row(s)", file=sys.stderr)
        all_rows.extend(rows)

    write_csv(output, columns, all_rows)
    print(f"wrote {len(all_rows)} row(s) to {output}", file=sys.stderr)
    print(f"next: gst validate {output} --section {args.section} --gstin <GSTIN> --period <MMYYYY>", file=sys.stderr)


if __name__ == "__main__":
    main()
