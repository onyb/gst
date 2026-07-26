//! End-to-end export invoices.
//!
//! No counterparty at all — the payload groups by export type, one envelope for
//! supplies made with payment of tax and one for supplies made without. Line
//! items are bare: no `num` and no `itm_det` wrapper. Tax is always integrated,
//! and `WOPAY` zeroes both the tax and the cess.

mod common;

use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn exp() -> &'static SectionSpec {
    common::sec("exp")
}

fn ctx() -> FilingContext {
    common::ctx(7, 2017)
}

fn base(sheet_row: usize) -> Row {
    common::row(
        exp(),
        sheet_row,
        &[
            "WPAY",
            "EX-001",
            "14-Jul-17",
            "295000",
            "INMAA1",
            "7896542",
            "18-Jul-17",
            "18",
            "250000",
            "",
        ],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

/// An invoice filed before its shipping bill exists: the whole pair is blank.
fn no_shipping_bill(sheet_row: usize) -> Row {
    base(sheet_row)
        .with_cell("Shipping Bill Number", "")
        .with_cell("Shipping Bill Date", "")
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    common::payload(exp(), rows, c)
}

#[test]
fn an_invoice_groups_under_its_export_type() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"exp_typ":"WPAY","inv":[{"inum":"EX-001","idt":"14-07-2017","val":295000,"sbpcode":"INMAA1","sbnum":"7896542","sbdt":"18-07-2017","itms":[{"txval":250000,"rt":18,"iamt":45000,"csamt":0}]}]}]"#
    );
}

#[test]
fn line_items_carry_no_number_and_no_itm_det_wrapper() {
    let json = payload(&[base(5)], &ctx());
    assert!(!json.contains(r#""num""#), "{json}");
    assert!(!json.contains(r#""itm_det""#), "{json}");
}

#[test]
fn the_export_type_appears_on_the_envelope_and_not_the_invoice() {
    let json = payload(&[base(5)], &ctx());
    assert_eq!(json.matches(r#""exp_typ""#).count(), 1, "{json}");
}

#[test]
fn tax_is_always_integrated() {
    let json = payload(&[base(5)], &ctx());
    // 250000 * 18% = 45000
    assert!(json.contains(r#""iamt":45000"#), "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
    assert!(!json.contains(r#""samt""#), "{json}");
}

#[test]
fn an_export_without_payment_zeroes_tax_and_cess() {
    let w = with(5, "Export Type", "WOPAY").with_cell("Cess Amount", "750");
    let json = payload(&[w], &ctx());
    assert!(json.contains(r#""iamt":0"#), "{json}");
    assert!(json.contains(r#""csamt":0"#), "{json}");
    // The rate is still required, and still emitted.
    assert!(json.contains(r#""rt":18"#), "{json}");

    // With payment of tax, both are computed normally.
    let p = base(6).with_cell("Cess Amount", "750");
    let json = payload(&[p], &ctx());
    assert!(json.contains(r#""iamt":45000"#), "{json}");
    assert!(json.contains(r#""csamt":750"#), "{json}");
}

#[test]
fn the_two_export_types_become_two_envelopes() {
    let json = payload(&[base(5), with(6, "Export Type", "WOPAY")], &ctx());
    assert_eq!(json.matches(r#""exp_typ""#).count(), 2, "{json}");
    assert!(json.contains(r#""exp_typ":"WPAY""#), "{json}");
    assert!(json.contains(r#""exp_typ":"WOPAY""#), "{json}");
}

#[test]
fn several_rates_for_one_invoice_become_one_record() {
    let second = with(6, "Rate", "5")
        .with_cell("Taxable Value", "20000")
        .with_cell("Cess Amount", "500");

    let report = validate(exp(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(exp(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 1);
    let json = out.to_json();
    assert_eq!(json.matches(r#""inum""#).count(), 1, "{json}");
    assert_eq!(json.matches(r#""txval""#).count(), 2, "{json}");
}

#[test]
fn the_shipping_bill_pair_is_optional_but_all_or_nothing() {
    // Both blank is fine, and neither key reaches the payload.
    let json = payload(&[no_shipping_bill(5)], &ctx());
    assert!(!json.contains(r#""sbnum""#), "{json}");
    assert!(!json.contains(r#""sbdt""#), "{json}");

    let number_only = no_shipping_bill(5).with_cell("Shipping Bill Number", "7896542");
    let report = validate(exp(), &[number_only], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("exp.shipping_bill_number_needs_date")),
        "{:?}",
        report.findings
    );

    let date_only = no_shipping_bill(5).with_cell("Shipping Bill Date", "18-Jul-17");
    let report = validate(exp(), &[date_only], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("exp.shipping_bill_date_needs_number")),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_shipping_bill_number_is_three_to_seven_digits() {
    for bad in ["12", "12345678", "ABC1234"] {
        let report = validate(exp(), &[with(5, "Shipping Bill Number", bad)], &ctx());
        assert!(
            report
                .errors()
                .any(|f| f.column.as_deref() == Some("Shipping Bill Number")),
            "'{bad}' should be rejected: {:?}",
            report.findings
        );
    }
    assert!(
        validate(exp(), &[with(5, "Shipping Bill Number", "123")], &ctx()).is_clean(),
        "three digits is the shortest accepted"
    );
}

#[test]
fn the_port_code_is_optional_and_unchecked_against_real_ports() {
    // Six alphanumerics of any kind pass; the reference has no list of ports.
    assert!(validate(exp(), &[with(5, "Port Code", "ZZ9999")], &ctx()).is_clean());

    let json = payload(&[with(5, "Port Code", "")], &ctx());
    assert!(!json.contains(r#""sbpcode""#), "{json}");

    let report = validate(exp(), &[with(5, "Port Code", "TOOLONG")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Port Code")),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_unknown_export_type_is_rejected() {
    for bad in ["", "WITHPAY", "wpay "] {
        let report = validate(exp(), &[with(5, "Export Type", bad)], &ctx());
        assert!(
            report
                .errors()
                .any(|f| f.column.as_deref() == Some("Export Type")),
            "'{bad}' should be rejected: {:?}",
            report.findings
        );
    }
}

#[test]
fn no_applicable_percent_of_tax_rate_column_exists() {
    // The reference defaults one internally, but it never reaches the payload
    // and the template offers no column for it.
    assert!(
        !exp().fields.iter().any(|f| f.id == "diff_percent"),
        "exp should declare no tax-rate factor field"
    );
    assert!(!payload(&[base(5)], &ctx()).contains("diff_percent"));
}

#[test]
fn rows_sharing_an_invoice_number_must_agree_on_its_details() {
    let conflicting = with(6, "Rate", "5")
        .with_cell("Taxable Value", "20000")
        .with_cell("Invoice Value", "1");
    let report = validate(exp(), &[base(5), conflicting], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(exp(), &report.records, &ctx());
    assert!(
        out.findings
            .iter()
            .any(|f| f.severity == Severity::Error && f.message.contains("Invoice Value")),
        "{:?}",
        out.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = common::repo_path("fixtures/gstr1/exp-sample.csv");
    let rows = gst_core::import::read(&path, exp()).expect("reads");
    assert_eq!(rows.len(), 4);

    let json = payload(&rows, &ctx());
    // Four rows, two envelopes, three invoices: two rates share the first.
    assert_eq!(json.matches(r#""exp_typ""#).count(), 2, "{json}");
    assert_eq!(json.matches(r#""inum""#).count(), 3, "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
}
