# GST return format specification

Machine-readable specification of the file formats and validation rules used
to prepare Indian GST returns offline: import templates (Excel/CSV), portal
upload JSON payloads, portal error files, and the masters (state codes, tax
rates, document types) they reference.

The implementation in this repository codes **only** against these spec
files — never against GSTN's tools.

## Provenance

Derived from publicly distributed GSTN artifacts — the formats themselves
(templates, JSON schemas, codes, validation behavior) are interfaces and
facts, documented here independently:

| Source artifact | Version | Covers |
|---|---|---|
| Returns Offline Tool (gst.gov.in download) | V3.2.4 | GSTR-1 templates, upload JSON, error files |
| GSTR-1 Excel Workbook Template | V2.2 | GSTR-1 sheet/column layout |
| Section-wise CSV files | 2025-01 | GSTR-1 per-section CSV headers |
| GSTR-3B Excel Utility | V5.8 | GSTR-3B form layout, upload JSON |

Contains no code from those artifacts.

One data file is reproduced verbatim: GSTN's own section-wise sample CSV for
GSTR-1 B2B, kept at `fixtures/gstr1/b2b-gstn-sample.csv` as a real-world test
input. It is sample data, not implementation. Note that it no longer passes
the official tool's current validation — see the corresponding `quirk` note in
`gstr1/b2b.json`.

## Layout

- `masters/` — shared fact tables: state codes, tax rate slabs, invoice
  types, credit/debit note reasons
- `gstr1/` — per-section field definitions, validation rules, import
  mappings, payload shape (B2B, B2CL, B2CS, CDNR, EXP, AT, HSN, DOCS, …)
- `gstr3b/` — form table definitions (3.1, 3.1.1, 3.2, 4, 5, 5.1),
  cross-table rules, payload shape

Spec files are versioned against the official artifact versions above; when
GSTN ships a new tool version, diffs land here first.
