//! End-to-end B2B: workbook rows in, portal upload JSON out.
//!
//! These exercise the whole spec-driven path — load `spec/gstr1/b2b.json`,
//! validate rows against it, group them, and assemble the payload — so a
//! change to either the spec or the engine that alters generated output shows
//! up here.

mod common;

use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn b2b() -> &'static SectionSpec {
    common::sec("b2b")
}

/// A Maharashtra (27) supplier filing for July 2017.
fn maharashtra() -> FilingContext {
    common::ctx(7, 2017)
}

/// A clean single-rate inter-state invoice, as a base for targeted mutation.
fn base(sheet_row: usize) -> Row {
    common::row(
        b2b(),
        sheet_row,
        &[
            "12GEOPS0823BBZH",
            "Acme Traders",
            "INV-001",
            "14-Jul-17",
            "50000",
            "37-Andhra Pradesh",
            "N",
            "",
            "Regular B2B",
            "",
            "18",
            "45000",
            "",
        ],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

/// Validate then generate, asserting validation was clean.
fn payload(rows: &[Row], ctx: &FilingContext) -> String {
    common::payload(b2b(), rows, ctx)
}

#[test]
fn a_single_interstate_invoice_produces_the_expected_payload() {
    let rows = [base(5)];

    // Supplier is 27, place of supply is 37, so this is inter-state: integrated
    // tax at the full rate, no central or state tax, and the empty e-commerce
    // GSTIN is omitted rather than emitted blank.
    assert_eq!(
        payload(&rows, &maharashtra()),
        r#"[{"ctin":"12GEOPS0823BBZH","inv":[{"inum":"INV-001","idt":"14-07-2017","val":50000,"pos":"37","rchrg":"N","inv_typ":"R","itms":[{"num":1801,"itm_det":{"txval":45000,"rt":18,"iamt":8100,"csamt":0}}]}]}]"#
    );
}

/// The row shared by the intra-state tests: the supplier's own state as both
/// recipient state and place of supply.
fn local_buyer(sheet_row: usize) -> Row {
    base(sheet_row)
        .with_cell("GSTIN/UIN of Recipient", "27AAPFU0939F1ZV")
        .with_cell("Receiver Name", "Local Buyer")
        .with_cell("Invoice Number", "INV-002")
        .with_cell("Place Of Supply", "27-Maharashtra")
}

#[test]
fn an_intrastate_invoice_splits_tax_into_central_and_state() {
    let rows = [local_buyer(5)];

    let json = payload(&rows, &maharashtra());
    // Half the rate each, and no integrated tax key at all.
    assert!(json.contains(r#""camt":4050,"samt":4050"#), "{json}");
    assert!(!json.contains("iamt"), "{json}");
}

#[test]
fn the_same_row_is_interstate_for_a_supplier_in_another_state() {
    // The split turns on the supplier's own state, so identical workbook rows
    // legitimately generate different payloads for different filers.
    let rows = [local_buyer(5)];

    let andhra = FilingContext {
        supplier_gstin: "37AAPFU0939F1ZV".into(),
        ..maharashtra()
    };
    let json = payload(&rows, &andhra);
    assert!(json.contains(r#""iamt":8100"#), "{json}");
    assert!(!json.contains("camt"), "{json}");
}

#[test]
fn an_sez_supplier_is_always_interstate() {
    let rows = [local_buyer(5)];

    let sez = FilingContext {
        is_sez: true,
        ..maharashtra()
    };
    let json = payload(&rows, &sez);
    assert!(json.contains(r#""iamt":8100"#), "{json}");
    assert!(!json.contains("camt"), "{json}");
}

#[test]
fn sez_without_payment_of_tax_carries_zero_tax_and_zero_cess() {
    let rows = [with(5, "Receiver Name", "SEZ Unit")
        .with_cell("Invoice Number", "INV-003")
        .with_cell("Invoice Type", "SEZ supplies without payment")
        .with_cell("Cess Amount", "900")];

    let json = payload(&rows, &maharashtra());
    assert!(json.contains(r#""inv_typ":"SEWOP""#), "{json}");
    // Cess is zeroed even though the workbook supplied 900.
    assert!(json.contains(r#""iamt":0,"csamt":0"#), "{json}");
}

#[test]
fn a_multi_rate_invoice_becomes_one_invoice_with_several_items() {
    let rows = [
        with(5, "Invoice Number", "INV-004").with_cell("Invoice Value", "100000"),
        with(6, "Invoice Number", "INV-004")
            .with_cell("Invoice Value", "100000")
            .with_cell("Rate", "5")
            .with_cell("Taxable Value", "40000"),
    ];

    let json = payload(&rows, &maharashtra());
    // One envelope, one invoice, two items — numbered from their rates.
    assert_eq!(json.matches(r#""inum":"INV-004""#).count(), 1);
    assert!(json.contains(r#""num":1801"#), "{json}");
    assert!(json.contains(r#""num":501"#), "{json}");
    assert!(json.contains(r#""iamt":8100"#), "{json}");
    assert!(json.contains(r#""iamt":2000"#), "{json}");
}

#[test]
fn invoice_numbers_group_case_insensitively() {
    let rows = [
        with(5, "Invoice Number", "inv-005").with_cell("Invoice Value", "100000"),
        with(6, "Invoice Number", "INV-005")
            .with_cell("Invoice Value", "100000")
            .with_cell("Rate", "5")
            .with_cell("Taxable Value", "40000"),
    ];

    let json = payload(&rows, &maharashtra());
    // Both rows land on one invoice; the first spelling seen is the one kept.
    assert_eq!(json.matches(r#""itm_det""#).count(), 2);
    assert!(json.contains(r#""inum":"inv-005""#), "{json}");
}

#[test]
fn separate_recipients_become_separate_envelopes() {
    let rows = [
        with(5, "Invoice Number", "INV-006"),
        common::row(
            b2b(),
            6,
            &[
                "29AAGCB7383J1Z4",
                "Other Buyer",
                "INV-007",
                "15-Jul-17",
                "20000",
                "29-Karnataka",
                "N",
                "",
                "Regular B2B",
                "",
                "5",
                "18000",
                "",
            ],
        ),
    ];

    let report = validate(b2b(), &rows, &maharashtra());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(b2b(), &report.records, &maharashtra());
    assert_eq!(out.envelopes.len(), 2);
    assert!(out.to_json().contains(r#""ctin":"29AAGCB7383J1Z4""#));
}

#[test]
fn applicable_percent_scales_every_computed_amount() {
    let rows = [with(5, "Invoice Number", "INV-008").with_cell("Applicable % of Tax Rate", "65")];

    let json = payload(&rows, &maharashtra());
    // 45000 * 18% * 0.65 = 5265, and the factor itself is emitted, not the
    // percent the workbook carried.
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    assert!(json.contains(r#""iamt":5265"#), "{json}");
}

#[test]
fn cess_is_carried_through_and_rounded() {
    let rows = [with(5, "Invoice Number", "INV-009").with_cell("Cess Amount", "756.50")];

    assert!(
        payload(&rows, &maharashtra()).contains(r#""csamt":756.5"#),
        "cess should survive to the payload"
    );
}

#[test]
fn a_duplicate_rate_replaces_the_earlier_line_and_warns() {
    // Reproduces the reference implementation's silent last-wins merge, but
    // surfaces it: the 45000 line is lost, not added to.
    let rows = [
        with(5, "Invoice Number", "INV-010").with_cell("Invoice Value", "100000"),
        with(6, "Invoice Number", "INV-010")
            .with_cell("Invoice Value", "100000")
            .with_cell("Taxable Value", "30000"),
    ];

    let report = validate(b2b(), &rows, &maharashtra());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(b2b(), &report.records, &maharashtra());

    let json = out.to_json();
    assert_eq!(json.matches(r#""itm_det""#).count(), 1, "{json}");
    assert!(json.contains(r#""txval":30000"#), "{json}");
    assert!(!json.contains("45000"), "the earlier line is gone: {json}");

    // No error, but the loss is reported.
    assert!(!out.findings.iter().any(|f| f.severity == Severity::Error));
    let warning = out
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("grouping.item_replaced"))
        .expect("replacement is reported");
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.sheet_row, 6);
    assert!(warning.message.contains("row 5"), "{}", warning.message);
}

#[test]
fn rows_of_one_invoice_that_disagree_are_rejected() {
    // Same invoice number and recipient, contradictory invoice value.
    let rows = [
        with(5, "Invoice Number", "INV-011").with_cell("Invoice Value", "100000"),
        with(6, "Invoice Number", "INV-011")
            .with_cell("Invoice Value", "999999")
            .with_cell("Rate", "5")
            .with_cell("Taxable Value", "40000"),
    ];

    let report = validate(b2b(), &rows, &maharashtra());
    assert!(
        report.is_clean(),
        "field validation passes: {:?}",
        report.findings
    );

    let out = generate(b2b(), &report.records, &maharashtra());
    let finding = out
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("grouping.field_disagreement"))
        .expect("disagreement is reported");
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.sheet_row, 6);
    assert_eq!(finding.column.as_deref(), Some("Invoice Value"));

    // The conflicting row is dropped; the first one still generates.
    let json = out.to_json();
    assert!(json.contains(r#""val":100000"#), "{json}");
    assert_eq!(json.matches(r#""itm_det""#).count(), 1, "{json}");
}

#[test]
fn a_row_failing_validation_never_reaches_the_payload() {
    let rows = [
        with(5, "Invoice Number", "INV-012"),
        // Reverse charge 'N' contradicts the invoice type.
        with(6, "Invoice Number", "INV-013")
            .with_cell("Place Of Supply", "27-Maharashtra")
            .with_cell("Invoice Type", "Intra-State supplies attracting IGST"),
    ];

    let report = validate(b2b(), &rows, &maharashtra());
    assert!(!report.is_clean());
    assert_eq!(report.records.len(), 1);

    let json = generate(b2b(), &report.records, &maharashtra()).to_json();
    assert!(json.contains("INV-012"), "{json}");
    assert!(!json.contains("INV-013"), "{json}");
}

#[test]
fn generated_output_is_byte_stable_across_runs() {
    // Key order comes from the spec, and grouping preserves first-seen order,
    // so the same input must serialize identically every time — the property
    // differential testing against reference output depends on.
    let rows = [
        with(5, "Invoice Number", "INV-014").with_cell("Invoice Value", "100000"),
        with(6, "Invoice Number", "INV-014")
            .with_cell("Invoice Value", "100000")
            .with_cell("Rate", "5")
            .with_cell("Taxable Value", "40000"),
    ];

    let first = payload(&rows, &maharashtra());
    for _ in 0..5 {
        assert_eq!(payload(&rows, &maharashtra()), first);
    }
}
