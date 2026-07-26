# Golden files captured from GSTN's offline tool

Reference output, captured by running the official Returns Offline Tool
(V3.2.4) and taking the file it wrote. These are the only artifacts in this
repository that come from the tool's own behaviour rather than from reading its
source, so they are what makes "matches the reference" a checkable claim
instead of an assertion.

| File | Captured from | Input |
|---|---|---|
| `gstr1-062025-reference.json` | tool V3.2.4, full import → generate chain | `fixtures/gstr1/demo-workbook.xlsx`, GSTIN `27AAPFU0939F1ZV`, period `062025` |

## How it was captured

The tool's row-to-payload mapping runs **server-side**, so the whole pipeline is
reachable over HTTP without driving the browser UI:

1. `POST /addtblfile` — multipart, `file` = the workbook, `shareData` = a JSON
   blob carrying `dashBoardDt` (form, gstin, fp), `yearsList` / `curFyMonths`
   (financial years with their months and `MMYYYY` values, which the amendment
   sections need in order to resolve an original period), `monthSelected`, and
   the `isSezTaxpayer` / `isTPQ` / `isUploadImport` flags. This parses the
   sheets, maps every row, and returns a `cache_key` plus per-section error
   lists — empty error lists mean the tool accepted the workbook.
2. `POST /addmltpldata` with `tbl_data: {cache_key}` — writes the working file at
   `public/userData/<gstin>/<form>/<fy>/<month>/`, holding the tool's own mapped
   payload.
3. `POST /generateFile` — applies the upload transformation (omit-empty and the
   strip functions) and writes the upload artifact.

Running all three matters. An earlier version of this capture skipped steps 1
and 2 and seeded the working file with output from *this* implementation, which
verified only the upload transformation — the mapping layer was being compared
against nothing, and a mapping bug survived precisely because of it (below).

Two obstacles are worth recording, because they will recur:

1. `generateFile` creates its output directory with an async `mkdirp` whose
   callback is empty, then writes into it immediately — so the write loses the
   race and fails with ENOENT. Its error path then calls an undefined
   `callback`, crashing the request. Pre-creating the timestamped output
   directory avoids both. (`addmltpldata` handles its own `mkdirp` correctly;
   only `generateFile` has the bug.)
2. The directory name embeds hours, minutes and seconds, so it cannot be
   predicted exactly; creating every second of the current and next minute is
   enough.

## What this file settled

From the first capture, which exercised the upload transformation only:

- Empty sections are **omitted** from the upload file, not emitted as `[]`.
- `diff_percent` is emitted **only** when it is `0.65`; every other value is
  discarded, across all ten sections that carry it.
- `cname` is **stripped** for b2b, b2ba, cdnr and cdnra — the recipient's name
  never reaches the portal.
- The filename's date segment is the **generation** date (day, month, year, no
  zero padding), not the return period.

It also confirmed `diffval` never reaches the upload JSON, that a numeric `0`
survives omit-empty, and that `hash` really is the literal string `"hash"`.

From the second capture, which added the import and mapping steps:

- **A blank Cess Amount in b2cl or b2cla emits no `csamt` key at all.** Those two
  tables compute cess without the empty-cell guard the others use, so a blank
  cell becomes NaN, the working file records `null`, and omit-empty drops the
  key. This had been recorded as a deliberate divergence emitting `0`, on the
  reasoning that `null` for a numeric tax field looked more like a defect than an
  interface. The capture showed the `null` is never observable at the interface,
  and that the interface is simply "no cess, no key". Every other section — b2b,
  b2cs, b2csa, cdnr, cdnur, cdnura, exp, expa — does emit `0` for a blank cell,
  so both behaviours coexist and the source reading was right about which is
  which.
- **The workbook's dates have to be text**, not date-formatted cells — see
  `spec/README.md`. This surfaced because step 1 rejected every row of a workbook
  the seeding method had never fed through the importer.
- The tool's own mapping of `exp` and `expa` matches this implementation exactly:
  grouping by export type, the bare line-item shape with no `num` and no
  `itm_det`, and `WOPAY` zeroing both the tax and the cess.

From a third capture, of a workbook with only one HSN half populated:

- **Omit-empty is recursive.** A nested object drops its empty members rather
  than carrying them as `[]`: an HSN section with only a B2B half emits
  `"hsn": {"hsn_b2b": [...]}` with no `hsn_b2c` at all. This had been inferred
  and flagged as unverified in the envelope spec, because the earlier captures
  happened to populate both halves. This implementation already behaved this
  way, and the one-sided file is byte-identical too.
