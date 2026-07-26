//! End-to-end B2C(Large): the first section that is not shaped like B2B.
//!
//! No recipient GSTIN, no reverse charge, no invoice type — the payload groups
//! by place of supply because the recipients are unregistered. It is also the
//! first section with a period-dependent rule: the invoice-value threshold
//! changed with the August 2024 period.

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, validate};

fn b2cl() -> &'static SectionSpec {
    spec::section("b2cl").expect("b2cl is registered")
}

/// A Maharashtra (27) supplier, so any other state is inter-state.
fn ctx(month: u32, year: i32) -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(month, year).unwrap(),
        is_sez: false,
    }
}

const COLUMNS: [&str; 9] = [
    "Invoice Number",
    "Invoice date",
    "Invoice Value",
    "Place Of Supply",
    "Applicable % of Tax Rate",
    "Rate",
    "Taxable Value",
    "Cess Amount",
    "E-Commerce GSTIN",
];

fn row(sheet_row: usize, values: [&str; 9]) -> Row {
    Row::from_pairs(sheet_row, COLUMNS.into_iter().zip(values))
}

fn base(sheet_row: usize) -> Row {
    row(
        sheet_row,
        [
            "INV-L001",
            "14-Jul-17",
            "295000",
            "37-Andhra Pradesh",
            "",
            "18",
            "250000",
            "",
            "",
        ],
    )
}

fn with(sheet_row: usize, column: &str, value: &str) -> Row {
    let mut r = base(sheet_row);
    r.cells.insert(column.to_owned(), value.to_owned());
    r
}

fn payload(rows: &[Row], c: &FilingContext) -> String {
    let report = validate(b2cl(), rows, c);
    assert!(report.is_clean(), "validation: {:?}", report.findings);
    let out = generate(b2cl(), &report.records, c);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "generation: {:?}",
        out.findings
    );
    out.to_json()
}

#[test]
fn every_derivation_the_spec_names_is_implemented() {
    assert!(gst_core::generate::unimplemented_derivations(b2cl()).is_empty());
}

#[test]
fn the_payload_groups_by_place_of_supply_not_recipient() {
    assert_eq!(
        payload(&[base(5)], &ctx(7, 2017)),
        r#"[{"pos":"37","inv":[{"inum":"INV-L001","idt":"14-07-2017","val":295000,"diff_percent":1,"itms":[{"num":1801,"itm_det":{"txval":250000,"rt":18,"iamt":45000,"csamt":0}}]}]}]"#
    );
}

#[test]
fn separate_places_of_supply_become_separate_envelopes() {
    let mut second = with(6, "Place Of Supply", "29-Karnataka");
    second
        .cells
        .insert("Invoice Number".into(), "INV-L002".into());
    let report = validate(b2cl(), &[base(5), second], &ctx(7, 2017));
    assert!(report.is_clean(), "{:?}", report.findings);
    let out = generate(b2cl(), &report.records, &ctx(7, 2017));
    assert_eq!(out.envelopes.len(), 2);
}

#[test]
fn the_value_threshold_follows_the_return_period() {
    // 1,50,000 clears the post-August-2024 bar of 1,00,000 but not the earlier
    // 2,50,000 one — the same row is valid or invalid depending on the period.
    let r = |sheet_row| {
        let mut x = with(sheet_row, "Invoice Value", "150000");
        x.cells.insert("Taxable Value".into(), "120000".into());
        x.cells.insert("Invoice date".into(), "14-Aug-24".into());
        x
    };

    let after = validate(b2cl(), &[r(5)], &ctx(8, 2024));
    assert!(after.is_clean(), "August 2024: {:?}", after.findings);

    let mut before_row = r(5);
    before_row
        .cells
        .insert("Invoice date".into(), "14-Jul-24".into());
    let before = validate(b2cl(), &[before_row], &ctx(7, 2024));
    assert!(
        before
            .errors()
            .any(|f| f.message.contains("more than 250000")),
        "July 2024: {:?}",
        before.findings
    );
}

