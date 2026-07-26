//! End-to-end amended B2C(Large).
//!
//! Combines the amendment keys of the amended B2B section with the
//! place-of-supply grouping and period-dependent threshold of B2C(Large) — and
//! adds one wrinkle of its own: the only place of supply it carries is the
//! ORIGINAL one, which is what both the grouping and the tax split read.

mod common;

use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn b2cla() -> &'static SectionSpec {
    common::sec("b2cla")
}

/// A Maharashtra (27) supplier amending, in September 2017, invoices first
/// reported in July.
fn ctx(month: u32, year: i32) -> FilingContext {
    common::ctx(month, year)
}

fn base(sheet_row: usize) -> Row {
    common::row(
        b2cla(),
        sheet_row,
        &[
            "INV-L001",
            "14-Jul-17",
            "37-Andhra Pradesh",
            "INV-L001-R",
            "05-Sep-17",
            "295000",
            "",
            "18",
            "250000",
            "",
            "",
        ],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    common::payload(b2cla(), rows, c)
}

#[test]
fn an_amendment_groups_by_original_place_of_supply() {
    assert_eq!(
        payload(&[base(5)], &ctx(9, 2017)),
        r#"[{"pos":"37","inv":[{"oinum":"INV-L001","oidt":"14-07-2017","inum":"INV-L001-R","idt":"05-09-2017","val":295000,"itms":[{"num":1801,"itm_det":{"txval":250000,"rt":18,"iamt":45000}}]}]}]"#
    );
}

#[test]
fn the_original_place_of_supply_drives_the_interstate_check() {
    // There is no revised place of supply to fall back on, so the original
    // value is what must differ from the supplier's own state.
    let own = with(5, "Original Place Of Supply", "27-Maharashtra");
    let report = validate(b2cla(), &[own], &ctx(9, 2017));
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("inter-state supplies only")),
        "{:?}",
        report.findings
    );
    // The finding points at the original column, not a generic 'pos'.
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Original Place Of Supply")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_original_invoice_may_predate_the_return_period() {
    let json = payload(&[base(5)], &ctx(9, 2017));
    assert!(json.contains(r#""oidt":"14-07-2017""#), "{json}");

    let too_early = with(5, "Original Invoice date", "30-Jun-17");
    let report = validate(b2cla(), &[too_early], &ctx(9, 2017));
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("before 1 July 2017")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_threshold_is_judged_against_the_filed_period_not_the_original() {
    // An amendment filed in August 2024 to a July-2017 invoice is measured
    // against the newer 1,00,000 bar, not the 2,50,000 one in force when the
    // original was reported.
    let mut r = with(5, "Invoice Value", "150000");
    r.cells.insert("Taxable Value".into(), "120000".into());
    r.cells
        .insert("Revised Invoice date".into(), "05-Aug-24".into());
    assert!(validate(b2cla(), &[r.clone()], &ctx(8, 2024)).is_clean());

    // Same row filed a month earlier fails the older threshold.
    r.cells
        .insert("Revised Invoice date".into(), "05-Jul-24".into());
    let before = validate(b2cla(), &[r], &ctx(7, 2024));
    assert!(
        before
            .errors()
            .any(|f| f.message.contains("more than 250000")),
        "{:?}",
        before.findings
    );
}

#[test]
fn both_invoice_numbers_reject_a_numerically_zero_value() {
    for column in ["Original Invoice Number", "Revised Invoice Number"] {
        let report = validate(b2cla(), &[with(5, column, "0")], &ctx(9, 2017));
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
    let r = with(5, "Revised Invoice Number", "INV-L001");
    let json = payload(&[r], &ctx(9, 2017));
    assert!(json.contains(r#""oinum":"INV-L001""#), "{json}");
    assert!(json.contains(r#""inum":"INV-L001""#), "{json}");
}

#[test]
fn rows_grouping_as_one_amendment_must_agree_on_the_original_invoice() {
    let mut second = base(6);
    second.cells.insert("Rate".into(), "5".into());
    second.cells.insert("Taxable Value".into(), "45000".into());
    second
        .cells
        .insert("Original Invoice Number".into(), "INV-L999".into());

    let report = validate(b2cla(), &[base(5), second], &ctx(9, 2017));
    assert!(report.is_clean(), "{:?}", report.findings);

    let out = generate(b2cla(), &report.records, &ctx(9, 2017));
    let finding = out
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("grouping.field_disagreement"))
        .expect("disagreement is reported");
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.column.as_deref(), Some("Original Invoice Number"));
}

#[test]
fn tax_is_always_integrated_never_split() {
    for (pos, is_sez) in [
        ("37-Andhra Pradesh", false),
        ("29-Karnataka", false),
        ("27-Maharashtra", true),
    ] {
        let mut c = ctx(9, 2017);
        c.is_sez = is_sez;
        let json = payload(&[with(5, "Original Place Of Supply", pos)], &c);
        assert!(json.contains(r#""iamt""#), "{pos}: {json}");
        assert!(!json.contains(r#""camt""#), "{pos}: {json}");
    }
}

#[test]
fn a_blank_cess_emits_no_csamt_key_at_all() {
    // Verified against a file captured from the tool: these two tables compute
    // cess without the empty-cell guard the B2B tables use, so a blank cell
    // becomes NaN, the working file records null, and omit-empty drops the key.
    let json = payload(&[base(5)], &ctx(9, 2017));
    assert!(!json.contains("csamt"), "{json}");
    assert!(!json.contains("null"), "{json}");

    // An explicit 0 in the cell is a real zero and is emitted.
    let json = payload(&[with(5, "Cess Amount", "0")], &ctx(9, 2017));
    assert!(json.contains(r#""csamt":0"#), "{json}");
}

#[test]
fn an_ecommerce_gstin_is_rejected_despite_the_template_column() {
    let report = validate(
        b2cla(),
        &[with(5, "E-Commerce GSTIN", "12AJIPA1572E1C7")],
        &ctx(9, 2017),
    );
    assert!(
        report.errors().any(|f| f.message.contains("must be blank")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = common::repo_path("fixtures/gstr1/b2cla-sample.csv");
    let rows = gst_core::import::read(&path, b2cla()).expect("reads");
    assert_eq!(rows.len(), 5);

    let json = payload(&rows, &ctx(9, 2017));
    // Three original places of supply; one amendment spans two rate rows.
    assert_eq!(json.matches(r#""pos""#).count(), 3, "{json}");
    assert_eq!(json.matches(r#""oinum""#).count(), 4, "{json}");
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    assert!(json.contains(r#""csamt":1500"#), "{json}");
}
