//! End-to-end for the two tables every GSTR-1 must carry: documents issued and
//! the HSN summary.
//!
//! Between them they add three firsts: a table with no money in it at all, a
//! positional payload number driven by a master's ORDER, and tax that the filer
//! ENTERS rather than the engine deriving.

mod common;

use gst_core::record::Row;
use gst_core::spec::SectionSpec;
use gst_core::validate::{FilingContext, validate};

fn section(code: &str) -> &'static SectionSpec {
    common::sec(code)
}

fn ctx(month: u32, year: i32, aato_over_5cr: bool) -> FilingContext {
    FilingContext {
        aato_over_5cr,
        ..common::ctx(month, year)
    }
}

fn payload(spec: &SectionSpec, rows: &[Row], c: &FilingContext) -> String {
    common::payload(spec, rows, c)
}

// ---------------------------------------------------------------- doc_issue

fn doc_row(sheet_row: usize, values: [&str; 5]) -> Row {
    common::row(section("doc_issue"), sheet_row, &values)
}

fn doc_base(sheet_row: usize) -> Row {
    doc_row(
        sheet_row,
        [
            "Invoices for outward supply",
            "INV-001",
            "INV-100",
            "100",
            "2",
        ],
    )
}

#[test]
fn documents_issued_needs_no_money_at_all() {
    let spec = section("doc_issue");
    // No rate, no taxable value, no tax anywhere in the field set.
    for forbidden in ["rt", "txval", "iamt", "camt", "samt", "csamt"] {
        assert!(
            spec.field(forbidden).is_none(),
            "doc_issue should not have {forbidden}"
        );
    }
    assert!(gst_core::generate::unimplemented_derivations(spec).is_empty());
}

