//! End-to-end amended credit/debit notes to registered persons.
//!
//! The unamended section plus original-note keys. Covers only what differs;
//! the shared note behaviour is exercised by the cdnr suite.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn cdnra() -> &'static SectionSpec {
    spec::section("cdnra").expect("cdnra is registered")
}

fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(9, 2017).unwrap(),
        is_sez: false,
    }
}

const COLUMNS: [&str; 15] = [
    "GSTIN/UIN of Recipient",
    "Receiver Name",
    "Original Note Number",
    "Original Note Date",
    "Revised Note Number",
    "Revised Note Date",
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
            "CN-001-R",
            "05-Sep-17",
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
    let report = validate(cdnra(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(cdnra(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(cdnra()).is_empty());
}

#[test]
fn an_amendment_carries_both_note_identities() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"ctin":"12GEOPS0823BBZH","cname":"Acme Traders","nt":[{"ont_num":"CN-001","ont_dt":"14-07-2017","nt_num":"CN-001-R","nt_dt":"05-09-2017","ntty":"C","val":59000,"pos":"37","diff_percent":1,"rchrg":"N","inv_typ":"R","itms":[{"num":1801,"itm_det":{"txval":50000,"rt":18,"iamt":9000,"csamt":0}}]}]}]"#
    );
}

#[test]
fn the_original_note_may_predate_the_return_period() {
    assert!(payload(&[base(5)], &ctx()).contains(r#""ont_dt":"14-07-2017""#));

    let early = with(5, "Original Note Date", "30-Jun-17");
    let report = validate(cdnra(), &[early], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("before 1 July 2017")),
        "{:?}",
        report.findings
    );
}

#[test]
fn both_note_numbers_reject_a_numerically_zero_value() {
    for column in ["Original Note Number", "Revised Note Number"] {
        let report = validate(cdnra(), &[with(5, column, "0")], &ctx());
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
fn the_revised_number_may_equal_the_original() {
    let same = with(5, "Revised Note Number", "CN-001");
    let json = payload(&[same], &ctx());
    assert!(json.contains(r#""ont_num":"CN-001","ont_dt""#), "{json}");
    assert!(json.contains(r#""nt_num":"CN-001""#), "{json}");
}

#[test]
fn rows_grouping_as_one_amendment_must_agree_on_the_original_note() {
    let mut second = with(6, "Rate", "5");
    second.cells.insert("Taxable Value".into(), "10000".into());
    second
        .cells
        .insert("Original Note Number".into(), "CN-999".into());

    let report = validate(cdnra(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(cdnra(), &report.records, &ctx());
    let finding = out
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("grouping.field_disagreement"))
        .expect("disagreement reported");
    assert_eq!(finding.column.as_deref(), Some("Original Note Number"));
}

#[test]
fn taxable_value_must_be_strictly_positive() {
    let report = validate(cdnra(), &[with(5, "Taxable Value", "0")], &ctx());
    assert!(
        report.errors().any(|f| f.message.contains("more than 0")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/cdnra-sample.csv");
    let rows = gst_core::import::read(&path, cdnra()).expect("reads");
    assert_eq!(rows.len(), 3);

    let json = payload(&rows, &ctx());
    assert_eq!(json.matches(r#""ont_num""#).count(), 3, "{json}");
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
}
