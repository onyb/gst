# gst

An open-source, cross-platform CLI for preparing Indian GST returns offline:
validate your Excel/CSV workbook, fix errors in the spreadsheet, and generate
portal-ready upload JSON — the job of GSTN's Windows-only offline utilities,
as a single `gst` command that runs anywhere.

**Status: work in progress.** MVP targets GSTR-1 and GSTR-3B.

## How it works

GST returns are pure data — no invoice documents are ever uploaded. The loop:

1. Fill the Excel workbook (invoice rows for GSTR-1, form cells for GSTR-3B)
2. `gst validate workbook.xlsx` — errors with sheet/row/column references;
   fix them in Excel
3. `gst generate workbook.xlsx` — portal-ready upload JSON (chunked under
   the portal's 5 MB cap for large GSTR-1 filings)
4. Upload on gst.gov.in (Returns Dashboard → Prepare Offline). If the portal
   rejects rows, `gst errors` maps its error file back to your sheet/rows
5. Review and file on the portal as usual (DSC/EVC)

Everything runs locally. The tool makes no network calls.

## Why

The official offline utilities are Windows-only (a legacy Node/AngularJS app
and an Excel VBA macro workbook), closed-source freeware. This project is an
independent, open-source implementation of the same publicly distributed
file formats that works on macOS, Linux, and Windows, and is scriptable.

## Provenance & licensing

This project contains **no code from GSTN's tools**. It is written from
scratch against the publicly distributed interfaces: Excel/CSV templates,
portal JSON schemas, error-file formats, and validation rules — documented
as a machine-readable spec in [`spec/`](spec/). Output equivalence is
verified against the official tools' output (see spec README for versions).

Licensed under the [Mozilla Public License 2.0](LICENSE).
