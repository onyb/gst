//! End-to-end credit/debit notes to registered persons.
//!
//! Structurally a B2B invoice section — envelope per recipient, notes with line
//! items — but the array key is `nt`, the record fields are note-shaped, and
//! nothing in the table may be negative: a credit note is signalled by its note
//! type, not by a sign.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn cdnr() -> &'static SectionSpec {
    spec::section("cdnr").expect("cdnr is registered")
}

fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(7, 2017).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    }
}

const COLUMNS: [&str; 13] = [
    "GSTIN/UIN of Recipient",
    "Receiver Name",
    "Note Number",
    "Note Date",
    "Note Type",
    "Place Of Supply",
    "Reverse Charge",
    "Note Supply Type",
    "Note Value",
    "Applicable % of Tax Rate",
    "Rate",
    "Taxable Value",
    "Cess Amount",
];

fn base(sheet_row: usize) -> Row {
    Row::from_pairs(
        sheet_row,
        COLUMNS.into_iter().zip([
            "12GEOPS0823BBZH",
            "Acme Traders",
            "CN-001",
            "14-Jul-17",
            "C",
            "37-Andhra Pradesh",
            "N",
            "Regular B2B",
            "59000",
            "",
            "18",
            "50000",
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
    let report = validate(cdnr(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(cdnr(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(cdnr()).is_empty());
}

#[test]
fn a_note_nests_under_an_nt_array_not_an_inv_array() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"ctin":"12GEOPS0823BBZH","cname":"Acme Traders","nt":[{"nt_num":"CN-001","nt_dt":"14-07-2017","ntty":"C","val":59000,"pos":"37","diff_percent":1,"rchrg":"N","inv_typ":"R","itms":[{"num":1801,"itm_det":{"txval":50000,"rt":18,"iamt":9000,"csamt":0}}]}]}]"#
    );
}

#[test]
fn both_note_types_are_accepted_and_nothing_else() {
    for ntty in ["C", "D"] {
        assert!(validate(cdnr(), &[with(5, "Note Type", ntty)], &ctx()).is_clean());
    }
    for bad in ["Credit", "c", "X"] {
        let report = validate(cdnr(), &[with(5, "Note Type", bad)], &ctx());
        assert!(
            report
                .errors()
                .any(|f| f.column.as_deref() == Some("Note Type")),
            "{bad}: {:?}",
            report.findings
        );
    }
}

#[test]
fn taxable_value_must_be_strictly_positive() {
    let zero = with(5, "Taxable Value", "0");
    let report = validate(cdnr(), &[zero], &ctx());
    assert!(
        report.errors().any(|f| f.message.contains("more than 0")),
        "{:?}",
        report.findings
    );
    // Negatives are rejected by the pattern rather than the minimum.
    let neg = with(5, "Taxable Value", "-50000");
    assert!(!validate(cdnr(), &[neg], &ctx()).is_clean());
}

#[test]
fn nothing_in_the_table_may_be_negative() {
    for column in ["Note Value", "Cess Amount"] {
        let report = validate(cdnr(), &[with(5, column, "-100")], &ctx());
        assert!(
            report.errors().any(|f| f.column.as_deref() == Some(column)),
            "{column}: {:?}",
            report.findings
        );
    }
}

#[test]
fn reverse_charge_is_mandatory_with_no_default() {
    // The B2B section defaults a blank to 'N'; this one rejects it.
    let report = validate(cdnr(), &[with(5, "Reverse Charge", "")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Reverse Charge")),
        "{:?}",
        report.findings
    );
}

#[test]
fn note_supply_type_reuses_the_invoice_type_codes() {
    let json = payload(&[with(5, "Note Supply Type", "Deemed Exp")], &ctx());
    assert!(json.contains(r#""inv_typ":"DE""#), "{json}");

    // And the same cross-field rules apply to it.
    let cbw = with(
        5,
        "Note Supply Type",
        "Intra-State supplies attracting IGST",
    );
    let report = validate(cdnr(), &[cbw], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("cdnr.cbw_requires_rchrg")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_tax_split_follows_the_note_supply_type() {
    // Own state plus a regular supply is intra-state.
    let intra = with(5, "Place Of Supply", "27-Maharashtra");
    let json = payload(&[intra], &ctx());
    assert!(json.contains(r#""camt":4500,"samt":4500"#), "{json}");

    // SEZ without payment zeroes the tax.
    let sewop = with(5, "Note Supply Type", "SEZ supplies without payment");
    let json = payload(&[sewop], &ctx());
    assert!(json.contains(r#""iamt":0"#), "{json}");
}

#[test]
fn rows_sharing_a_note_must_agree_on_the_receiver_name() {
    // The reference compares the name for notes, unlike for B2B invoices.
    let mut second = with(6, "Rate", "5");
    second.cells.insert("Taxable Value".into(), "10000".into());
    second
        .cells
        .insert("Receiver Name".into(), "Someone Else".into());

    let report = validate(cdnr(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(cdnr(), &report.records, &ctx());
    assert!(
        out.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("grouping.field_disagreement")),
        "{:?}",
        out.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/cdnr-sample.csv");
    let rows = gst_core::import::read(&path, cdnr()).expect("reads");
    assert_eq!(rows.len(), 4);

    let json = payload(&rows, &ctx());
    assert_eq!(json.matches(r#""ctin""#).count(), 2, "{json}");
    assert_eq!(json.matches(r#""nt_num""#).count(), 3, "{json}");
    // Row 2 carries "1,18,000" — two separators. The reference would read 118.
    assert!(json.contains(r#""val":118000"#), "{json}");
}
