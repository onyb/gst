//! Behaviours found by reading GSTN's own JavaScript, each of which this
//! implementation previously got wrong.
//!
//! Every case here was checked against the reference source, and several
//! against the running tool. They are gathered in one file because what they
//! have in common is provenance rather than section: none of them is reachable
//! from the captured golden files, which is exactly why they survived.

mod common;

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::upload::{self, Turnover};
use gst_core::validate::{FilingContext, validate};

fn sec(code: &str) -> &'static SectionSpec {
    common::sec(code)
}

/// Every template column, blank except those named.
fn sparse(spec: &SectionSpec, sheet_row: usize, cells: &[(&str, &str)]) -> Row {
    Row::from_pairs(
        sheet_row,
        spec.columns().into_iter().map(|column| {
            let value = cells
                .iter()
                .find(|(name, _)| *name == column)
                .map_or("", |(_, value)| *value);
            (column, value)
        }),
    )
}

// ---------------------------------------------------------------------------
// Registration numbers
// ---------------------------------------------------------------------------

/// The reference compares a GSTIN against `checkGstn(gst.substr(0, 14))`, which
/// appends a check character taken from an uppercase alphabet. A lowercase
/// check character therefore never matches, however correct its value — while
/// the shape patterns admit one.
#[test]
fn a_lowercase_check_digit_fails_even_though_the_shape_allows_it() {
    assert!(gst_core::gstin::checksum_valid("12GEOPS0823BBZH"));
    assert!(!gst_core::gstin::checksum_valid("12geops0823bbzh"));
    // Only the 15th character is case-sensitive; the body is folded.
    assert!(gst_core::gstin::checksum_valid("12geops0823bbzH"));
}

/// Table 14's operator GSTIN went through a shape test only. The reference
/// runs the full `validateGSTIN`, check digit included.
#[test]
fn an_operator_gstin_with_a_bad_check_digit_is_rejected() {
    let spec = sec("supeco");
    let row = |etin: &str| {
        sparse(
            spec,
            5,
            &[
                ("Nature of Supply", "Liable to collect tax u/s 52(TCS)"),
                ("GSTIN of E-Commerce Operator", etin),
                ("Trade/Legal name", "Acme Marketplace"),
                ("Net value of supplies", "100000"),
                ("Integrated tax", "18000"),
                ("Central tax", "0"),
                ("State/UT tax", "0"),
                ("Cess", "0"),
            ],
        )
    };
    let ctx = common::ctx(6, 2025);

    assert!(validate(spec, &[row("12AJIPA1572E1C7")], &ctx).is_clean());

    let report = validate(spec, &[row("12AJIPA1572E1C0")], &ctx);
    assert!(
        report.errors().any(|f| f.message.contains("check digit")),
        "{:?}",
        report.findings
    );
}

// ---------------------------------------------------------------------------
// Table 14 cross-field rules
// ---------------------------------------------------------------------------

fn eco_row(sheet_row: usize, net: &str, igst: &str, cgst: &str, sgst: &str) -> Row {
    sparse(
        sec("supeco"),
        sheet_row,
        &[
            ("Nature of Supply", "Liable to collect tax u/s 52(TCS)"),
            ("GSTIN of E-Commerce Operator", "12AJIPA1572E1C7"),
            ("Trade/Legal name", "Acme Marketplace"),
            ("Net value of supplies", net),
            ("Integrated tax", igst),
            ("Central tax", cgst),
            ("State/UT tax", sgst),
            ("Cess", "0"),
        ],
    )
}

#[test]
fn central_and_state_tax_must_be_equal() {
    let report = validate(
        sec("supeco"),
        &[eco_row(5, "100000", "0", "100", "200")],
        &common::ctx(6, 2025),
    );
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("supeco.central_and_state_tax_must_match")),
        "{:?}",
        report.findings
    );
}

#[test]
fn tax_must_agree_in_sign_with_the_net_value_of_supplies() {
    let ctx = common::ctx(6, 2025);

    // Positive supplies, negative tax.
    let report = validate(
        sec("supeco"),
        &[eco_row(5, "100000", "-50", "0", "0")],
        &ctx,
    );
    assert!(
        report
            .errors()
            .any(|f| f.rule.as_deref() == Some("supeco.igst_sign_agrees_with_net_value")),
        "{:?}",
        report.findings
    );

    // An all-negative row is legitimate: these tables report a net figure.
    let all_negative = eco_row(6, "-100000", "-18000", "0", "0");
    let report = validate(sec("supeco"), &[all_negative], &ctx);
    assert!(report.is_clean(), "{:?}", report.findings);
}

