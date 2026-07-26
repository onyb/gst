//! End-to-end amended export invoices.
//!
//! Same envelope, item shape and tax treatment as the unamended section — those
//! are exercised by the `exp` suite. What differs is the extra original
//! invoice number and date, which the payload carries alongside the revised
//! pair.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn expa() -> &'static SectionSpec {
    spec::section("expa").expect("expa is registered")
}

fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(9, 2017).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    }
}

const COLUMNS: [&str; 12] = [
    "Export Type",
    "Original Invoice Number",
    "Original Invoice date",
    "Revised Invoice Number",
    "Revised Invoice date",
    "Invoice Value",
    "Port Code",
    "Shipping Bill Number",
    "Shipping Bill Date",
    "Rate",
    "Taxable Value",
    "Cess Amount",
];

fn base(sheet_row: usize) -> Row {
    Row::from_pairs(
        sheet_row,
        COLUMNS.into_iter().zip([
            "WPAY",
            "EX-101",
            "14-Jul-17",
            "EX-101-R",
            "05-Sep-17",
            "295000",
            "INMAA1",
            "7896542",
            "08-Sep-17",
            "18",
            "250000",
            "",
        ]),
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    let mut r = base(sheet_row);
    r.cells.insert(column.to_owned(), value.to_owned());
    r
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    let report = validate(expa(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(expa(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(expa()).is_empty());
}

#[test]
fn an_amendment_carries_both_the_original_and_the_revised_invoice() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"exp_typ":"WPAY","inv":[{"oinum":"EX-101","oidt":"14-07-2017","inum":"EX-101-R","idt":"05-09-2017","val":295000,"sbpcode":"INMAA1","sbnum":"7896542","sbdt":"08-09-2017","itms":[{"txval":250000,"rt":18,"iamt":45000,"csamt":0}]}]}]"#
    );
}

#[test]
fn the_original_invoice_may_predate_the_period_being_filed() {
    // A July invoice amended in the September return is the normal case.
    assert!(validate(expa(), &[base(5)], &ctx()).is_clean());

    let too_early = with(5, "Original Invoice date", "30-Jun-17");
    let report = validate(expa(), &[too_early], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Original Invoice date")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_revised_invoice_number_may_equal_the_original() {
    let unchanged = with(5, "Revised Invoice Number", "EX-101");
    let json = payload(&[unchanged], &ctx());
    assert!(json.contains(r#""oinum":"EX-101""#), "{json}");
    assert!(json.contains(r#""inum":"EX-101""#), "{json}");
}

#[test]
fn both_invoice_numbers_are_required() {
    for column in ["Original Invoice Number", "Revised Invoice Number"] {
        let report = validate(expa(), &[with(5, column, "")], &ctx());
        assert!(
            report.errors().any(|f| f.column.as_deref() == Some(column)),
            "{column} should be required: {:?}",
            report.findings
        );
    }
}

#[test]
fn an_export_without_payment_zeroes_tax_and_cess() {
    let mut w = with(5, "Export Type", "WOPAY");
    w.cells.insert("Cess Amount".into(), "750".into());
    let json = payload(&[w], &ctx());
    assert!(json.contains(r#""iamt":0"#), "{json}");
    assert!(json.contains(r#""csamt":0"#), "{json}");
}

#[test]
fn line_items_carry_no_number_and_no_itm_det_wrapper() {
    let json = payload(&[base(5)], &ctx());
    assert!(!json.contains(r#""num""#), "{json}");
    assert!(!json.contains(r#""itm_det""#), "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
}

#[test]
fn the_shipping_bill_pair_stays_all_or_nothing() {
    let mut number_only = base(5);
    number_only
        .cells
        .insert("Shipping Bill Date".into(), String::new());
    let report = validate(expa(), &[number_only], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("expa.shipping_bill_number_needs_date")),
        "{:?}",
        report.findings
    );
}

#[test]
fn several_rates_for_one_amendment_become_one_record() {
    let mut second = with(6, "Rate", "5");
    second.cells.insert("Taxable Value".into(), "20000".into());
    second.cells.insert("Cess Amount".into(), "500".into());

    let report = validate(expa(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(expa(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 1);
    let json = out.to_json();
    assert_eq!(json.matches(r#""oinum""#).count(), 1, "{json}");
    assert_eq!(json.matches(r#""txval""#).count(), 2, "{json}");
}

#[test]
fn rows_sharing_a_revised_number_must_agree_on_the_original() {
    let mut conflicting = with(6, "Rate", "5");
    conflicting
        .cells
        .insert("Taxable Value".into(), "20000".into());
    conflicting
        .cells
        .insert("Original Invoice Number".into(), "EX-999".into());
    let report = validate(expa(), &[base(5), conflicting], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(expa(), &report.records, &ctx());
    assert!(
        out.findings.iter().any(|f| f.severity == Severity::Error
            && f.message.contains("Original Invoice Number")),
        "{:?}",
        out.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/expa-sample.csv");
    let rows = gst_core::import::read(&path, expa()).expect("reads");
    assert_eq!(rows.len(), 3);

    let json = payload(&rows, &ctx());
    // Three rows, two envelopes, two amendments: two rates share the first.
    assert_eq!(json.matches(r#""exp_typ""#).count(), 2, "{json}");
    assert_eq!(json.matches(r#""oinum""#).count(), 2, "{json}");
    // The WOPAY row gives a port code but no shipping bill yet.
    assert!(json.contains(r#""sbpcode":"INNSA1""#), "{json}");
}
