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

## Install

```sh
brew install onyb/tap/gst
```

Or build from source: `cargo install --locked --path crates/gst-cli`.

## Status

Work in progress. The MVP targets GSTR-1 and GSTR-3B.

| Feature | Status |
|---|---|
| GSTR-1: B2B invoices (B2B, B2BA) | Supported |
| GSTR-1: B2C invoices (B2CL, B2CLA, B2CS, B2CSA) | Supported |
| GSTR-1: credit/debit notes (CDNR, CDNRA, CDNUR, CDNURA) | Supported |
| GSTR-1: exports (EXP, EXPA) | Pending |
| GSTR-1: advances (AT, ATA, TXPD) | Pending |
| GSTR-1: nil-rated, HSN summary, document series | Pending |
| GSTR-3B | Pending |
| `gst validate`, `gst generate` | Supported |
| `gst summary`, `gst errors`, `gst diff` | Pending |

## Provenance and licensing

This project contains no code from GSTN's tools. The spec documents publicly
distributed interfaces (Excel/CSV templates, portal JSON schemas, error-file
formats, validation behavior); see [`spec/README.md`](spec/README.md) for the
exact artifact versions it was derived from.

Licensed under the [Mozilla Public License 2.0](LICENSE).

## Landing page

The landing page lives in [`web/`](web/) (Vite + React + TypeScript).
