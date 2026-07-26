//! End-to-end amended credit/debit notes to unregistered persons.
//!
//! The unamended unregistered section plus original-note keys. Covers only what
//! differs; UR-type behaviour is exercised by the cdnur suite.

mod common;

use gst_core::record::Row;
use gst_core::spec::SectionSpec;
use gst_core::validate::{FilingContext, validate};

fn cdnura() -> &'static SectionSpec {
    common::sec("cdnura")
}

fn ctx() -> FilingContext {
    common::ctx(9, 2017)
}

fn base(sheet_row: usize) -> Row {
    common::row(
        cdnura(),
        sheet_row,
        &[
            "B2CL",
            "UN-001",
            "14-Jul-17",
            "UN-001-R",
            "05-Sep-17",
            "C",
            "37-Andhra Pradesh",
            "295000",
            "",
            "18",
            "250000",
            "",
        ],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    common::payload(cdnura(), rows, c)
}

#[test]
fn an_amendment_carries_both_note_identities_at_the_top_level() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"ont_num":"UN-001","ont_dt":"14-07-2017","nt_num":"UN-001-R","nt_dt":"05-09-2017","ntty":"C","val":295000,"typ":"B2CL","pos":"37","itms":[{"num":1801,"itm_det":{"txval":250000,"rt":18,"iamt":45000,"csamt":0}}]}]"#
    );
}

#[test]
fn the_original_note_may_predate_the_return_period() {
    assert!(payload(&[base(5)], &ctx()).contains(r#""ont_dt":"14-07-2017""#));

    let early = with(5, "Original Note Date", "30-Jun-17");
    let report = validate(cdnura(), &[early], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("before 1 July 2017")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_ur_type_rules_still_apply_to_amendments() {
    // Export amendment: place of supply must be blank.
    let e = with(5, "UR Type", "EXPWP");
    let report = validate(cdnura(), &[e], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("cdnura.pos_forbidden_for_exports")),
        "{:?}",
        report.findings
    );

    // Domestic amendment: place of supply is required.
    let blank = with(5, "Place Of Supply", "");
    let report = validate(cdnura(), &[blank], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("cdnura.pos_required_for_domestic")),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_export_amendment_without_payment_zeroes_tax() {
    let e = with(5, "UR Type", "EXPWOP")
        .with_cell("Place Of Supply", "")
        .with_cell("Cess Amount", "750");
    let json = payload(&[e], &ctx());
    assert!(json.contains(r#""iamt":0"#), "{json}");
    assert!(json.contains(r#""csamt":0"#), "{json}");
    assert!(!json.contains(r#""pos""#), "{json}");
}

#[test]
fn both_note_numbers_reject_a_numerically_zero_value() {
    for column in ["Original Note Number", "Revised Note Number"] {
        let report = validate(cdnura(), &[with(5, column, "0")], &ctx());
        assert!(
            report
                .errors()
                .any(|f| f.message.contains("cannot be zero")),
            "{column}: {:?}",
            report.findings
        );
    }
}

#[test]
fn tax_is_never_split_into_central_and_state() {
    let json = payload(&[base(5)], &ctx());
    assert!(!json.contains(r#""camt""#), "{json}");
    assert!(!json.contains(r#""samt""#), "{json}");
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = common::repo_path("fixtures/gstr1/cdnura-sample.csv");
    let rows = gst_core::import::read(&path, cdnura()).expect("reads");
    assert_eq!(rows.len(), 3);

    let json = payload(&rows, &ctx());
    assert_eq!(json.matches(r#""ont_num""#).count(), 3, "{json}");
    assert!(json.contains(r#""typ":"EXPWOP""#), "{json}");
    // The zero note value in GSTN's own sample passes the non-negative check.
    assert!(json.contains(r#""iamt":0"#), "{json}");
}
