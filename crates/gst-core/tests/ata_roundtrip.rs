//! End-to-end amended tax on advances received.
//!
//! Same record and item shape as the unamended table, which the `at` suite
//! covers. What differs is the original period: a financial year and a
//! spelled-out month resolve to the `omon` the payload carries, and that
//! period must agree across rows sharing a place of supply.

mod common;

use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn ata() -> &'static SectionSpec {
    common::sec("ata")
}

fn ctx() -> FilingContext {
    common::ctx(9, 2017)
}

fn base(sheet_row: usize) -> Row {
    common::row(
        ata(),
        sheet_row,
        &[
            "2017-18",
            "JULY",
            "37-Andhra Pradesh",
            "",
            "18",
            "100000",
            "",
        ],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    common::payload(ata(), rows, c)
}

#[test]
fn the_amended_period_leads_the_record() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"omon":"072017","pos":"37","sply_ty":"INTER","itms":[{"rt":18,"ad_amt":100000,"iamt":18000,"csamt":0}]}]"#
    );
}

#[test]
fn the_original_period_comes_from_the_year_and_the_month() {
    // March belongs to the closing calendar year of the financial year.
    let mut r = with(5, "Original Month", "MARCH");
    r.cells.insert("Financial Year".into(), "2017-18".into());
    let json = payload(&[r], &ctx());
    assert!(json.contains(r#""omon":"032018""#), "{json}");
}

#[test]
fn an_unknown_year_or_month_is_rejected() {
    for (column, bad) in [("Financial Year", "2017"), ("Original Month", "Jul")] {
        let report = validate(ata(), &[with(5, column, bad)], &ctx());
        assert!(
            report.errors().any(|f| f.column.as_deref() == Some(column)),
            "'{bad}' in {column} should be rejected: {:?}",
            report.findings
        );
    }
}

#[test]
fn rows_sharing_a_place_of_supply_must_agree_on_the_amended_period() {
    let mut conflicting = with(6, "Original Month", "AUGUST");
    conflicting.cells.insert("Rate".into(), "5".into());
    let report = validate(ata(), &[base(5), conflicting], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(ata(), &report.records, &ctx());
    assert!(
        out.findings.iter().any(|f| f.severity == Severity::Error),
        "two periods for one state should be rejected: {:?}",
        out.findings
    );
}

#[test]
fn the_suppliers_own_state_takes_the_central_state_split() {
    let json = payload(
        &[with(5, "Original Place Of Supply", "27-Maharashtra")],
        &ctx(),
    );
    assert!(json.contains(r#""sply_ty":"INTRA""#), "{json}");
    assert!(json.contains(r#""camt":9000"#), "{json}");
}

#[test]
fn a_blank_cess_is_emitted_as_zero() {
    let json = payload(&[base(5)], &ctx());
    assert!(json.contains(r#""csamt":0"#), "{json}");
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = common::repo_path("fixtures/gstr1/ata-sample.csv");
    let rows = gst_core::import::read(&path, ata()).expect("reads");
    assert_eq!(rows.len(), 3);

    let json = payload(&rows, &ctx());
    assert_eq!(json.matches(r#""omon""#).count(), 2, "{json}");
    assert!(json.contains(r#""sply_ty":"INTRA""#), "{json}");
}
