//! End-to-end amended tax on advances adjusted.
//!
//! The intersection of the other three: the original period of `ata` with the
//! unsigned amounts and omitted blank cess of `atadj`. Only what is unique to
//! the combination is tested here.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn atadja() -> &'static SectionSpec {
    spec::section("atadja").expect("atadja is registered")
}

fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(9, 2017).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    }
}

const COLUMNS: [&str; 7] = [
    "Financial Year",
    "Original Month",
    "Original Place Of Supply",
    "Applicable % of Tax Rate",
    "Rate",
    "Gross Advance Adjusted",
    "Cess Amount",
];

fn base(sheet_row: usize) -> Row {
    Row::from_pairs(
        sheet_row,
        COLUMNS
            .into_iter()
            .zip(["2017-18", "JULY", "37-Andhra Pradesh", "", "18", "30000", ""]),
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    let mut r = base(sheet_row);
    r.cells.insert(column.to_owned(), value.to_owned());
    r
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    let report = validate(atadja(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(atadja(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(atadja()).is_empty());
}

#[test]
fn an_amended_adjustment_carries_the_period_and_omits_a_blank_cess() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"omon":"072017","pos":"37","sply_ty":"INTER","itms":[{"rt":18,"ad_amt":30000,"iamt":5400}]}]"#
    );
}

#[test]
fn amounts_stay_unsigned_in_the_amended_table_too() {
    let report = validate(atadja(), &[with(5, "Gross Advance Adjusted", "-100")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Gross Advance Adjusted")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_reduced_rate_scales_the_tax_and_is_emitted() {
    let json = payload(&[with(5, "Applicable % of Tax Rate", "65")], &ctx());
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    assert!(json.contains(r#""iamt":3510"#), "{json}");
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/atadja-sample.csv");
    let rows = gst_core::import::read(&path, atadja()).expect("reads");
    assert_eq!(rows.len(), 2);

    let json = payload(&rows, &ctx());
    assert_eq!(json.matches(r#""omon""#).count(), 2, "{json}");
    assert_eq!(json.matches("csamt").count(), 1, "{json}");
}
