#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["openpyxl>=3.1"]
# ///
"""Rebuild fixtures/gstr3b/form-workbook.xlsx — a plain-value replica of the
official GSTR-3B utility's data-entry sheet.

The official .xlsm is never committed (it contains GSTN's VBA); this workbook
carries only a sheet named `GSTR-3B` with literal values at the same cell
addresses, formula cells written as the values the formulas would show. The
period is FY 2024-25 / January (ret_period 012025), chosen so table 3.1.1,
inter_sup, outward negatives and ITC negatives are all live.

Also prints the cell -> value manifest for the Mac Excel oracle session: fill
the real utility with exactly these values, Validate, Create JSON, and compare
the produced file semantically against fixtures/golden/gstr3b-012025-expected.json.
"""

from pathlib import Path

import openpyxl

VALUES = {
    # header
    "C5": "27AAPFU0939F1ZV",
    "C6": "Test Traders Pvt Ltd",
    "G5": "2024-25",
    "G6": "January",
    # 3.1 (F11/F14 are =E formulas in the utility; written as their values)
    "C11": 500000.555, "D11": 50000, "E11": 25000, "F11": 25000, "G11": 1000,
    "C12": 200000, "D12": 0, "G12": 0,
    "C13": -15000,
    "C14": 80000, "D14": 8000, "E14": 4000, "F14": 4000, "G14": 0,
    "C15": 30000,
    # 3.1.1 (F22 = E22)
    "C22": 100000, "D22": 18000, "E22": 0, "F22": 0, "G22": 0,
    "C23": 50000,
    # 4(A) ITC available (E33/E34/E35 = D formulas)
    "C31": 5000, "F31": 100,
    "C32": 2000, "F32": 0,
    "C33": 8000, "D33": 1500, "E33": 1500, "F33": 200,
    "C34": 1000, "D34": 500, "E34": 500, "F34": 0,
    "C35": -3000, "D35": 2500, "E35": 2500, "F35": 300,
    # 4(B) ITC reversed (E37 = D37; E38 is a REAL input, set != D38 on purpose)
    "C37": 1000, "D37": 200, "E37": 200, "F37": 50,
    "C38": 500, "D38": 100, "E38": 90, "F38": 0,
    # 4(C) net — cached formula results, matching the recomputed values
    "C39": 11500, "D39": 4200, "E39": 4210, "F39": 550,
    # 4(D) ineligible (E41/E42 = D formulas)
    "C41": 700, "D41": 150, "E41": 150, "F41": 0,
    "C42": -200, "D42": 80, "E42": 80, "F42": 0,
    # 5
    "D48": 12000, "E48": 6000,
    "D49": 3000, "E49": 1500,
    # 5.1 (C65 pins the double-rounding edge: 100.005 as a double is
    # 100.00499…, so Excel ROUND gives 100.0, not 100.01)
    "C65": 100.005, "D65": 50, "E65": 50, "F65": 0,
    # late fee: raw, more than two decimals, samt deliberately blank
    "D66": 125.456,
    # 3.2 — unreg pair, comp pair (iamt blank -> coerced 0), uin pair, and a
    # row feeding two pairs at once
    "B88": "06-Haryana", "C88": 10000, "D88": 1800,
    "B89": "09-Uttar Pradesh", "E89": 5000,
    "B90": "32-Kerala", "G90": 2000, "H90": 360,
    "B91": "19-West Bengal", "C91": 1000, "D91": 180, "G91": 500, "H91": 90,
}


def main() -> None:
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "GSTR-3B"
    for cell, value in VALUES.items():
        ws[cell] = value
    out = Path("fixtures/gstr3b/form-workbook.xlsx")
    out.parent.mkdir(parents=True, exist_ok=True)
    wb.save(out)
    print(f"wrote {out}\n")
    print("Oracle manifest — fill the official V5.8 utility with exactly:")
    for cell, value in VALUES.items():
        print(f"  {cell:5} = {value}")


if __name__ == "__main__":
    main()
