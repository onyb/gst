//! End-to-end B2C(Small): the first FLAT section.
//!
//! No document number, no date, no invoice or line-item level — one payload
//! object per row, carrying its own tax split and an INTRA/INTER discriminator.
//! It is also the first section that permits negative values, and the first
//! where both branches of the tax split are genuinely reachable.

mod common;

use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn b2cs() -> &'static SectionSpec {
    common::sec("b2cs")
}

/// A Maharashtra (27) supplier.
fn ctx() -> FilingContext {
    common::ctx(7, 2017)
}

fn base(sheet_row: usize) -> Row {
    common::row(
        b2cs(),
        sheet_row,
        &["OE", "37-Andhra Pradesh", "", "18", "300000", "", ""],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    common::payload(b2cs(), rows, c)
}

#[test]
fn the_section_is_flat() {
    assert!(b2cs().is_flat(), "b2cs should be a flat section");
}

#[test]
fn a_row_becomes_one_flat_payload_object() {
    // No envelope, no inv array, no itms — and no diff_percent, because an
    // unscaled factor of 1 is dropped.
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"sply_ty":"INTER","rt":18,"typ":"OE","pos":"37","txval":300000,"iamt":54000,"csamt":0}]"#
    );
}

#[test]
fn the_suppliers_own_state_is_intra_state_here() {
    // Unlike B2C(Large), an own-state row is valid and splits into central and
    // state tax — both branches are reachable in this section.
    let own = with(5, "Place Of Supply", "27-Maharashtra");
    assert_eq!(
        payload(&[own], &ctx()),
        r#"[{"sply_ty":"INTRA","rt":18,"typ":"OE","pos":"27","txval":300000,"camt":27000,"samt":27000,"csamt":0}]"#
    );
}

#[test]
fn an_sez_supplier_is_always_interstate() {
    let own = with(5, "Place Of Supply", "27-Maharashtra");
    let sez = FilingContext {
        is_sez: true,
        ..ctx()
    };
    let json = payload(&[own], &sez);
    assert!(json.contains(r#""sply_ty":"INTER""#), "{json}");
    assert!(json.contains(r#""iamt":54000"#), "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
}

#[test]
fn an_unscaled_rate_factor_is_dropped_but_sixty_five_is_kept() {
    let json = payload(&[base(5)], &ctx());
    assert!(!json.contains("diff_percent"), "{json}");

    let scaled = with(5, "Applicable % of Tax Rate", "65");
    let json = payload(&[scaled], &ctx());
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    // 300000 * 18% * 0.65 = 35100
    assert!(json.contains(r#""iamt":35100"#), "{json}");
}

#[test]
fn negative_values_are_permitted_and_carry_through_to_tax() {
    let neg = with(5, "Taxable Value", "-75000");
    let json = payload(&[neg], &ctx());
    assert!(json.contains(r#""txval":-75000"#), "{json}");
    assert!(json.contains(r#""iamt":-13500"#), "{json}");

    // Cess may be negative too.
    let mut both = with(5, "Taxable Value", "-75000");
    both.cells.insert("Cess Amount".into(), "-500".into());
    let json = payload(&[both], &ctx());
    assert!(json.contains(r#""csamt":-500"#), "{json}");
}

#[test]
fn type_e_is_rejected_even_with_a_valid_operator_gstin() {
    // The e-commerce branch is unreachable: Type must match 'OE' exactly, so an
    // 'E' row fails before its operator GSTIN is ever considered.
    let mut e = with(5, "Type", "E");
    e.cells
        .insert("E-Commerce GSTIN".into(), "12AJIPA1572E1C7".into());
    let report = validate(b2cs(), &[e], &ctx());
    assert!(!report.is_clean());
    assert!(
        report.errors().any(|f| f.column.as_deref() == Some("Type")),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_operator_gstin_is_rejected_on_an_oe_row() {
    let report = validate(
        b2cs(),
        &[with(5, "E-Commerce GSTIN", "12AJIPA1572E1C7")],
        &ctx(),
    );
    assert!(
        report.errors().any(|f| f.message.contains("must be blank")),
        "{:?}",
        report.findings
    );
}

/// Two rows agreeing on place of supply, rate, e-commerce GSTIN and applicable
/// percentage COLLAPSE to one, and the later row wins.
///
/// The reference splices the newcomer into the earlier row's slot
/// (offline.js:10705). Nothing is summed, so the earlier row's money is simply
/// gone — which is why this is reported even though it reproduces the
/// reference faithfully.
#[test]
fn rows_sharing_a_collapse_key_keep_only_the_last() {
    let rows = [
        with(5, "Taxable Value", "300000"),
        with(6, "Taxable Value", "111111"),
    ];
    let report = validate(b2cs(), &rows, &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(b2cs(), &report.records, &ctx());

    assert_eq!(out.envelopes.len(), 1, "{:?}", out.to_json());
    let json = out.to_json();
    assert!(json.contains("111111"), "{json}");
    assert!(
        !json.contains("300000"),
        "the earlier row is dropped: {json}"
    );

    // Reproduced, but not silently.
    assert!(
        out.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("grouping.record_replaced")),
        "{:?}",
        out.findings
    );
    // Losing a row is not itself an error — the reference accepts the file.
    assert!(!out.findings.iter().any(|f| f.severity == Severity::Error));
}

/// The supply type is deliberately absent from the collapse key: the reference
/// compares place of supply, rate, operator GSTIN and percentage, and nothing
/// else. Two rows differing only in E versus OE therefore still collapse.
#[test]
fn the_supply_type_is_not_part_of_the_collapse_key() {
    let spec = b2cs();
    let key_fields: Vec<&str> = spec
        .grouping
        .record_key
        .iter()
        .map(|p| p.field.as_str())
        .collect();
    assert_eq!(key_fields, ["pos", "rt", "etin", "diff_percent"]);
    assert!(!key_fields.contains(&"typ"));
}

#[test]
fn one_object_is_emitted_per_row_in_row_order() {
    let rows = [
        with(5, "Place Of Supply", "27-Maharashtra"),
        with(6, "Place Of Supply", "37-Andhra Pradesh"),
        with(7, "Place Of Supply", "29-Karnataka"),
    ];
    let report = validate(b2cs(), &rows, &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(b2cs(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 3);

    let json = out.to_json();
    let first = json.find(r#""pos":"27""#).expect("27 present");
    let second = json.find(r#""pos":"37""#).expect("37 present");
    let third = json.find(r#""pos":"29""#).expect("29 present");
    assert!(
        first < second && second < third,
        "row order preserved: {json}"
    );
}

#[test]
fn a_rate_outside_the_slabs_is_rejected() {
    let report = validate(b2cs(), &[with(5, "Rate", "19")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("must be one of")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    // The fixture is written in the SECTION CSV's column order, which swaps
    // Rate and Applicable % of Tax Rate relative to the Excel template —
    // import matches on header text, so it reads correctly either way.
    let path = common::repo_path("fixtures/gstr1/b2cs-sample.csv");
    let rows = gst_core::import::read(&path, b2cs()).expect("reads");
    assert_eq!(rows.len(), 5);

    let json = payload(&rows, &ctx());
    // One object per row, flat.
    assert_eq!(json.matches(r#""sply_ty""#).count(), 5, "{json}");
    assert_eq!(json.matches(r#""INTRA""#).count(), 1, "{json}");
    assert_eq!(json.matches(r#""INTER""#).count(), 4, "{json}");
    // Only the 65% row carries the factor.
    assert_eq!(json.matches("diff_percent").count(), 1, "{json}");
    assert!(json.contains(r#""txval":-75000"#), "{json}");
}
