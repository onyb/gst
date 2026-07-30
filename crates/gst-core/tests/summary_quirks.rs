//! The pre-upload summary's quirks, each pinned against the reference:
//! credit notes subtract, without-payment supplies zero IGST and cess at
//! display time, supeco rows carry different key names, counts mean different
//! things per section, nil and doc_issue never appear, and the row order is
//! the official server order rather than this engine's registry order.

mod common;

use gst_core::generate::Generated;
use gst_core::payload::Json;
use gst_core::record::Row;
use gst_core::summary::{SectionSummary, meta_json, summarize};
use gst_core::upload::WorkbookRun;
use gst_core::validate::FilingContext;
use rust_decimal::Decimal;

/// A run holding exactly the given sections — the shape `summarize` reads.
fn run_of(entries: Vec<(&str, Generated)>) -> WorkbookRun {
    let mut run = WorkbookRun::default();
    for (code, out) in entries {
        run.sections.insert(code.to_string(), out);
    }
    run
}

fn summary_of(code: &str, rows: &[Row], ctx: &FilingContext) -> SectionSummary {
    let out = common::generated(common::sec(code), rows, ctx);
    let mut rows = summarize(&run_of(vec![(code, out)]), ctx);
    assert_eq!(rows.len(), 1, "expected one summary row: {rows:?}");
    rows.remove(0)
}

fn dec(n: i64) -> Decimal {
    Decimal::from(n)
}

#[test]
fn credit_notes_subtract_and_debit_notes_add() {
    let ctx = common::ctx(7, 2017);
    let credit = common::row(
        common::sec("cdnr"),
        5,
        &[
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
        ],
    );
    let debit = credit
        .clone()
        .with_cell("Note Number", "DN-001")
        .with_cell("Note Type", "D")
        .with_cell("Note Value", "118000")
        .with_cell("Taxable Value", "100000");

    // Each note counts once; the debit's 18000 IGST less the credit's 9000.
    let s = summary_of("cdnr", &[credit, debit], &ctx);
    assert_eq!(s.count, 2);
    assert_eq!(s.totals.igst, dec(9000));
    assert_eq!(s.totals.cess, dec(0));
}

#[test]
fn cdnur_reads_the_note_type_on_the_record_and_totals_can_go_negative() {
    let ctx = common::ctx(9, 2017);
    let credit = common::row(
        common::sec("cdnur"),
        5,
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
    );
    let s = summary_of("cdnur", &[credit], &ctx);
    assert_eq!(s.count, 1);
    assert_eq!(s.totals.igst, dec(-45000));
}

#[test]
fn expwop_zeroes_an_unregistered_export_note_before_the_sign_matters() {
    let ctx = common::ctx(9, 2017);
    let note = common::row(
        common::sec("cdnur"),
        5,
        &[
            "EXPWOP",
            "UN-002",
            "14-Jul-17",
            "C",
            "",
            "295000",
            "",
            "18",
            "250000",
            "750",
        ],
    );
    let s = summary_of("cdnur", &[note], &ctx);
    assert_eq!(s.count, 1);
    assert_eq!(s.totals, Default::default());
}

#[test]
fn sewop_zeroes_igst_and_cess_but_the_invoice_still_counts() {
    let ctx = common::ctx(7, 2017);
    let regular = common::row(
        common::sec("b2b"),
        5,
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
    );
    let sez_without_payment = regular
        .clone()
        .with_cell("Invoice Number", "INV-002")
        .with_cell("Invoice Type", "SEZ supplies without payment");

    // Both invoices count, but only the regular one's 8100 IGST survives.
    let s = summary_of("b2b", &[regular, sez_without_payment], &ctx);
    assert_eq!(s.count, 2);
    assert_eq!(s.totals.igst, dec(8100));
    assert_eq!(s.totals.cess, dec(0));
}

