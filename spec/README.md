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

Latest run: **577 cases at 06-2025 and 561 at 04-2025, 0 misses in either.** A miss would be the tool
rejecting a row this implementation called clean, which is the direction that
matters — it would mean handing a filer a return the portal refuses. There are
none.

The remaining disagreements — 39 and 36 respectively — are all this
implementation being **stricter**, and each was checked by looking at what the
tool actually emits rather than trusting its accept:

| What the tool accepts | What it then emits | Why we reject |
|---|---|---|
| A blank taxable value | `txval: 0` with `iamt: null` | A line with no taxable value is a filer error, and the reference builds a broken item out of it |
| An unknown invoice type or note supply type | The `inv_typ` key vanishes from the record | A record missing a mandatory field is worse than a rejected row |
| A non-numeric `Applicable % of Tax Rate` | Nothing — it becomes `NaN`, fails no numeric guard, and is dropped | `50` is properly rejected but `ZZBOGUS` is not; silently discarding the filer's intent is worse than saying so |
| An unknown UQC | An unmapped code the portal will not recognise | Same reasoning |
| A negative cess on an export | A negative `csamt` | Exports never validate the cess column at all |
| An over-long or illegally-punctuated HSN description | The value unchanged | The reference builds the pattern and then never applies it — a missing-braces bug, and from 05-2021 the cell is overwritten from the code table before anything reads it |

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

It is run at two periods, because the HSN summary is a different section either
side of the 05-2025 bifurcation and one run can only exercise one of them:

```sh
# from 05-2025: the hsn(b2b)/hsn(b2c) pair
uv run scripts/validation_differential.py --app-dir <tool>

# before it: the single combined hsn sheet, which needs a workbook carrying it
uv run scripts/validation_differential.py --app-dir <tool> \
    --workbook <pre-2025 workbook> --period 042025 --fy 2024-25 --month April
```

The harness skips sections outside their period window, so each run probes only
what that period actually files.

## Read against the reference's source

The differential harness and the captured golden files between them cover a
great deal, but both are driven by inputs someone chose. A separate pass read
GSTN's JavaScript directly — `service/offline.js`, `utility/returnStructure.js`,
`utility/common.js` and the browser controllers — looking for behaviour no
probe had happened to reach. What it found sits almost entirely in the gaps
between the two: cases the golden workbooks do not exercise and the differential
does not construct.

| What was wrong | Where the reference says otherwise |
|---|---|
| The HSN summary was dropped from every return before 05-2025 | A single `hsn` sheet feeds `hsn.data` until the bifurcation (`returnStructure.js:1468`); only `hsn(b2b)`/`hsn(b2c)` were specified, so the envelope's pre-bifurcation branch drew from a section code nothing registered |
| Table 15's `Document type` was ignored | `getInvType` maps it to R/DE/SEWP/SEWOP, `inv_typ` is emitted, the intra-state branch admits only R and DE, and SEWOP zeroes tax and cess (`:245`, `:1691`, `:3362`) |
| Money rounded on exact decimals | The reference is `parseFloat(x.toFixed(2))` over a binary double, which lands on the other side of a decimal midpoint about half the time |
| Service codes kept a unit and a quantity | Any code starting `99` has its `UQC` forced to `NA` and its `Total Quantity` to `0`, before validation, in all three HSN tables (`offline2.js:390`, `:399`) |
| The combined HSN table used the filer's own description | From 05-2021 the reference overwrites that cell with the official description from the code table (`offline2.js:388`), and there is no `user_desc` to keep the filer's in |
| Six flat sections emitted a record per row | The reference collapses rows on a per-section key in `addmltpldata` — not in its row mapping, which is why the behaviour is invisible from the payload builder |
| Rate-zero HSN lines were rejected | The `isITAmt`/`isCTAmt`/`isSTUTAmt` flags never reach a decision: `validatePattern`'s mandatory branch is `isMandatory ? true : true`, and `Math.abs(cnvt2Nm(blank))` is `0`, which clears the emptiness guard |
| Shipping bill dates were bounded by the return period | `validateDate(..., true)` — the third argument is `allowFuture`, and only the 1 July 2017 floor applies (`:8065`, `:9125`) |
| Tables 14/14A had no cross-field rules | `validateAllNegorAllPosPattern` (`:5429`) enforces sign agreement across all five amounts and `cgst == sgst` |
| E-commerce registration numbers skipped the check digit | Every one goes through the full `validateGSTIN` (`:406`) |
| A quarterly filer's first two months produced a full return | An IFF carries four tables and the header keys; the rest is deleted (`offline.js:5464`) |