#[test]
fn the_threshold_is_exclusive() {
    // Exactly at the threshold does not belong in this table.
    let mut at = with(5, "Invoice Value", "100000");
    at.cells.insert("Taxable Value".into(), "90000".into());
    at.cells.insert("Invoice date".into(), "14-Aug-24".into());
    let report = validate(b2cl(), &[at], &ctx(8, 2024));
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("more than 100000")),
        "{:?}",
        report.findings
    );

    // One paisa over is enough.
    let mut over = with(5, "Invoice Value", "100000.01");
    over.cells.insert("Taxable Value".into(), "90000".into());
    over.cells.insert("Invoice date".into(), "14-Aug-24".into());
    assert!(validate(b2cl(), &[over], &ctx(8, 2024)).is_clean());
}

#[test]
fn the_place_of_supply_cannot_be_the_suppliers_own_state() {
    // This table is inter-state by definition.
    let own = with(5, "Place Of Supply", "27-Maharashtra");
    let report = validate(b2cl(), &[own], &ctx(7, 2017));
    assert!(
        report
            .errors()
            .any(|f| f.message.contains("inter-state supplies only")),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_sez_supplier_is_exempt_from_the_interstate_check() {
    // The reference skips the check for an SEZ filer, whose supplies are
    // treated as inter-state regardless of the state involved.
    let own = with(5, "Place Of Supply", "27-Maharashtra");
    let sez = FilingContext {
        is_sez: true,
        ..ctx(7, 2017)
    };
    let json = payload(&[own], &sez);
    // Still integrated tax, never a central/state split.
    assert!(json.contains(r#""iamt":45000"#), "{json}");
    assert!(!json.contains(r#""camt""#), "{json}");
}

#[test]
fn tax_is_always_integrated_never_split() {
    // The reference's central/state branch is unreachable for this section, so
    // camt/samt must never appear whatever the state or filer type.
    for (pos, is_sez) in [
        ("37-Andhra Pradesh", false),
        ("29-Karnataka", false),
        ("27-Maharashtra", true),
    ] {
        let mut c = ctx(7, 2017);
        c.is_sez = is_sez;
        let json = payload(&[with(5, "Place Of Supply", pos)], &c);
        assert!(json.contains(r#""iamt""#), "{pos}: {json}");
        assert!(!json.contains(r#""camt""#), "{pos}: {json}");
    }
}

#[test]
fn a_blank_cess_is_zero_not_null() {
    // Deliberate divergence: the reference omits the empty-cell guard it uses
    // in B2B, producing NaN which serializes as null.
    let json = payload(&[base(5)], &ctx(7, 2017));
    assert!(json.contains(r#""csamt":0"#), "{json}");
    assert!(!json.contains("null"), "{json}");
}

#[test]
fn an_ecommerce_gstin_is_rejected_despite_the_template_column() {
    let report = validate(
        b2cl(),
        &[with(5, "E-Commerce GSTIN", "12AJIPA1572E1C7")],
        &ctx(7, 2017),
    );
    assert!(
        report.errors().any(|f| f.message.contains("must be blank")),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_multi_rate_invoice_becomes_one_invoice_with_several_items() {
    let mut second = with(6, "Rate", "5");
    second.cells.insert("Taxable Value".into(), "50000".into());
    let json = payload(&[base(5), second], &ctx(7, 2017));
    assert_eq!(json.matches(r#""inum":"INV-L001""#).count(), 1, "{json}");
    assert!(json.contains(r#""num":1801"#), "{json}");
    assert!(json.contains(r#""num":501"#), "{json}");
}

#[test]
fn applicable_percent_scales_the_tax() {
    let json = payload(&[with(5, "Applicable % of Tax Rate", "65")], &ctx(7, 2017));
    assert!(json.contains(r#""diff_percent":0.65"#), "{json}");
    // 250000 * 18% * 0.65 = 29250
    assert!(json.contains(r#""iamt":29250"#), "{json}");
}

#[test]
fn the_shipped_fixture_validates_and_generates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gstr1/b2cl-sample.csv");
    let rows = gst_core::import::read(&path, b2cl()).expect("reads");
    assert_eq!(rows.len(), 5);

    let json = payload(&rows, &ctx(7, 2017));
    // Three places of supply; one invoice spans two rate rows.
    assert_eq!(json.matches(r#""pos""#).count(), 3, "{json}");
    assert_eq!(json.matches(r#""inum""#).count(), 4, "{json}");
    assert!(json.contains(r#""csamt":1500"#), "{json}");
}
