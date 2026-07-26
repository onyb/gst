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

## Dates in the workbook must be text, not date cells

A trap that costs filers a whole import: the official tool accepts a date only
in `DD-MMM-YYYY`, `DD-MMM-YY`, `D-MMM-YYYY` or `D-MMM-YY` form (`14-Jul-2017`),
parsed strictly. It reads the workbook with `sheet_to_json`, which renders a
date-formatted cell through that cell's *number format* — so a cell holding a
real date formatted `yyyy-mm-dd` arrives as `"2017-07-14"` and is rejected, and
one formatted with a time component arrives as `"2017-07-14 0:00:00"` and is
rejected too. Every row fails, and the reported error is a generic pattern
mismatch that says nothing about the number format being at fault.

Type dates as text in `DD-MMM-YYYY` form, which is also what GSTN's own
section-wise sample CSVs use. `fixtures/gstr1/demo-workbook.xlsx` does this
deliberately, so it imports cleanly into the official tool as well as this one.

This implementation is more permissive on input — `gst validate` accepts several
text layouts and Excel serials — but it cannot rescue a workbook that the
official tool will reject, so the constraint is recorded here rather than
quietly worked around.

## Layout

- `masters/` — shared fact tables: state codes, tax rate slabs, invoice
  types, credit/debit note reasons
- `gstr1/` — per-section field definitions, validation rules, import
  mappings, payload shape (B2B, B2CL, B2CS, CDNR, EXP, AT, HSN, DOCS, …)
- `gstr3b/` — form table definitions (3.1, 3.1.1, 3.2, 4, 5, 5.1),
  cross-table rules, payload shape

Spec files are versioned against the official artifact versions above; when
GSTN ships a new tool version, diffs land here first.
