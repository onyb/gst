//! End-to-end nil-rated, exempted and non-GST outward supplies.
//!
//! A flat section: four fixed categories, one payload object per row, no
//! invoices and no line items. The only mapping of substance is the category
//! label becoming a code, and the payload's key order differing from the
//! template's column order.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn nil() -> &'static SectionSpec {
    spec::section("nil").expect("nil is registered")
}

fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(7, 2017).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    }
}

const COLUMNS: [&str; 4] = [
    "Description",
    "Nil Rated Supplies",
    "Exempted(other than nil rated/non GST supply)",
    "Non-GST Supplies",
];

fn base(sheet_row: usize) -> Row {
    Row::from_pairs(
        sheet_row,
        COLUMNS.into_iter().zip([
            "Inter-State supplies to registered persons",
            "21143",
            "51235",
            "5213",
        ]),
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    let mut r = base(sheet_row);
    r.cells.insert(column.to_owned(), value.to_owned());
    r
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    let report = validate(nil(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(nil(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(nil()).is_empty());
}

#[test]
fn a_row_becomes_one_flat_record_with_the_category_as_a_code() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"sply_ty":"INTRB2B","expt_amt":51235,"nil_amt":21143,"ngsup_amt":5213}]"#
    );
}

#[test]
fn the_payload_orders_exempted_before_nil_rated_unlike_the_template() {
    // The template reads Nil, Exempted, Non-GST; the payload emits the
    // exempted amount first. Emitting in column order would be byte-different.
    let json = payload(&[base(5)], &ctx());
    let expt = json.find("expt_amt").expect("expt_amt present");
    let nil_amt = json.find("nil_amt").expect("nil_amt present");
    let ngsup = json.find("ngsup_amt").expect("ngsup_amt present");
    assert!(expt < nil_amt && nil_amt < ngsup, "{json}");
}

#[test]
fn each_of_the_four_categories_maps_to_its_own_code() {
    for (label, code) in [
        ("Inter-State supplies to registered persons", "INTRB2B"),
        ("Intra-State supplies to registered persons", "INTRAB2B"),
        ("Inter-State supplies to unregistered persons", "INTRB2C"),
        ("Intra-State supplies to unregistered persons", "INTRAB2C"),
    ] {
        let json = payload(&[with(5, "Description", label)], &ctx());
        assert!(json.contains(&format!(r#""sply_ty":"{code}""#)), "{json}");
    }
}

#[test]
fn inter_state_is_intr_and_intra_state_is_intra() {
    // One letter apart, and the longer code belongs to the shorter word — the
    // easiest pair in the whole spec to transpose.
    let inter = payload(
        &[with(5, "Description", "Inter-State supplies to registered persons")],
        &ctx(),
    );
    let intra = payload(
        &[with(5, "Description", "Intra-State supplies to registered persons")],
        &ctx(),
    );
    assert!(inter.contains(r#""sply_ty":"INTRB2B""#), "{inter}");
    assert!(intra.contains(r#""sply_ty":"INTRAB2B""#), "{intra}");
}

#[test]
fn an_unknown_category_is_rejected() {
    for bad in ["", "Inter-State supplies", "INTRB2B", "inter-state supplies to registered persons"] {
        let report = validate(nil(), &[with(5, "Description", bad)], &ctx());
        assert!(
            report
                .errors()
                .any(|f| f.column.as_deref() == Some("Description")),
            "'{bad}' should be rejected: {:?}",
            report.findings
        );
    }
}

#[test]
fn a_blank_amount_is_emitted_as_zero() {
    // Unlike the B2C(Large) tables, no amount key is ever omitted here: the
    // reference falls back to 0 for anything falsy.
    let json = payload(&[with(5, "Non-GST Supplies", "")], &ctx());
    assert!(json.contains(r#""ngsup_amt":0"#), "{json}");
    assert_eq!(json.matches("amt").count(), 3, "{json}");
}

#[test]
fn negative_amounts_are_accepted() {
    // The reference's pattern carries an optional leading minus, which most
    // other sections' amount patterns do not.
    let json = payload(&[with(5, "Nil Rated Supplies", "-500.25")], &ctx());
    assert!(json.contains(r#""nil_amt":-500.25"#), "{json}");
}

#[test]
fn an_amount_beyond_the_declared_precision_is_rejected() {
    for bad in ["1.234", "123456789012"] {
        let report = validate(nil(), &[with(5, "Nil Rated Supplies", bad)], &ctx());
        assert!(
            report
                .errors()
                .any(|f| f.column.as_deref() == Some("Nil Rated Supplies")),
            "'{bad}' should be rejected: {:?}",
            report.findings
        );
    }
}

#[test]
fn every_row_becomes_its_own_record_in_row_order() {
    let second = with(6, "Description", "Intra-State supplies to registered persons");
    let report = validate(nil(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(nil(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 2);
    let json = out.to_json();
    assert!(
        json.find("INTRB2B").unwrap() < json.find("INTRAB2B").unwrap(),
        "{json}"
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/exemp-sample.csv");
    let rows = gst_core::import::read(&path, nil()).expect("reads");
    assert_eq!(rows.len(), 4);

    let json = payload(&rows, &ctx());
    assert_eq!(json.matches("sply_ty").count(), 4, "{json}");
    // The last row leaves Non-GST Supplies blank, which still emits a zero.
    assert!(json.ends_with(r#""ngsup_amt":0}]"#), "{json}");
}