Two things are worth recording about the method, because they cost time.

**A captured file only pins the input that produced it.** The ECO golden file
shows no `inv_typ`, which reads like proof that the section has no such key. It
is not: the workbook behind it says `Document type = Invoice`, which is not one
of the four labels the reference recognises, so its lookup returns `undefined`
and the key drops out. The capture pins the *degenerate* case. The template
offers no dropdown for that column, so unrecognised labels are the common case
rather than the exceptional one — which is why they are reproduced, and warned
about, rather than rejected.

**Reading the source is not enough on its own.** Two of the fixes above were
wrong when written and were caught by re-running the differential, both for the
same reason: the row mapping reads a cell that something else has already
rewritten. `case 'hsn'` maps `"desc": inv['Description']`, which is true and
tells you nothing — from 05-2021 `offline2.js:388` has already replaced that
cell with the official description from the code table, so the filer's own text
never reaches the payload and its "required" flag can never fire. The same pass
forces `UQC` to `NA` and `Total Quantity` to `0` for any code starting `99`, in
all three HSN tables — which the bifurcated pair had been missing since it was
written. A payload builder is only the last step; what a cell contains by the
time it runs is a separate question.

**A test can assert the bug.** The test covering the HSN bifurcation planted a
`"hsn"` section key directly into the map by hand. No import could produce that
key, so `take("hsn")` was always empty and every pre-2025 return shipped without
its HSN summary while the test stayed green. Tests for envelope behaviour now
drive the real pipeline, and one of them checks that every section the envelope
names is actually registered.

## The chunk splitter is specified against, not reproduced

The reference's splitter (`utility/common.js` `exports.chunk`) is the one place
this implementation deliberately does not reproduce observable behaviour,
because that behaviour defeats the feature's own purpose: every chunk after the
first loses the `gstin`/`fp`/`version`/`hash` header and cannot be uploaded,
oversized sections outside five hardcoded cases are silently dropped, one path
writes byte-identical duplicate chunks, and filenames carry `Math.random()`
suffixes — so its output is neither usable nor capturable as a golden. The
trigger and the size measure ARE faithful (`chunking` in
`upload-envelope.json`: the 4.7 MiB float threshold and the double-stringified
measure). The defect list and the divergence declaration live in that block
(`chunking.reference_defects`, `chunking.divergence`); the split implemented
here is correct by design — full header in every part, envelope-granularity
splitting in envelope key order, union of parts equal to the unsplit file,
deterministic `part{n}of{m}` names.

## Layout

- `masters/` — shared fact tables: state codes, tax rate slabs, invoice
  types, credit/debit note reasons
- `gstr1/` — per-section field definitions, validation rules, import
  mappings, payload shape (B2B, B2CL, B2CS, CDNR, EXP, AT, HSN, DOCS, …).
  Two standalone files describe whole-return behaviour rather than one
  section: `upload-envelope.json` (the wrapper the portal expects around the
  section payloads) and `summary.json` (the pre-upload View Summary table —
  per-section counts and the four tax-head totals, with the official row
  order, verbatim labels, credit-note sign flip and without-payment zeroing
  rules)
- `gstr3b/` — the whole-form return, from the V5.8 Excel VBA utility:
  `form.json` (cell map for tables 3.1/3.1.1/4/5/5.1/3.2, validation rules,
  period gates, payload emission order, recorded divergences — the utility's
  broken negatives gate, its byte-level output noise we deliberately do not
  reproduce) and `pos.json` (the verbatim 3.2 place-of-supply dropdown, code
  28 absent and misspellings preserved, plus the FY list). Like the GSTR-1
  whole-return documents, these carry no meta-schema

Spec files are versioned against the official artifact versions above; when
GSTN ships a new tool version, diffs land here first.
