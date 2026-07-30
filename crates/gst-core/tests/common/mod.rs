//! Helpers shared by the integration suites.
//!
//! Every roundtrip file drives the same loop — build rows against the spec's
//! template columns, validate, assert clean, generate, assert clean — so the
//! loop lives here once.

#![allow(dead_code)] // each test binary uses the subset it needs

use gst_core::date::ReturnPeriod;
use gst_core::generate::{Generated, generate};
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec};
use gst_core::validate::{FilingContext, validate};

/// The standard test filer: a Maharashtra (27) supplier, so place of supply
/// 27 is intra-state and everything else is inter-state.
pub fn ctx(month: u32, year: i32) -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(month, year).expect("test period is valid"),
        is_sez: false,
        aato_over_5cr: false,
        is_quarterly: false,
    }
}

/// A registered section, by code.
pub fn sec(code: &str) -> &'static SectionSpec {
    spec::section(code).unwrap_or_else(|| panic!("{code} is registered"))
}

/// A row built by zipping the spec's template columns with values, so a spec
/// column reorder surfaces as a loud length/content mismatch instead of
/// silently shifting every literal.
pub fn row(spec: &SectionSpec, sheet_row: usize, values: &[&str]) -> Row {
    let columns = spec.columns();
    assert_eq!(
        columns.len(),
        values.len(),
        "{}: {} template columns but {} values",
        spec.section,
        columns.len(),
        values.len()
    );
    Row::from_pairs(sheet_row, columns.into_iter().zip(values.iter().copied()))
}

/// Validate, assert clean, generate, assert clean — the roundtrip contract
/// every suite drives — and return the grouped output.
pub fn generated(spec: &SectionSpec, rows: &[Row], ctx: &FilingContext) -> Generated {
    let report = validate(spec, rows, ctx);
    assert!(
        report.is_clean(),
        "{} validation: {:?}",
        spec.section,
        report.findings
    );
    let out = generate(spec, &report.records, ctx);
    assert!(
        out.is_clean(),
        "{} generation: {:?}",
        spec.section,
        out.findings
    );
    out
}

/// [`generated`], serialized — for the suites that compare payload JSON.
pub fn payload(spec: &SectionSpec, rows: &[Row], ctx: &FilingContext) -> String {
    generated(spec, rows, ctx).to_json()
}

/// Read a whole fixture workbook and assert it is error-free — the shared
/// preamble of the golden differential suites.
pub fn clean_run(workbook_rel: &str, ctx: &FilingContext) -> gst_core::upload::WorkbookRun {
    let run =
        gst_core::upload::read_workbook(&repo_path(workbook_rel), ctx).expect("workbook reads");
    let errors: Vec<_> = run.errors().collect();
    assert!(errors.is_empty(), "workbook should be clean: {errors:?}");
    run
}

/// Repo-root-relative path, for fixtures.
pub fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}