/// Only rows liable to COLLECT tax carry the extra operator-marker pattern; the
/// reference ANDs it in for that branch alone.
#[test]
fn the_operator_marker_is_required_only_for_tcs_rows() {
    let spec = sec("supeco");
    let ctx = common::ctx(6, 2025);
    let with_nature = |nature: &str| {
        sparse(
            spec,
            5,
            &[
                ("Nature of Supply", nature),
                // A valid GSTIN, but a regular one: 'Z' where an operator
                // registration carries 'C'.
                ("GSTIN of E-Commerce Operator", "27AAPFU0939F1ZV"),
                ("Trade/Legal name", "Acme Marketplace"),
                ("Net value of supplies", "100000"),
                ("Integrated tax", "18000"),
                ("Central tax", "0"),
                ("State/UT tax", "0"),
                ("Cess", "0"),
            ],
        )
    };

    let report = validate(
        spec,
        &[with_nature("Liable to collect tax u/s 52(TCS)")],
        &ctx,
    );
    assert!(
        report.errors().any(|f| {
            f.rule.as_deref() == Some("supeco.tcs_operator_gstin_carries_the_operator_marker")
        }),
        "{:?}",
        report.findings
    );

    let report = validate(spec, &[with_nature("Liable to pay tax u/s 9(5)")], &ctx);
    assert!(report.is_clean(), "{:?}", report.findings);
}

// ---------------------------------------------------------------------------
// Dates and places of supply
// ---------------------------------------------------------------------------

/// A shipping bill is routinely raised after the invoice's return period has
/// closed. The reference passes `allowFuture` for this one date.
#[test]
fn a_shipping_bill_may_be_dated_after_the_return_period() {
    let spec = sec("exp");
    let row = sparse(
        spec,
        5,
        &[
            ("Export Type", "WPAY"),
            ("Invoice Number", "EX-001"),
            ("Invoice date", "14-Jul-17"),
            ("Invoice Value", "295000"),
            ("Port Code", "INMAA1"),
            ("Shipping Bill Number", "7896542"),
            ("Shipping Bill Date", "05-Aug-17"),
            ("Rate", "18"),
            ("Taxable Value", "250000"),
        ],
    );
    let report = validate(spec, &[row], &common::ctx(7, 2017));
    assert!(report.is_clean(), "{:?}", report.findings);

    // The floor still applies: nothing predates GST itself.
    let early = sparse(
        spec,
        5,
        &[
            ("Export Type", "WPAY"),
            ("Invoice Number", "EX-001"),
            ("Invoice date", "14-Jul-17"),
            ("Invoice Value", "295000"),
            ("Port Code", "INMAA1"),
            ("Shipping Bill Number", "7896542"),
            ("Shipping Bill Date", "05-Jun-17"),
            ("Rate", "18"),
            ("Taxable Value", "250000"),
        ],
    );
    let report = validate(spec, &[early], &common::ctx(7, 2017));
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Shipping Bill Date")),
        "{:?}",
        report.findings
    );
}

/// The reference range-checks the WHOLE cell with `parseInt`, so `296` is 296
/// and out of range. Taking a two-character prefix would read it as `29`.
#[test]
fn a_three_digit_place_of_supply_is_rejected_not_truncated() {
    let spec = sec("b2cs");
    let row = |pos: &str| {
        sparse(
            spec,
            5,
            &[
                ("Type", "OE"),
                ("Place Of Supply", pos),
                ("Applicable % of Tax Rate", "100"),
                ("Rate", "18"),
                ("Taxable Value", "100000"),
                ("Cess Amount", "0"),
            ],
        )
    };
    let ctx = common::ctx(7, 2017);

    assert!(validate(spec, &[row("29-Karnataka")], &ctx).is_clean());

    let report = validate(spec, &[row("296")], &ctx);
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Place Of Supply")),
        "296 must not be read as Karnataka: {:?}",
        report.findings
    );
}

