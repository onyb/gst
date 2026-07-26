//! End-to-end tax on advances received.
//!
//! Records sit at the top level keyed by place of supply, with bare line items
//! carrying `ad_amt` rather than a taxable value. This is one of the few
//! sections where BOTH tax branches are reachable: nothing rejects a place of
//! supply equal to the supplier's own state, and that is exactly the case that
//! produces central and state tax instead of integrated.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn at() -> &'static SectionSpec {
    spec::section("at").expect("at is registered")
}

fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(7, 2017).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    }
}

const COLUMNS: [&str; 5] = [
    "Place Of Supply",
    "Applicable % of Tax Rate",
    "Rate",
    "Gross Advance Received",
    "Cess Amount",
];

fn base(sheet_row: usize) -> Row {
    Row::from_pairs(
        sheet_row,
        COLUMNS
            .into_iter()
            .zip(["37-Andhra Pradesh", "", "18", "100000", ""]),
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    let mut r = base(sheet_row);
    r.cells.insert(column.to_owned(), value.to_owned());
    r
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    let report = validate(at(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(at(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(at()).is_empty());
}

#[test]
fn an_inter_state_advance_carries_integrated_tax_only() {
    assert_eq!(
        payload(&[base(5)], &ctx()),
        r#"[{"pos":"37","sply_ty":"INTER","itms":[{"rt":18,"ad_amt":100000,"iamt":18000,"csamt":0}]}]"#
    );
}

#[test]
fn the_suppliers_own_state_takes_the_central_state_split() {
    // Both branches are reachable here, unlike B2C(Large) or exports.
    assert_eq!(
        payload(&[with(5, "Place Of Supply", "27-Maharashtra")], &ctx()),
        r#"[{"pos":"27","sply_ty":"INTRA","itms":[{"rt":18,"ad_amt":100000,"camt":9000,"samt":9000,"csamt":0}]}]"#
    );
}

#[test]
fn an_sez_supplier_stays_inter_state_in_its_own_state() {
    let mut c = ctx();
    c.is_sez = true;
    let json = payload(&[with(5, "Place Of Supply", "27-Maharashtra")], &c);
    assert!(json.contains(r#""sply_ty":"INTER""#), "{json}");
    assert!(json.contains(r#""iamt":18000"#), "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
}

#[test]
fn line_items_carry_no_number_and_no_itm_det_wrapper() {
    let json = payload(&[base(5)], &ctx());
    assert!(!json.contains(r#""num""#), "{json}");
    assert!(!json.contains(r#""itm_det""#), "{json}");
    assert!(!json.contains(r#""txval""#), "{json}");
}

#[test]
fn a_blank_cess_is_emitted_as_zero() {
    // Validation writes 0 into a blank cess for the advance-received tables,
    // so unlike the adjustment tables the key is always present.
    let json = payload(&[base(5)], &ctx());
    assert!(json.contains(r#""csamt":0"#), "{json}");
}

#[test]
fn a_blank_tax_rate_factor_means_the_full_rate_and_emits_no_key() {
    let json = payload(&[base(5)], &ctx());
    assert!(!json.contains("diff_percent"), "{json}");
    assert!(json.contains(r#""iamt":18000"#), "{json}");

    // The reduced rate is emitted, and scales the tax.
    let json = payload(&[with(5, "Applicable % of Tax Rate", "65")], &ctx());
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    assert!(json.contains(r#""iamt":11700"#), "{json}");
}

#[test]
fn only_a_hundred_or_sixty_five_are_accepted_as_the_factor() {
    for bad in ["50", "0", "65.5"] {
        let report = validate(at(), &[with(5, "Applicable % of Tax Rate", bad)], &ctx());
        assert!(
            report
                .errors()
                .any(|f| f.column.as_deref() == Some("Applicable % of Tax Rate")),
            "'{bad}' should be rejected: {:?}",
            report.findings
        );
    }
    // A CSV writes the factor as '65.00'; the reference's string pattern
    // rejects that while accepting an Excel cell holding 65. We accept both.
    assert!(validate(at(), &[with(5, "Applicable % of Tax Rate", "65.00")], &ctx()).is_clean());
}

#[test]
fn a_zero_advance_is_rejected() {
    let report = validate(at(), &[with(5, "Gross Advance Received", "0")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("at.advance_must_not_be_zero")),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_negative_advance_is_accepted_and_taxed_negatively() {
    let mut r = with(5, "Gross Advance Received", "-100000");
    r.cells.insert("Cess Amount".into(), "-500".into());
    let json = payload(&[r], &ctx());
    assert!(json.contains(r#""ad_amt":-100000"#), "{json}");
    assert!(json.contains(r#""iamt":-18000"#), "{json}");
    assert!(json.contains(r#""csamt":-500"#), "{json}");
}

#[test]
fn the_cess_may_not_contradict_the_sign_of_the_advance() {
    // Negative advance with a positive cess, and the mirror case.
    let mut r = with(5, "Gross Advance Received", "-100000");
    r.cells.insert("Cess Amount".into(), "500".into());
    let report = validate(at(), &[r], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("at.cess_sign_follows_the_advance")),
        "{:?}",
        report.findings
    );

    let report = validate(at(), &[with(5, "Cess Amount", "-500")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("at.cess_sign_follows_the_advance")),
        "{:?}",
        report.findings
    );

    // A zero cess agrees with either sign.
    let mut r = with(5, "Gross Advance Received", "-100000");
    r.cells.insert("Cess Amount".into(), "0".into());
    assert!(validate(at(), &[r], &ctx()).is_clean());
}

#[test]
fn several_rates_for_one_place_of_supply_become_one_record() {
    let mut second = with(6, "Rate", "5");
    second
        .cells
        .insert("Gross Advance Received".into(), "50000".into());
    let report = validate(at(), &[base(5), second], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(at(), &report.records, &ctx());
    assert_eq!(out.envelopes.len(), 1);
    assert_eq!(out.to_json().matches(r#""ad_amt""#).count(), 2);
}

#[test]
fn rows_sharing_a_place_of_supply_must_agree_on_the_tax_rate_factor() {
    let mut conflicting = with(6, "Rate", "5");
    conflicting
        .cells
        .insert("Gross Advance Received".into(), "50000".into());
    conflicting
        .cells
        .insert("Applicable % of Tax Rate".into(), "65".into());
    let report = validate(at(), &[base(5), conflicting], &ctx());
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(at(), &report.records, &ctx());
    assert!(
        out.findings.iter().any(|f| f.severity == Severity::Error
            && f.message.contains("Applicable % of Tax Rate")),
        "{:?}",
        out.findings
    );
}

#[test]
fn a_place_of_supply_outside_the_code_range_is_rejected() {
    let report = validate(at(), &[with(5, "Place Of Supply", "99-Nowhere")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Place Of Supply")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/at-sample.csv");
    let rows = gst_core::import::read(&path, at()).expect("reads");
    assert_eq!(rows.len(), 4);

    let json = payload(&rows, &ctx());
    // Four rows, three records: two rates share the first place of supply.
    assert_eq!(json.matches(r#""pos""#).count(), 3, "{json}");
    assert!(json.contains(r#""sply_ty":"INTRA""#), "{json}");
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
}