#[test]
fn wopay_zeroes_exports_reading_the_type_on_the_group() {
    let ctx = common::ctx(7, 2017);
    let with_payment = common::row(
        common::sec("exp"),
        5,
        &[
            "WPAY",
            "EX-001",
            "14-Jul-17",
            "295000",
            "INMAA1",
            "7896542",
            "18-Jul-17",
            "18",
            "250000",
            "750",
        ],
    );
    let without_payment = with_payment
        .clone()
        .with_cell("Export Type", "WOPAY")
        .with_cell("Invoice Number", "EX-002");

    // Only the with-payment invoice's tax and cess reach the totals.
    let s = summary_of("exp", &[with_payment, without_payment], &ctx);
    assert_eq!(s.count, 2);
    assert_eq!(s.totals.igst, dec(45000));
    assert_eq!(s.totals.cess, dec(750));
}

#[test]
fn supeco_counts_both_members_and_reads_the_gst_key_names() {
    let ctx = common::ctx(6, 2025);
    let row = |nature: &str| {
        common::row(
            common::sec("supeco"),
            5,
            &[
                nature,
                "12AJIPA1572E1C7",
                "Acme Marketplace",
                "100000",
                "18000",
                "0",
                "0",
                "0",
            ],
        )
    };
    let rows = [
        row("Liable to collect tax u/s 52(TCS)"),
        row("Liable to pay tax u/s 9(5)"),
    ];
    // The rows carry igst/cgst/sgst/cess, not iamt/camt/samt/csamt — a wrong
    // alias set would sum to zero here.
    let s = summary_of("supeco", &rows, &ctx);
    assert_eq!(s.count, 2);
    assert_eq!(s.totals.igst, dec(36000));
}

#[test]
fn b2csa_counts_rate_rows_not_records() {
    let ctx = common::ctx(9, 2017);
    let first = common::row(
        common::sec("b2csa"),
        5,
        &[
            "2017-18",
            "JULY",
            "37-Andhra Pradesh",
            "OE",
            "",
            "18",
            "300000",
            "",
            "",
        ],
    );
    let second = first
        .clone()
        .with_cell("Rate", "5")
        .with_cell("Taxable Value", "100000");

    // One grouped record, but the reference counts its two rate items.
    let s = summary_of("b2csa", &[first, second], &ctx);
    assert_eq!(s.count, 2);
    assert_eq!(s.totals.igst, dec(59000));
}

#[test]
fn advances_sum_the_item_tax_but_never_the_gross_advance() {
    let ctx = common::ctx(7, 2017);
    let advance = common::row(
        common::sec("at"),
        5,
        &["37-Andhra Pradesh", "", "18", "100000", ""],
    );
    let s = summary_of("at", &[advance], &ctx);
    assert_eq!(s.count, 1);
    // 18% of the 100000 advance; the 100000 itself (ad_amt) stays out.
    assert_eq!(s.totals.igst, dec(18000));
}

#[test]
fn ecomab2c_counts_pos_items_and_sums_their_bare_items() {
    let ctx = common::ctx(6, 2025);
    let row = common::row(
        common::sec("ecomab2c"),
        5,
        &[
            "2017-18",
            "JULY",
            "29AAPFU0939F1ZR",
            "Seller Ltd",
            "37-Andhra Pradesh",
            "18",
            "50000",
            "0",
        ],
    );
    let s = summary_of("ecomab2c", &[row], &ctx);
    assert_eq!(s.count, 1);
    assert_eq!(s.totals.igst, dec(9000));
}

/// A hand-built envelope, for tests that are about the walk rather than the
/// generation pipeline.
fn envelope(build: impl FnOnce(&mut Json)) -> Generated {
    let mut e = Json::obj();
    build(&mut e);
    Generated {
        envelopes: vec![e],
        members: vec![None],
        findings: vec![],
    }
}

fn item(pairs: &[(&str, i64)]) -> Json {
    let mut o = Json::obj();
    for (k, v) in pairs {
        o.insert_path(k, Json::Num(Decimal::from(*v)));
    }
    o
}

#[test]
fn excluded_and_empty_sections_never_appear() {
    let ctx = common::ctx(6, 2025);
    // nil and doc_issue have records; b2b exists with an envelope holding no
    // invoices, so its count stays 0.
    let run = run_of(vec![
        (
            "nil",
            envelope(|e| e.insert_path("sply_ty", Json::Str("INTRB2B".into()))),
        ),
        (
            "doc_issue",
            envelope(|e| e.insert_path("doc_num", Json::Num(dec(1)))),
        ),
        ("b2b", envelope(|e| e.insert_path("inv", Json::Arr(vec![])))),
    ]);
    assert!(summarize(&run, &ctx).is_empty());
}