/// The SEZ carve-out on the place-of-supply check belongs to B2C(Large) alone.
#[test]
fn an_sez_filer_still_cannot_issue_an_unregistered_note_in_its_own_state() {
    let spec = sec("cdnur");
    let row = sparse(
        spec,
        5,
        &[
            ("UR Type", "B2CL"),
            ("Note/Refund Voucher Number", "NT-1"),
            ("Note/Refund Voucher date", "20-Jul-17"),
            ("Document Type", "C"),
            ("Place Of Supply", "27-Maharashtra"),
            ("Note/Refund Voucher Value", "300000"),
            ("Rate", "18"),
            ("Taxable Value", "250000"),
        ],
    );
    let sez = FilingContext {
        is_sez: true,
        ..common::ctx(7, 2017)
    };
    let report = validate(spec, &[row], &sez);
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Place Of Supply")),
        "{:?}",
        report.findings
    );
}

// ---------------------------------------------------------------------------
// The upload envelope
// ---------------------------------------------------------------------------

fn qrmp(month: u32) -> FilingContext {
    FilingContext {
        period: ReturnPeriod::new(month, 2025).expect("valid"),
        is_quarterly: true,
        ..common::ctx(month, 2025)
    }
}

fn b2b_section() -> gst_core::generate::Generated {
    let spec = sec("b2b");
    let row = sparse(
        spec,
        5,
        &[
            ("GSTIN/UIN of Recipient", "12GEOPS0823BBZH"),
            ("Receiver Name", "Buyer Ltd"),
            ("Invoice Number", "INV-001"),
            ("Invoice date", "14-Jun-2025"),
            ("Invoice Value", "118000"),
            ("Place Of Supply", "37-Andhra Pradesh"),
            ("Reverse Charge", "N"),
            ("Applicable % of Tax Rate", "100"),
            ("Invoice Type", "Regular B2B"),
            ("Rate", "18"),
            ("Taxable Value", "100000"),
            ("Cess Amount", "0"),
        ],
    );
    let ctx = common::ctx(6, 2025);
    let report = validate(spec, &[row], &ctx);
    assert!(report.is_clean(), "{:?}", report.findings);
    generate(spec, &report.records, &ctx)
}

fn at_section() -> gst_core::generate::Generated {
    let spec = sec("at");
    let row = sparse(
        spec,
        5,
        &[
            ("Place Of Supply", "37-Andhra Pradesh"),
            ("Applicable % of Tax Rate", "100"),
            ("Rate", "18"),
            ("Gross Advance Received", "50000"),
            ("Cess Amount", "0"),
        ],
    );
    let ctx = common::ctx(6, 2025);
    let report = validate(spec, &[row], &ctx);
    assert!(report.is_clean(), "{:?}", report.findings);
    generate(spec, &report.records, &ctx)
}

