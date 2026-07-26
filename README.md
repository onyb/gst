# gst

[![CI](https://github.com/onyb/gst/actions/workflows/ci.yml/badge.svg)](https://github.com/onyb/gst/actions/workflows/ci.yml)

An open-source, cross-platform CLI for preparing Indian GST returns offline:
validate your Excel/CSV workbook, fix errors in the spreadsheet, and generate
portal-ready upload JSON.

## The interesting part

The official way to do this is GSTN's Offline Tool, a closed-source
Windows-only `.exe`. We reverse-engineered that binary and distilled
everything it knows (templates, validation rules, payload shapes, error
formats) into a machine-readable spec in [`spec/`](spec/). The Rust core and
CLI in this repo are then written purely against that spec, never against the
tool itself, and their output is verified against the official tool's.

So the spec is the product as much as the CLI: an independent, testable
description of the GST return formats that anyone can build on.

## How it works

1. Fill the Excel workbook as usual
2. `gst validate workbook.xlsx` reports errors with sheet/row/column
   references; fix them in Excel
3. `gst generate workbook.xlsx` writes portal-ready upload JSON, chunked
   under the portal's 5 MB cap
4. Upload on gst.gov.in (Returns Dashboard, Prepare Offline), then review
   and file as usual

Everything runs locally. The tool makes no network calls.

## Extract from invoice PDFs (Sarvam AI)

An optional companion script skips step 1: point it at invoice PDFs and it
produces the section CSV for you, using [Sarvam AI](https://docs.sarvam.ai) —
Document Digitization (Sarvam Vision) to read the PDF, then a `sarvam-105b`
chat completion to structure rows against the same section specs the
validator uses.

```sh
export SARVAM_API_KEY=...   # from dashboard.sarvam.ai
uv run scripts/sarvam_extract.py fixtures/invoices/inv-007.pdf --section b2b -o b2b.csv
gst validate b2b.csv --section b2b --gstin 27AAPFU0939F1ZV --period 072017
```

This is strictly additive: the `gst` CLI itself is unchanged and still makes
no network calls. Extracted rows go through the exact same validate/generate
pipeline, so anything the model mis-reads is caught before it reaches the
portal. A sample invoice PDF lives at `fixtures/invoices/inv-007.pdf`.

## Install

```sh
brew install onyb/tap/gst
```

Or build from source: `cargo install --locked --path crates/gst-cli`.

## Status

Work in progress. The MVP targets GSTR-1 and GSTR-3B. 13 of the 30 GSTR-1
worksheets are implemented, including both tables every filer must submit
(HSN summary and documents issued).

Output is structurally complete but **not yet verified against GSTN's own
tool** — there is no differential test against a captured reference file, so
treat generated JSON as ready to inspect rather than proven to upload.

| GSTR-1 section | Sheets | Status |
|---|---|---|
| B2B invoices (B2B, B2BA) | 2 | ✅ |
| B2C invoices (B2CL, B2CLA, B2CS, B2CSA) | 4 | ✅ |
| Credit/debit notes (CDNR, CDNRA, CDNUR, CDNURA) | 4 | ✅ |
| HSN summary (HSN B2B, HSN B2C) | 2 | ✅ |
| Documents issued (DOCS) | 1 | ✅ |
| Exports (EXP, EXPA) | 2 | ⏳ |
| Advances (AT, ATA, ATADJ, ATADJA) | 4 | ⏳ |
| Nil-rated and exempt (EXEMP) | 1 | ⏳ |
| E-commerce (ECO and amendments) | 10 | ⏳ |
| GSTR-3B | — | ⏳ |

| Command | Status |
|---|---|
| `gst validate` — problems with sheet, row and column | ✅ |
| `gst upload` — complete portal file from one workbook | ✅ |
| `gst generate` — one section's payload on its own | ✅ |
| `gst summary` — section totals before uploading | ⏳ |
| `gst errors` — map the portal's error file back to rows | ⏳ |
| `gst diff` — semantic diff of two payloads | ⏳ |

## Provenance and licensing

This project contains no code from GSTN's tools. The spec documents publicly
distributed interfaces (Excel/CSV templates, portal JSON schemas, error-file
formats, validation behavior); see [`spec/README.md`](spec/README.md) for the
exact artifact versions it was derived from.

Licensed under the [Mozilla Public License 2.0](LICENSE).

## Landing page

The landing page lives in [`web/`](web/) (Vite + React + TypeScript).
