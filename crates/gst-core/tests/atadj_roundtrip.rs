//! End-to-end tax on advances adjusted.
//!
//! Same record and item shape as the advances-received table, which the `at`
//! suite covers. What differs is the amount column and, more importantly, the
//! treatment of a blank cess: validation defaults one to 0 for `at` but NOT
//! here, so a blank cess produces no `csamt` key at all. Amounts are also
//! unsigned here, and a zero adjustment is permitted.

mod common;

use gst_core::record::Row;
use gst_core::spec::SectionSpec;
use gst_core::validate::{FilingContext, validate};

fn atadj() -> &'static SectionSpec {
    common::sec("atadj")
}

fn ctx() -> FilingContext {
    common::ctx(7, 2017)
}

fn base(sheet_row: usize) -> Row {
    common::row(
        atadj(),
        sheet_row,
        &["37-Andhra Pradesh", "", "18", "30000", ""],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    common::payload(atadj(), rows, c)
}

#[test]
fn an_adjustment_maps_the_same_way_as_an_advance() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"pos":"37","sply_ty":"INTER","itms":[{"rt":18,"ad_amt":30000,"iamt":5400}]}]"#
    );
}

#[test]
fn a_blank_cess_emits_no_csamt_key_at_all() {
    // The distinction from `at`: validation defaults a blank cess to 0 there
    // but not here, so the blank survives to the mapping and is dropped.
    let json = payload(&[base(5)], &ctx());
    assert!(!json.contains("csamt"), "{json}");
    assert!(!json.contains("null"), "{json}");

    // A cess that is present is emitted normally, including an explicit zero.
    let json = payload(&[with(5, "Cess Amount", "2300")], &ctx());
    assert!(json.contains(r#""csamt":2300"#), "{json}");
    let json = payload(&[with(5, "Cess Amount", "0")], &ctx());
    assert!(json.contains(r#""csamt":0"#), "{json}");
}

#[test]
fn the_suppliers_own_state_takes_the_central_state_split() {
    let json = payload(&[with(5, "Place Of Supply", "27-Maharashtra")], &ctx());
    assert!(json.contains(r#""camt":2700"#), "{json}");
    assert!(json.contains(r#""samt":2700"#), "{json}");
    assert!(!json.contains(r#""iamt""#), "{json}");
}

#[test]
fn amounts_are_unsigned_here_unlike_the_advances_received_table() {
    for column in ["Gross Advance Adjusted", "Cess Amount"] {
        let report = validate(atadj(), &[with(5, column, "-100")], &ctx());
        assert!(
            report.errors().any(|f| f.column.as_deref() == Some(column)),
            "{column} should reject a negative: {:?}",
            report.findings
        );
    }
}

#[test]
fn a_zero_adjustment_is_accepted() {
    // `at` rejects a zero advance; this table has no such check.
    let json = payload(&[with(5, "Gross Advance Adjusted", "0")], &ctx());
    assert!(json.contains(r#""ad_amt":0"#), "{json}");
}

#[test]
fn the_csv_and_excel_disagree_on_column_order_but_both_import() {
    // Columns are matched by header text, so the swapped pair reads correctly.
    let spec = atadj();
    let by_order: Vec<&str> = {
        let mut f: Vec<_> = spec.fields.iter().collect();
        f.sort_by_key(|x| x.order);
        f.iter().map(|x| x.column.as_str()).collect()
    };
    assert_eq!(by_order[1], "Applicable % of Tax Rate");
    assert_eq!(by_order[2], "Rate");

    // The shipped CSV has them the other way round and still reads.
    let path = common::repo_path("fixtures/gstr1/atadj-sample.csv");
    let rows = gst_core::import::read(&path, spec).expect("reads");
    assert_eq!(rows[0].cells.get("Rate").map(String::as_str), Some("18"));
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = common::repo_path("fixtures/gstr1/atadj-sample.csv");
    let rows = gst_core::import::read(&path, atadj()).expect("reads");
    assert_eq!(rows.len(), 2);

    let json = payload(&rows, &ctx());
    assert_eq!(json.matches(r#""pos""#).count(), 2, "{json}");
    // The first row has a cess, the second leaves it blank.
    assert_eq!(json.matches("csamt").count(), 1, "{json}");
}