/// In months 1 and 2 of a quarter a QRMP filer submits an IFF, which carries
/// four tables and the header keys and nothing else.
#[test]
fn a_quarterly_filers_first_two_months_carry_only_the_iff_tables() {
    let mut sections = std::collections::HashMap::new();
    sections.insert("b2b".to_string(), b2b_section());
    sections.insert("at".to_string(), at_section());

    // Month 1 of the quarter: an IFF.
    let iff = upload::build(&sections, &qrmp(4), Turnover::default()).to_json();
    assert!(iff.contains(r#""b2b""#), "{iff}");
    assert!(
        !iff.contains(r#""at""#),
        "advances do not belong in an IFF: {iff}"
    );

    // Month 3 closes the quarter and takes the full return.
    let full = upload::build(&sections, &qrmp(6), Turnover::default()).to_json();
    assert!(full.contains(r#""b2b""#), "{full}");
    assert!(full.contains(r#""at""#), "{full}");

    // A monthly filer is never filtered.
    let monthly = upload::build(&sections, &common::ctx(4, 2025), Turnover::default()).to_json();
    assert!(monthly.contains(r#""at""#), "{monthly}");
}

/// The turnover figures are read with `parseInt` and emitted together or not
/// at all.
#[test]
fn turnover_is_written_as_a_whole_number_and_only_ever_as_a_pair() {
    use rust_decimal::Decimal;

    let sections = std::collections::HashMap::new();
    let ctx = common::ctx(7, 2017);

    let both = Turnover {
        gross: Some(Decimal::new(1234567890, 2)), // 12345678.90
        current: Some(Decimal::new(50000050, 2)), // 500000.50
    };
    let json = upload::build(&sections, &ctx, both).to_json();
    assert!(json.contains(r#""gt":12345678"#), "{json}");
    assert!(json.contains(r#""cur_gt":500000"#), "{json}");

    // Half a pair is no pair: the reference has no input that produces one key
    // without the other.
    let lone = Turnover {
        gross: None,
        current: Some(Decimal::from(500000)),
    };
    let json = upload::build(&sections, &ctx, lone).to_json();
    assert!(!json.contains("cur_gt"), "{json}");
    assert!(!json.contains(r#""gt""#), "{json}");
}

/// The reference's omit-empty walks array elements too, so an empty key inside
/// an invoice is dropped at any depth — not only at the top level.
#[test]
fn omit_empty_reaches_inside_arrays() {
    use gst_core::payload::Json;

    let mut sections = std::collections::HashMap::new();
    sections.insert(
        "b2b".to_string(),
        gst_core::generate::Generated {
            envelopes: vec![Json::Obj(vec![
                ("ctin".to_string(), Json::Str("12GEOPS0823BBZH".into())),
                (
                    "inv".to_string(),
                    Json::Arr(vec![Json::Obj(vec![
                        ("inum".to_string(), Json::Str("INV-001".into())),
                        ("omon".to_string(), Json::Null),
                        ("csamt".to_string(), Json::Num(0.into())),
                    ])]),
                ),
            ])],
            ..Default::default()
        },
    );
    let json = upload::build(&sections, &common::ctx(7, 2017), Turnover::default()).to_json();
    assert!(json.contains(r#""inum":"INV-001""#), "{json}");
    assert!(
        !json.contains("omon"),
        "a null inside an array survives: {json}"
    );
    // A numeric zero is a value, not emptiness.
    assert!(json.contains(r#""csamt":0"#), "{json}");
}

/// The reference measures a size that is not the file's length: it stringifies
/// an already-serialized string, so every quote and backslash costs an extra
/// byte. It therefore splits earlier than its own constant suggests.
#[test]
fn the_chunk_size_counts_the_references_double_encoding() {
    assert_eq!(upload::reference_size(""), 2);
    // Three quotes -> three escape bytes on top of the body and the wrapper.
    assert_eq!(upload::reference_size(r#"{"a":1}"#), 7 + 2 + 2);
    assert!(upload::reference_size(r#"{"a":"b"}"#) > r#"{"a":"b"}"#.len());
}

// ---------------------------------------------------------------------------
// Sections and periods
// ---------------------------------------------------------------------------

/// The HSN summary is a single sheet up to 05-2025 and a pair from it. A
/// workbook carrying the wrong one for its period used to be read, generated,
/// and then dropped on the floor by the envelope.
#[test]
fn hsn_sections_are_scoped_to_the_periods_they_are_filed_for() {
    let combined = sec("hsn");
    let bifurcated = sec("hsn(b2b)");

    let april_2025 = common::ctx(4, 2025).period.as_yyyymm();
    let may_2025 = common::ctx(5, 2025).period.as_yyyymm();

    assert!(combined.active_for(april_2025));
    assert!(!combined.active_for(may_2025));
    assert!(!bifurcated.active_for(april_2025));
    assert!(bifurcated.active_for(may_2025));
}

/// Every section the envelope draws from has to exist, or its key is silently
/// empty in every file produced.
#[test]
fn the_combined_hsn_section_is_registered() {
    assert!(spec::section("hsn").is_some());
    assert_eq!(
        spec::section("hsn")
            .unwrap()
            .source
            .excel
            .as_ref()
            .unwrap()
            .sheet,
        "hsn"
    );
}

/// Two ranges of one document nature may legitimately start at the same serial.
#[test]
fn two_document_ranges_may_share_a_starting_number() {
    let spec = sec("doc_issue");
    let row = |sheet_row: usize, to: &str| {
        sparse(
            spec,
            sheet_row,
            &[
                ("Nature of Document", "Invoices for outward supply"),
                ("Sr. No. From", "INV-001"),
                ("Sr. No. To", to),
                ("Total Number", "100"),
                ("Cancelled", "0"),
            ],
        )
    };
    let ctx = common::ctx(7, 2017);
    let rows = [row(5, "INV-100"), row(6, "INV-050")];
    let report = validate(spec, &rows, &ctx);
    assert!(report.is_clean(), "{:?}", report.findings);

    let out = generate(spec, &report.records, &ctx);
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "{:?}",
        out.findings
    );
    let json = out.to_json();
    assert_eq!(json.matches(r#""from""#).count(), 2, "{json}");
}

// ---------------------------------------------------------------------------
// HSN summaries
// ---------------------------------------------------------------------------

/// A service code discards the unit and the quantity.
///
/// The reference rewrites both cells before validation whenever the code starts
/// with 99 (`offline2.js:390` for the combined table, `:399` for the pair), so
/// whatever the filer typed there never reaches the payload and is never
/// reported as wrong.
#[test]
fn a_service_code_forces_the_unit_to_na_and_the_quantity_to_zero() {
    for code in ["hsn", "hsn(b2b)", "hsn(b2c)"] {
        let spec = sec(code);
        // The combined table is only filed before the bifurcation.
        let ctx = if code == "hsn" {
            common::ctx(6, 2024)
        } else {
            common::ctx(6, 2025)
        };
        let row = |hsn: &str| {
            sparse(
                spec,
                5,
                &[
                    ("HSN", hsn),
                    ("Description", "Consultancy"),
                    ("UQC", "KGS-KILOGRAMS"),
                    ("Total Quantity", "42"),
                    ("Total Value", "118000"),
                    ("Rate", "18"),
                    ("Taxable Value", "100000"),
                    ("Integrated Tax Amount", "18000"),
                ],
            )
        };

        let report = validate(spec, &[row("998313")], &ctx);
        assert!(report.is_clean(), "{code}: {:?}", report.findings);
        let json = generate(spec, &report.records, &ctx).to_json();
        assert!(json.contains(r#""uqc":"NA""#), "{code}: {json}");
        assert!(json.contains(r#""qty":0"#), "{code}: {json}");

        // A goods code keeps both.
        let report = validate(spec, &[row("01012100")], &ctx);
        assert!(report.is_clean(), "{code}: {:?}", report.findings);
        let json = generate(spec, &report.records, &ctx).to_json();
        assert!(json.contains(r#""uqc":"KGS""#), "{code}: {json}");
        assert!(json.contains(r#""qty":42"#), "{code}: {json}");
    }
}

/// The combined table's description changes source at 05-2021.
///
/// Before it, the filer's own Description reaches the payload. From it, the
/// reference overwrites the cell with the official description looked up from
/// its own HSN/SAC table (`offline2.js:388`) before anything validates or maps
/// it — so the filer's text is discarded, and unlike `hsn(b2b)` there is no
/// `user_desc` key to keep it in.
#[test]
fn the_combined_hsn_description_switches_to_the_official_lookup_at_05_2021() {
    let spec = sec("hsn");
    let row = |sheet_row: usize| {
        sparse(
            spec,
            sheet_row,
            &[
                ("HSN", "0101"),
                ("Description", "My own horses"),
                ("UQC", "NOS-NUMBERS"),
                ("Total Quantity", "10"),
                ("Total Value", "118000"),
                ("Rate", "18"),
                ("Taxable Value", "100000"),
                ("Integrated Tax Amount", "18000"),
            ],
        )
    };

    let before = common::ctx(4, 2021);
    let report = validate(spec, &[row(5)], &before);
    assert!(report.is_clean(), "{:?}", report.findings);
    let json = generate(spec, &report.records, &before).to_json();
    assert!(json.contains(r#""desc":"My own horses""#), "{json}");

    let after = common::ctx(6, 2021);
    let report = validate(spec, &[row(5)], &after);
    assert!(report.is_clean(), "{:?}", report.findings);
    let json = generate(spec, &report.records, &after).to_json();
    assert!(json.contains("LIVE HORSES"), "{json}");
    assert!(!json.contains("My own horses"), "{json}");
    // And no user_desc to fall back on.
    assert!(!json.contains("user_desc"), "{json}");
}

/// A blank Description is accepted from 05-2021: the reference fills the cell
/// from the code table before anything checks it, so its own "required" flag
/// can never fire.
#[test]
fn a_blank_description_is_accepted_because_the_reference_fills_it() {
    let spec = sec("hsn");
    let row = sparse(
        spec,
        5,
        &[
            ("HSN", "0101"),
            ("UQC", "NOS-NUMBERS"),
            ("Total Quantity", "10"),
            ("Total Value", "118000"),
            ("Rate", "18"),
            ("Taxable Value", "100000"),
            ("Integrated Tax Amount", "18000"),
        ],
    );
    let ctx = common::ctx(6, 2021);
    let report = validate(spec, &[row], &ctx);
    assert!(report.is_clean(), "{:?}", report.findings);
    assert!(
        generate(spec, &report.records, &ctx)
            .to_json()
            .contains("LIVE HORSES")
    );
}
