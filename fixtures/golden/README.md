# Golden files captured from GSTN's offline tool

Reference output, captured by running the official Returns Offline Tool
(V3.2.4) and taking the file it wrote. These are the only artifacts in this
repository that come from the tool's own behaviour rather than from reading its
source, so they are what makes "matches the reference" a checkable claim
instead of an assertion.

| File | Captured from | Input |
|---|---|---|
| `gstr1-062025-reference.json` | `POST /generateFile`, tool V3.2.4 | `fixtures/gstr1/demo-workbook.xlsx`, GSTIN `27AAPFU0939F1ZV`, period `062025` |

## How it was captured

The tool keeps a working file per GSTIN/form/year/month under
`public/userData/<gstin>/<form>/<fy>/<month>/`, in the same envelope shape as
the upload file. Seeding that file and calling `POST /generateFile` produces
the upload artifact without driving the browser UI at all.

Two obstacles are worth recording, because they will recur:

1. `generateFile` creates its output directory with an async `mkdirp` whose
   callback is empty, then writes into it immediately — so the write loses the
   race and fails with ENOENT. Its error path then calls an undefined
   `callback`, crashing the request. Pre-creating the timestamped output
   directory avoids both.
2. The directory name embeds hours, minutes and seconds, so it cannot be
   predicted exactly; creating every second of the current and next minute is
   enough.

## What this file settled

Four corrections to specs that had been derived from source alone:

- Empty sections are **omitted** from the upload file, not emitted as `[]`.
- `diff_percent` is emitted **only** when it is `0.65`; every other value is
  discarded, across all ten sections that carry it.
- `cname` is **stripped** for b2b, b2ba, cdnr and cdnra — the recipient's name
  never reaches the portal.
- The filename's date segment is the **generation** date (day, month, year, no
  zero padding), not the return period.

It also confirmed `diffval` never reaches the upload JSON, that a numeric `0`
survives omit-empty, and that `hash` really is the literal string `"hash"`.
