//! End-to-end amended B2B: what an amendment adds over the unamended section.
//!
//! The shared machinery (tax split, item numbering, envelope grouping) is
//! covered by the B2B suite; these focus on the original-invoice keys and on
//! the parts where the two sections legitimately differ.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn b2ba() -> &'static SectionSpec {
    spec::section("b2ba").expect("b2ba is registered")
}

/// A Maharashtra supplier amending, in September 2017, invoices first reported
/// in July.
fn september() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(9, 2017).unwrap(),
        is_sez: false,
    }
}

const COLUMNS: [&str; 15] = [
    "GSTIN/UIN of Recipient",
    "Receiver Name",
    "Original Invoice Number",
    "Original Invoice date",
    "Revised Invoice Number",
    "Revised Invoice date",
    "Invoice Value",
    "Place Of Supply",
    "Reverse Charge",
    "Applicable % of Tax Rate",
    "Invoice Type",
    "E-Commerce GSTIN",
    "Rate",
    "Taxable Value",
    "Cess Amount",
];

fn row(sheet_row: usize, values: [&str; 15]) -> Row {
    Row::from_pairs(sheet_row, COLUMNS.into_iter().zip(values))
}

/// A clean single-rate amendment, as a base for targeted mutation.
fn base(sheet_row: usize) -> Row {
    row(
        sheet_row,
        [
            "12GEOPS0823BBZH",
            "Acme Traders",
            "INV-001",
            "14-Jul-17",
            "INV-001-R",
            "05-Sep-17",
            "118000",
            "37-Andhra Pradesh",
            "N",
            "",
            "Regular B2B",
            "",
            "18",
            "100000",
            "",
        ],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    let mut r = base(sheet_row);
    r.cells.insert(column.to_owned(), value.to_owned());
    r
}

fn payload(rows: &[Row], ctx: &FilingContext) -> String {
    let report = validate(b2ba(), rows, ctx);
    assert!(
        report.is_clean(),
        "validation should be clean, got {:?}",
        report.findings
    );
    let out = generate(b2ba(), &report.records, ctx);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation should be clean, got {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(b2ba()).is_empty());
}

#[test]
fn an_amendment_carries_both_the_original_and_revised_identity() {
    assert_eq!(
        payload(&[base(5)], &september()),
        r#"[{"ctin":"12GEOPS0823BBZH","cname":"Acme Traders","inv":[{"oinum":"INV-001","oidt":"14-07-2017","inum":"INV-001-R","idt":"05-09-2017","val":118000,"pos":"37","rchrg":"N","diff_percent":1,"inv_typ":"R","itms":[{"num":1801,"itm_det":{"txval":100000,"rt":18,"iamt":18000,"csamt":0}}]}]}]"#
    );
}

#[test]
fn the_original_invoice_may_predate_the_return_period() {
    // The whole point of an amendment: July's invoice corrected in September.
    // Only the GST start date bounds it.
    let json = payload(&[base(5)], &september());
    assert!(json.contains(r#""oidt":"14-07-2017""#), "{json}");

    let too_early = with(5, "Original Invoice date", "30-Jun-17");
    let report = validate(b2ba(), &[too_early], &september());
    assert!(!report.is_clean());
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("before 1 July 2017")),
        "{:?}",
        report.findings
    );
}

#[test]
fn nothing_requires_the_revision_to_follow_the_original() {
    // A deliberate faithfulness call: the reference's date-ordering check is
    // unreachable, so an amendment dated before its own original is accepted.
    // Recorded as a quirk on the spec rather than silently tightened.
    let backwards = with(5, "Revised Invoice date", "01-Jul-17");
    let report = validate(b2ba(), &[backwards], &september());
    assert!(report.is_clean(), "{:?}", report.findings);
}

#[test]
fn the_revised_number_may_equal_the_original() {
    // Common when only the value or place of supply was wrong.
    let mut r = base(5);
    r.cells
        .insert("Revised Invoice Number".into(), "INV-001".into());
    let json = payload(&[r], &september());
    assert!(
        json.contains(r#""oinum":"INV-001","oidt":"14-07-2017","inum":"INV-001""#),
        "{json}"
    );
}

#[test]
fn both_invoice_numbers_reject_a_numerically_zero_value() {
    for column in ["Original Invoice Number", "Revised Invoice Number"] {
        let report = validate(b2ba(), &[with(5, column, "0")], &september());
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
fn rows_grouping_as_one_amendment_must_agree_on_the_original_invoice() {
    // Same recipient and revised number, contradictory original number: the
    // reference rejects this, and so does the field-agreement check.
    let mut second = base(6);
    second.cells.insert("Rate".into(), "5".into());
    second.cells.insert("Taxable Value".into(), "20000".into());
    second
        .cells
        .insert("Original Invoice Number".into(), "INV-999".into());

    let report = validate(b2ba(), &[base(5), second], &september());
    assert!(report.is_clean(), "{:?}", report.findings);

    let out = generate(b2ba(), &report.records, &september());
    let finding = out
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("grouping.field_disagreement"))
        .expect("disagreement is reported");
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.sheet_row, 6);
    assert_eq!(finding.column.as_deref(), Some("Original Invoice Number"));
}

#[test]
fn a_multi_rate_amendment_becomes_one_record_with_several_items() {
    let mut second = base(6);
    second.cells.insert("Rate".into(), "5".into());
    second.cells.insert("Taxable Value".into(), "20000".into());

    let json = payload(&[base(5), second], &september());
    assert_eq!(json.matches(r#""oinum":"INV-001""#).count(), 1, "{json}");
    assert!(json.contains(r#""num":1801"#), "{json}");
    assert!(json.contains(r#""num":501"#), "{json}");
}

#[test]
fn the_tax_split_behaves_as_it_does_for_unamended_invoices() {
    // Same derivation, driven by the corrected values — the first real check
    // that a derivation is reusable across sections.
    let intra = with(5, "Place Of Supply", "27-Maharashtra");
    let json = payload(&[intra], &september());
    assert!(json.contains(r#""camt":9000,"samt":9000"#), "{json}");
    assert!(!json.contains("iamt"), "{json}");

    let sez = with(5, "Invoice Type", "SEZ supplies without payment");
    let json = payload(&[sez], &september());
    assert!(json.contains(r#""iamt":0,"csamt":0"#), "{json}");
}

#[test]
fn cross_field_rules_apply_to_the_corrected_invoice_type() {
    let cbw = with(5, "Invoice Type", "Intra-State supplies attracting IGST");
    let report = validate(b2ba(), &[cbw], &september());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("b2ba.cbw_requires_rchrg")),
        "{:?}",
        report.findings
    );

    let mut de = with(5, "Invoice Type", "Deemed Exp");
    de.cells.insert("Reverse Charge".into(), "Y".into());
    let report = validate(b2ba(), &[de], &september());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("b2ba.de_forbids_rchrg")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/b2ba-sample.csv");
    let rows = gst_core::import::read(&path, b2ba()).expect("reads");
    assert_eq!(rows.len(), 5);

    let json = payload(&rows, &september());
    // Four amendments across two recipients: one pair of rows shares an
    // amendment, so five rows become four records.
    assert_eq!(json.matches(r#""oinum""#).count(), 4, "{json}");
    assert_eq!(json.matches(r#""ctin""#).count(), 2, "{json}");
    // The 65% row scales its tax.
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    assert!(json.contains(r#""iamt":11700"#), "{json}");
}
