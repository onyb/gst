//! End-to-end amended B2C(Small): records at the top level, but with items.
//!
//! A third payload shape. The unamended section is flat with no items; the
//! invoice sections nest items under an envelope. This one puts its records
//! directly in the payload array while still carrying line items, and its items
//! have neither a `num` nor an `itm_det` wrapper.
//!
//! It is also the first section that identifies a period by a spelled-out month
//! plus a financial year rather than a date.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn b2csa() -> &'static SectionSpec {
    spec::section("b2csa").expect("b2csa is registered")
}

/// A Maharashtra (27) supplier filing for September 2017.
fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(9, 2017).unwrap(),
        is_sez: false,
    }
}

/// Excel-template order, which is what the spec records.
const COLUMNS: [&str; 9] = [
    "Financial Year",
    "Original Month",
    "Place Of Supply",
    "Type",
    "Applicable % of Tax Rate",
    "Rate",
    "Taxable Value",
    "Cess Amount",
    "E-Commerce GSTIN",
];

fn base(sheet_row: usize) -> Row {
    Row::from_pairs(
        sheet_row,
        COLUMNS.into_iter().zip([
            "2017-18",
            "JULY",
            "37-Andhra Pradesh",
            "OE",
            "",
            "18",
            "300000",
            "",
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
    let report = validate(b2csa(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(b2csa(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(b2csa()).is_empty());
    // Not flat: it has an item level, unlike the unamended section.
    assert!(!b2csa().is_flat());
}

#[test]
fn a_record_sits_at_the_top_level_and_carries_items() {
    // No envelope wrapper, no item `num`, no `itm_det` nesting.
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"omon":"072017","sply_ty":"INTER","typ":"OE","pos":"37","itms":[{"rt":18,"txval":300000,"iamt":54000,"csamt":0}]}]"#
    );
}

#[test]
fn the_amended_period_respects_the_financial_year_boundary() {
    // Indian financial years run April to March, so JANUARY of 2017-18 is
    // January 2018 — the month name alone would be ambiguous.
    let jan = with(5, "Original Month", "JANUARY");
    assert!(payload(&[jan], &ctx()).contains(r#""omon":"012018""#));

    let jul = with(5, "Original Month", "JULY");
    assert!(payload(&[jul], &ctx()).contains(r#""omon":"072017""#));

    let mar = with(5, "Original Month", "MARCH");
    assert!(payload(&[mar], &ctx()).contains(r#""omon":"032018""#));
}

#[test]
fn the_month_must_be_uppercase() {
    // The reference's month pattern is case-sensitive even though its own
    // period lookup compares case-insensitively.
    let report = validate(b2csa(), &[with(5, "Original Month", "July")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Original Month")),
        "{:?}",
        report.findings
    );
    assert!(validate(b2csa(), &[with(5, "Original Month", "JULY")], &ctx()).is_clean());
}

#[test]
fn an_unknown_financial_year_is_rejected() {
    let report = validate(b2csa(), &[with(5, "Financial Year", "2016-17")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Financial Year")),
        "{:?}",
        report.findings
    );
}

#[test]
fn several_rates_for_one_amended_period_become_one_record() {
    let mut second = with(6, "Rate", "5");
    second.cells.insert("Taxable Value".into(), "120000".into());
    second.cells.insert("Cess Amount".into(), "2500".into());

    let report = validate(b2csa(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(b2csa(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 1, "one record");

    let json = out.to_json();
    assert_eq!(json.matches(r#""rt""#).count(), 2, "two items: {json}");
    assert!(json.contains(r#""csamt":2500"#), "{json}");
}

#[test]
fn a_different_amended_month_is_a_different_record() {
    let other = with(6, "Original Month", "AUGUST");
    let report = validate(b2csa(), &[base(5), other], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(b2csa(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 2);
}

#[test]
fn the_suppliers_own_state_is_intra_state() {
    let own = with(5, "Place Of Supply", "27-Maharashtra");
    let json = payload(&[own], &ctx());
    assert!(json.contains(r#""sply_ty":"INTRA""#), "{json}");
    assert!(json.contains(r#""camt":27000,"samt":27000"#), "{json}");
    assert!(!json.contains(r#""iamt""#), "{json}");
}

#[test]
fn an_unscaled_rate_factor_is_dropped() {
    assert!(!payload(&[base(5)], &ctx()).contains("diff_percent"));
    let scaled = with(5, "Applicable % of Tax Rate", "65");
    let json = payload(&[scaled], &ctx());
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    assert!(json.contains(r#""iamt":35100"#), "{json}");
}

#[test]
fn negative_values_carry_through_to_tax() {
    let neg = with(5, "Taxable Value", "-75000");
    let json = payload(&[neg], &ctx());
    assert!(json.contains(r#""txval":-75000"#), "{json}");
    assert!(json.contains(r#""iamt":-13500"#), "{json}");
}

#[test]
fn type_e_is_rejected_even_with_a_valid_operator_gstin() {
    let mut e = with(5, "Type", "E");
    e.cells
        .insert("E-Commerce GSTIN".into(), "12AJIPA1572E1C7".into());
    let report = validate(b2csa(), &[e], &ctx());
    assert!(
        report.errors().any(|f| f.column.as_deref() == Some("Type")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    // Fixture is in the SECTION CSV's column order, which swaps Rate and
    // Applicable % of Tax Rate relative to the Excel template.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/b2csa-sample.csv");
    let rows = gst_core::import::read(&path, b2csa()).expect("reads");
    assert_eq!(rows.len(), 5);

    let json = payload(&rows, &ctx());
    // Five rows collapse to four records: two share an amended period, state
    // and rate factor, so they become two items of one record.
    assert_eq!(json.matches(r#""omon""#).count(), 4, "{json}");
    assert!(json.contains(r#""omon":"012018""#), "FY rollover: {json}");
    assert_eq!(json.matches(r#""INTRA""#).count(), 1, "{json}");
    assert!(json.contains(r#""txval":-75000"#), "{json}");
}
