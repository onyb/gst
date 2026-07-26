//! End-to-end credit/debit notes to unregistered persons.
//!
//! No registered counterparty, so records sit at the top level of the payload.
//! Tax is always integrated — the reference has no central/state branch for
//! these at all — and UR Type drives three cross-field rules.

mod common;

use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::SectionSpec;
use gst_core::validate::{FilingContext, validate};

fn cdnur() -> &'static SectionSpec {
    common::sec("cdnur")
}

fn ctx() -> FilingContext {
    common::ctx(7, 2017)
}

fn base(sheet_row: usize) -> Row {
    common::row(
        cdnur(),
        sheet_row,
        &[
            "B2CL",
            "UN-001",
            "14-Jul-17",
            "C",
            "37-Andhra Pradesh",
            "295000",
            "",
            "18",
            "250000",
            "",
        ],
    )
}

/// An export note: no place of supply.
fn export(sheet_row: usize, ur_type: &str) -> Row {
    base(sheet_row)
        .with_cell("UR Type", ur_type)
        .with_cell("Place Of Supply", "")
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    base(sheet_row).with_cell(column, value)
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    common::payload(cdnur(), rows, c)
}

#[test]
fn a_record_sits_at_the_top_level_with_no_counterparty() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"nt_num":"UN-001","nt_dt":"14-07-2017","ntty":"C","val":295000,"typ":"B2CL","pos":"37","itms":[{"num":1801,"itm_det":{"txval":250000,"rt":18,"iamt":45000,"csamt":0}}]}]"#
    );
}

#[test]
fn tax_is_always_integrated_even_for_the_suppliers_own_state() {
    // There is no central/state code path for unregistered notes at all.
    let own = with(5, "Place Of Supply", "27-Maharashtra");
    let report = validate(cdnur(), &[own], &ctx());
    // Own state is rejected for a B2CL note, so use an export to reach output.
    assert!(!report.is_clean(), "own state should be rejected for B2CL");

    let json = payload(&[export(5, "EXPWP")], &ctx());
    // 250000 * 18% = 45000
    assert!(json.contains(r#""iamt":45000"#), "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
    assert!(!json.contains(r#""samt""#), "{json}");
}

#[test]
fn a_domestic_note_needs_a_place_of_supply() {
    let blank = with(5, "Place Of Supply", "");
    let report = validate(cdnur(), &[blank], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("cdnur.pos_required_for_domestic")),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_export_note_must_leave_the_place_of_supply_blank() {
    let e = export(5, "EXPWP").with_cell("Place Of Supply", "37-Andhra Pradesh");
    let report = validate(cdnur(), &[e], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("cdnur.pos_forbidden_for_exports")),
        "{:?}",
        report.findings
    );

    // Blank is accepted, and the key is omitted from the payload.
    let json = payload(&[export(5, "EXPWP")], &ctx());
    assert!(!json.contains(r#""pos""#), "{json}");
}

#[test]
fn an_export_note_may_not_use_the_reduced_rate() {
    let e = export(5, "EXPWOP").with_cell("Applicable % of Tax Rate", "65");
    let report = validate(cdnur(), &[e], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("cdnur.exports_take_the_full_rate")),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_export_without_payment_zeroes_tax_and_cess() {
    let e = export(5, "EXPWOP").with_cell("Cess Amount", "750");
    let json = payload(&[e], &ctx());
    assert!(json.contains(r#""iamt":0"#), "{json}");
    assert!(json.contains(r#""csamt":0"#), "{json}");

    // With payment of tax, both are computed normally.
    let w = export(6, "EXPWP").with_cell("Cess Amount", "750");
    let json = payload(&[w], &ctx());
    assert!(json.contains(r#""iamt":45000"#), "{json}");
    assert!(json.contains(r#""csamt":750"#), "{json}");
}

#[test]
fn a_domestic_notes_place_of_supply_must_differ_from_the_supplier_state() {
    let own = with(5, "Place Of Supply", "27-Maharashtra");
    let report = validate(cdnur(), &[own], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("inter-state supplies only")),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_unknown_ur_type_is_rejected() {
    let report = validate(cdnur(), &[with(5, "UR Type", "B2CS")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("UR Type")),
        "{:?}",
        report.findings
    );
}

#[test]
fn several_rates_for_one_note_become_one_record() {
    let second = with(6, "Rate", "5")
        .with_cell("Taxable Value", "20000")
        .with_cell("Cess Amount", "500");

    let report = validate(cdnur(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(cdnur(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 1);
    assert_eq!(out.to_json().matches(r#""num""#).count(), 2);
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = common::repo_path("fixtures/gstr1/cdnur-sample.csv");
    let rows = gst_core::import::read(&path, cdnur()).expect("reads");
    assert_eq!(rows.len(), 4);

    let json = payload(&rows, &ctx());
    // Four rows, three records: two rates share the first note.
    assert_eq!(json.matches(r#""nt_num""#).count(), 3, "{json}");
    assert!(json.contains(r#""typ":"EXPWOP""#), "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
}
