//! Differential test against output captured from GSTN's own offline tool.
//!
//! Every other test in this suite checks the engine against my READING of the
//! reference implementation. This one checks it against the reference's actual
//! behaviour: `fixtures/golden/gstr1-062025-reference.json` is a file the
//! official tool wrote, and the engine must reproduce it byte for byte.
//!
//! Four corrections came out of the first run of this comparison — empty
//! sections being omitted, `diff_percent` surviving only at 0.65, `cname` being
//! stripped, and the filename carrying the generation date. If any of them
//! regress, this test fails.

use std::collections::HashMap;

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::spec;
use gst_core::upload::{self, Turnover};
use gst_core::validate::{FilingContext, validate};

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Rebuild the upload file from the same workbook the golden file came from.
fn build_upload() -> String {
    let ctx = FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(6, 2025).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    };
    let workbook = repo_path("fixtures/gstr1/demo-workbook.xlsx");

    let mut sections: HashMap<String, Vec<gst_core::payload::Json>> = HashMap::new();
    for section in spec::sections() {
        let rows = match gst_core::import::read(&workbook, section) {
            Ok(rows) => rows,
            Err(gst_core::import::ImportError::SheetMissing { .. }) => continue,
            Err(e) => panic!("reading {}: {e}", section.section),
        };
        if rows.is_empty() {
            continue;
        }
        let report = validate(section, &rows, &ctx);
        assert!(
            report.is_clean(),
            "{} should validate cleanly: {:?}",
            section.section,
            report.findings
        );
        let out = generate(section, &report.records, &ctx);
        if !out.envelopes.is_empty() {
            sections.insert(section.section.clone(), out.envelopes);
        }
    }
    upload::build(&sections, &ctx, Turnover::default()).to_json()
}

#[test]
fn the_upload_file_matches_the_captured_reference_byte_for_byte() {
    let golden = std::fs::read_to_string(repo_path("fixtures/golden/gstr1-062025-reference.json"))
        .expect("golden file is present");
    let ours = build_upload();

    if ours != golden {
        // Narrow the failure to the first differing key rather than dumping 9 KB.
        let g: serde_json::Value = serde_json::from_str(&golden).expect("golden parses");
        let o: serde_json::Value = serde_json::from_str(&ours).expect("ours parses");
        let (gk, ok) = (
            g.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
            o.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
        );
        assert_eq!(gk, ok, "top-level keys differ from the reference");
        for key in gk.unwrap_or_default() {
            assert_eq!(
                g.get(&key),
                o.get(&key),
                "section '{key}' differs from the reference"
            );
        }
        panic!(
            "byte output differs though the parsed values match — check key order or number formatting"
        );
    }
}

#[test]
fn the_filename_matches_the_captured_reference() {
    // The golden file's own name encodes the date it was generated.
    let ctx = FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(6, 2025).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    };
    let captured_on = chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap();
    assert_eq!(
        upload::filename(&ctx, captured_on),
        "returns_2672026_R1_27AAPFU0939F1ZV_offline.json"
    );
}