#[test]
fn the_document_number_is_the_masters_position() {
    let spec = section("doc_issue");
    // 'Invoices for outward supply' is first, 'Credit Note' fifth.
    let json = payload(spec, &[doc_base(5)], &ctx(7, 2017, false));
    assert!(json.contains(r#""doc_num":1"#), "{json}");

    let credit = doc_row(5, ["Credit Note", "CN-001", "CN-010", "10", "1"]);
    let json = payload(spec, &[credit], &ctx(7, 2017, false));
    assert!(json.contains(r#""doc_num":5"#), "{json}");
}

#[test]
fn net_issue_is_derived_not_entered() {
    let json = payload(section("doc_issue"), &[doc_base(5)], &ctx(7, 2017, false));
    assert!(
        json.contains(r#""totnum":100,"cancel":2,"net_issue":98"#),
        "{json}"
    );
}

#[test]
fn cancelled_cannot_exceed_the_total() {
    let bad = doc_row(5, ["Debit Note", "DN-001", "DN-005", "5", "6"]);
    let report = validate(section("doc_issue"), &[bad], &ctx(7, 2017, false));
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("doc_issue.cancelled_within_total")),
        "{:?}",
        report.findings
    );

    // Equal is allowed: a whole range may be cancelled.
    let all = doc_row(5, ["Debit Note", "DN-001", "DN-005", "5", "5"]);
    assert!(validate(section("doc_issue"), &[all], &ctx(7, 2017, false)).is_clean());
}

#[test]
fn an_all_zero_serial_range_is_rejected() {
    let bad = doc_row(5, ["Debit Note", "0", "00", "5", "0"]);
    let report = validate(section("doc_issue"), &[bad], &ctx(7, 2017, false));
    assert!(!report.is_clean(), "{:?}", report.findings);
    // But a zero with something else in it is fine.
    let ok = doc_row(5, ["Debit Note", "0A", "A1", "5", "0"]);
    assert!(validate(section("doc_issue"), &[ok], &ctx(7, 2017, false)).is_clean());
}

#[test]
fn several_ranges_for_one_nature_collapse_into_one_record() {
    let second = doc_row(
        6,
        [
            "Invoices for outward supply",
            "INV-201",
            "INV-250",
            "50",
            "0",
        ],
    );
    let json = payload(
        section("doc_issue"),
        &[doc_base(5), second],
        &ctx(7, 2017, false),
    );
    assert_eq!(json.matches(r#""doc_num""#).count(), 1, "{json}");
    assert_eq!(json.matches(r#""net_issue""#).count(), 2, "{json}");
    // Ranges are numbered within their nature of document.
    assert!(
        json.contains(r#""num":1"#) && json.contains(r#""num":2"#),
        "{json}"
    );
}

// ------------------------------------------------------------------- hsn

fn hsn_row(sheet_row: usize, values: [&str; 11]) -> Row {
    common::row(section("hsn(b2b)"), sheet_row, &values)
}

fn hsn_base(sheet_row: usize) -> Row {
    hsn_row(
        sheet_row,
        [
            "0101",
            "Live horses",
            "NOS-NUMBERS",
            "10",
            "118000",
            "18",
            "100000",
            "18000",
            "",
            "",
            "",
        ],
    )
}

#[test]
fn both_hsn_halves_are_registered_and_identical_in_shape() {
    let b2b = section("hsn(b2b)");
    let b2c = section("hsn(b2c)");
    assert_eq!(b2b.columns(), b2c.columns());
    assert!(b2b.is_flat() && b2c.is_flat());
    assert!(gst_core::generate::unimplemented_derivations(b2b).is_empty());
    assert!(gst_core::generate::unimplemented_derivations(b2c).is_empty());
}

#[test]
fn the_description_is_looked_up_from_the_code_table() {
    let json = payload(section("hsn(b2b)"), &[hsn_base(5)], &ctx(6, 2025, false));
    // The filer's own wording is kept, and the official one is added.
    assert!(json.contains(r#""user_desc":"Live horses""#), "{json}");
    assert!(
        json.contains(r#""desc":"LIVE HORSES, ASSES, MULES AND HINNIES.""#),
        "{json}"
    );
}

#[test]
fn an_unknown_hsn_code_is_rejected() {
    // Not in the table, so the reference would blank it and then fail.
    let bad = hsn_row(
        5,
        [
            // 9999 is a real code; 12345678 is not.
            "12345678",
            "Nothing",
            "NOS-NUMBERS",
            "1",
            "100",
            "18",
            "100",
            "18",
            "",
            "",
            "",
        ],
    );
    let report = validate(section("hsn(b2b)"), &[bad], &ctx(6, 2025, false));
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("not a code in the HSN/SAC table")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_unit_code_is_reduced_to_its_prefix() {
    let json = payload(section("hsn(b2b)"), &[hsn_base(5)], &ctx(6, 2025, false));
    assert!(json.contains(r#""uqc":"NOS""#), "{json}");
    assert!(!json.contains("NUMBERS"), "{json}");
}

#[test]
fn the_code_length_follows_the_turnover_band() {
    let spec = section("hsn(b2b)");
    // Four digits is fine below the threshold.
    assert!(validate(spec, &[hsn_base(5)], &ctx(6, 2025, false)).is_clean());

    // Above it, six digits are required.
    let report = validate(spec, &[hsn_base(5)], &ctx(6, 2025, true));
    assert!(
        report.errors().any(|f| f.message.contains("6 to 8 digits")),
        "{:?}",
        report.findings
    );

    // A six-digit code satisfies both bands.
    let six = hsn_row(
        5,
        [
            "010121",
            "Breeding animals",
            "NOS-NUMBERS",
            "1",
            "100",
            "18",
            "100",
            "18",
            "",
            "",
            "",
        ],
    );
    assert!(validate(spec, std::slice::from_ref(&six), &ctx(6, 2025, true)).is_clean());
    assert!(validate(spec, &[six], &ctx(6, 2025, false)).is_clean());
}

#[test]
fn tax_must_be_stated_one_way_or_the_other() {
    let spec = section("hsn(b2b)");
    // Neither integrated nor the central/state pair.
    let none = hsn_row(
        5,
        [
            "0101",
            "Live horses",
            "NOS-NUMBERS",
            "10",
            "118000",
            "18",
            "100000",
            "",
            "",
            "",
            "",
        ],
    );
    let report = validate(spec, &[none], &ctx(6, 2025, false));
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("hsn(b2b).tax_must_be_stated")),
        "{:?}",
        report.findings
    );

    // Central and state together are accepted in place of integrated.
    let split = hsn_row(
        5,
        [
            "1006",
            "Rice",
            "KGS-KILOGRAMS",
            "1000",
            "105000",
            "5",
            "100000",
            "",
            "2500",
            "2500",
            "",
        ],
    );
    assert!(validate(spec, &[split], &ctx(6, 2025, false)).is_clean());
}

#[test]
fn tax_is_taken_as_entered_and_never_recomputed() {
    // 100000 at 18% would be 18000; a filer stating 999 gets 999 through.
    let odd = hsn_row(
        5,
        [
            "0101",
            "Live horses",
            "NOS-NUMBERS",
            "10",
            "118000",
            "18",
            "100000",
            "999",
            "",
            "",
            "",
        ],
    );
    let json = payload(section("hsn(b2b)"), &[odd], &ctx(6, 2025, false));
    assert!(json.contains(r#""iamt":999"#), "{json}");
}

#[test]
fn total_value_is_read_but_never_emitted() {
    let json = payload(section("hsn(b2b)"), &[hsn_base(5)], &ctx(6, 2025, false));
    // The template column exists and validates, but no payload key carries it.
    assert!(section("hsn(b2b)").field("total_value").is_some());
    assert!(!json.contains("118000"), "{json}");
}

#[test]
fn records_carry_a_real_serial_not_the_references_constant_one() {
    let second = hsn_base(6).with_cell("HSN", "0102");
    let json = payload(
        section("hsn(b2b)"),
        &[hsn_base(5), second],
        &ctx(6, 2025, false),
    );
    assert!(json.contains(r#""num":1"#), "{json}");
    assert!(json.contains(r#""num":2"#), "{json}");
}

#[test]
fn the_shipped_fixtures_validate_and_generate() {
    for (code, file) in [
        ("doc_issue", "docs-sample.csv"),
        ("hsn(b2b)", "hsn-b2b-sample.csv"),
        ("hsn(b2c)", "hsn-b2c-sample.csv"),
    ] {
        let path = common::repo_path("fixtures/gstr1").join(file);
        let spec = section(code);
        let rows = gst_core::import::read(&path, spec).expect("reads");
        assert!(!rows.is_empty(), "{file} has rows");
        let json = payload(spec, &rows, &ctx(6, 2025, false));
        assert!(json.len() > 2, "{code} generated something");
    }
}
