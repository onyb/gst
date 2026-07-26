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

## Validation, checked against the tool

Every rule here is a claim about what GSTN's tool rejects, and those claims were
wrong often enough to be worth testing rather than trusting.
`scripts/validation_differential.py` does that by construction: for each
declared constraint on each field it builds a row violating it, feeds that row
to both validators, and compares the verdicts. The tool's verdict is read from
the working file it writes — with one data row in one sheet, an empty section
means the row was rejected — which is more reliable than its error lists, since
those mix row numbers with invoice numbers and leave some rejections unreported.

Latest run: **384 cases, 347 agree, 0 misses.** A miss would be the tool
rejecting a row this implementation called clean, which is the direction that
matters — it would mean handing a filer a return the portal refuses. There are
none.

The 35 remaining disagreements are all this implementation being **stricter**,
and each was checked by looking at what the tool actually emits rather than
trusting its accept:

| What the tool accepts | What it then emits | Why we reject |
|---|---|---|
| A blank taxable value | `txval: 0` with `iamt: null` | A line with no taxable value is a filer error, and the reference builds a broken item out of it |
| An unknown invoice type or note supply type | The `inv_typ` key vanishes from the record | A record missing a mandatory field is worse than a rejected row |
| A non-numeric `Applicable % of Tax Rate` | Nothing — it becomes `NaN`, fails no numeric guard, and is dropped | `50` is properly rejected but `ZZBOGUS` is not; silently discarding the filer's intent is worse than saying so |
| An unknown UQC | An unmapped code the portal will not recognise | Same reasoning |
| A negative cess on an export | A negative `csamt` | Exports never validate the cess column at all |
| An over-long or illegally-punctuated HSN description | The value unchanged | The reference builds the pattern and then never applies it — a missing-braces bug |

Two cases are worse than a disagreement: **a blank Note Supply Type crashes the
tool outright.** Its label-to-code helper calls `.trim()` on a cell it never
checks exists, the resulting TypeError escapes the request handler, and the
import hangs with no response. A filer sees the import spin forever. Recorded
in `gstr1/cdnr.json` and `gstr1/cdnra.json`.

The differential also found a real bug on this side, since fixed: 53 amount
fields rejected a third decimal place that the reference quietly rounds to two
before checking — and the rounded value is what reaches the payload, so this
affected output, not just validation. Those fields now carry `round_to`.

Running it needs the official tool, so it is not part of `cargo test`:

```sh
cargo build -p gst-cli
uv run scripts/validation_differential.py --app-dir <tool>/app
```

## Layout

- `masters/` — shared fact tables: state codes, tax rate slabs, invoice
  types, credit/debit note reasons
- `gstr1/` — per-section field definitions, validation rules, import
  mappings, payload shape (B2B, B2CL, B2CS, CDNR, EXP, AT, HSN, DOCS, …)
- `gstr3b/` — form table definitions (3.1, 3.1.1, 3.2, 4, 5, 5.1),
  cross-table rules, payload shape

Spec files are versioned against the official artifact versions above; when
GSTN ships a new tool version, diffs land here first.