#[test]
fn an_iff_month_keeps_only_the_eight_iff_rows() {
    // Data in supeco and b2b; a quarterly filer in month 1 of a quarter may
    // only file the IFF set, so only the b2b row survives. At quarter-end the
    // full monthly row set returns.
    let supeco = envelope(|e| e.insert_path("igst", Json::Num(dec(100))));
    let b2b = envelope(|e| {
        let mut det = Json::obj();
        det.insert_path("itm_det", item(&[("camt", 9)]));
        let mut inv = Json::obj();
        inv.insert_path("itms", Json::Arr(vec![det]));
        e.insert_path("inv", Json::Arr(vec![inv]));
    });
    let run = run_of(vec![("supeco", supeco), ("b2b", b2b)]);

    let mut iff = common::ctx(7, 2025);
    iff.is_quarterly = true;
    let rows: Vec<&str> = summarize(&run, &iff).iter().map(|s| s.cd).collect();
    assert_eq!(rows, ["b2b"]);

    let mut quarter_end = common::ctx(9, 2025);
    quarter_end.is_quarterly = true;
    let rows: Vec<&str> = summarize(&run, &quarter_end).iter().map(|s| s.cd).collect();
    assert_eq!(rows, ["b2b", "supeco"]);
}

#[test]
fn eco_rows_are_gated_by_period() {
    let row = envelope(|e| e.insert_path("igst", Json::Num(dec(100))));
    let run = run_of(vec![("supeco", row)]);
    // Table 14 exists only from 01-2024; a 2017 filing has no such row.
    assert!(summarize(&run, &common::ctx(7, 2017)).is_empty());
    assert_eq!(summarize(&run, &common::ctx(6, 2025)).len(), 1);
}

#[test]
fn rows_follow_the_official_order_not_the_registry_order() {
    let inv_of = |items: Json| {
        let mut inv = Json::obj();
        inv.insert_path("itms", Json::Arr(vec![items]));
        Json::Arr(vec![inv])
    };
    let run = run_of(vec![
        (
            "hsn(b2b)",
            envelope(|e| e.insert_path("iamt", Json::Num(dec(3)))),
        ),
        (
            "supeco",
            envelope(|e| e.insert_path("igst", Json::Num(dec(2)))),
        ),
        (
            "b2b",
            envelope(|e| {
                let mut det = Json::obj();
                det.insert_path("itm_det", item(&[("camt", 9), ("samt", 9)]));
                e.insert_path("inv", inv_of(det));
            }),
        ),
        (
            "exp",
            envelope(|e| e.insert_path("inv", inv_of(item(&[("iamt", 5)])))),
        ),
        (
            "at",
            envelope(|e| e.insert_path("itms", Json::Arr(vec![item(&[("iamt", 1)])]))),
        ),
    ]);
    let summaries = summarize(&run, &common::ctx(6, 2025));
    let order: Vec<&str> = summaries.iter().map(|s| s.cd).collect();
    assert_eq!(order, ["b2b", "exp", "at", "supeco", "hsn(b2b)"]);
}

#[test]
fn the_meta_shape_matches_the_reference_sidecar() {
    let ctx = common::ctx(6, 2025);
    let run = run_of(vec![(
        "b2cs",
        envelope(|e| {
            e.insert_path("txval", Json::Num(dec(50000)));
            e.insert_path("iamt", Json::Num(dec(9000)));
            e.insert_path("csamt", Json::Num(dec(0)));
        }),
    )]);
    let summaries = summarize(&run, &ctx);
    assert_eq!(
        meta_json(&summaries, &ctx).to_json(),
        r#"{"gstin":"27AAPFU0939F1ZV","fp":"062025","version":"GST3.2.4","hash":"hash","counts":[{"cd":"b2cs","result":{"cgTl":0,"sgTl":0,"igTl":9000,"csTl":0},"count":1,"name":"B2C(Small) Details - 7"}]}"#
    );
}
