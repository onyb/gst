//! Differential test against summary metadata captured from GSTN's own tool.
//!
//! `fixtures/golden/gstr1-062025-meta.json` (and its eco sibling) are
//! `_meta.json` sidecars the official tool wrote for the committed fixture
//! workbooks, captured with `scripts/capture_summary_meta.py`. The engine's
//! `summary::meta_json` must reproduce them byte for byte — counts, the four
//! tax-head totals per section, the verbatim labels (including the
//! non-breaking space in the table-15 names, which this comparison caught),
//! and the key order.

mod common;

use gst_core::summary::{meta_json, summarize};

use gst_core::validate::FilingContext;

fn compare_with(workbook: &str, golden_file: &str, ctx: &FilingContext) {
    let golden =
        std::fs::read_to_string(common::repo_path(golden_file)).expect("golden file is present");
    let run = common::clean_run(workbook, ctx);
    let ours = meta_json(&summarize(&run, ctx), ctx).to_json();

    if ours != golden {
        // Narrow the failure to the first differing row rather than dumping 3.5 KB.
        let g: serde_json::Value = serde_json::from_str(&golden).expect("golden parses");
        let o: serde_json::Value = serde_json::from_str(&ours).expect("ours parses");
        for key in ["gstin", "fp", "version", "hash"] {
            assert_eq!(g.get(key), o.get(key), "'{key}' differs from the reference");
        }
        let rows = |v: &serde_json::Value| v["counts"].as_array().cloned().unwrap_or_default();
        let (gr, or) = (rows(&g), rows(&o));
        for (theirs, mine) in gr.iter().zip(or.iter()) {
            assert_eq!(
                theirs, mine,
                "summary row '{}' differs from the reference",
                theirs["cd"]
            );
        }
        assert_eq!(gr.len(), or.len(), "summary row counts differ");
        panic!(
            "byte output differs though the parsed values match — check key order or number formatting"
        );
    }
}

#[test]
fn the_demo_workbook_summary_matches_the_captured_meta_byte_for_byte() {
    compare_with(
        "fixtures/gstr1/demo-workbook.xlsx",
        "fixtures/golden/gstr1-062025-meta.json",
        &common::ctx(6, 2025),
    )
}

#[test]
fn the_eco_workbook_summary_matches_the_captured_meta_byte_for_byte() {
    compare_with(
        "fixtures/gstr1/eco-workbook.xlsx",
        "fixtures/golden/gstr1-eco-062025-meta.json",
        &common::ctx(6, 2025),
    )
}

#[test]
fn the_iff_workbook_summary_matches_the_captured_meta_byte_for_byte() {
    // A quarterly filer in month 1 of a quarter: only the eight IFF rows can
    // appear, with the reduced b2b-shaped e-commerce set.
    let mut ctx = common::ctx(7, 2025);
    ctx.is_quarterly = true;
    compare_with(
        "fixtures/gstr1/iff-workbook.xlsx",
        "fixtures/golden/gstr1-iff-072025-meta.json",
        &ctx,
    )
}
