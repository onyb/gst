//! End-to-end e-commerce sections.
//!
//! Ten sheets feeding four payload objects, and the least uniform corner of the
//! return. What is exercised here is what makes them different from everything
//! else: one sheet routed into two members by a field value, both tax halves
//! emitted at once, a `flag` no column supplies, and a `posItms` nesting level
//! nothing else uses.

use std::collections::HashMap;

use gst_core::date::ReturnPeriod;
use gst_core::generate::{Generated, generate};
use gst_core::record::Row;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::upload::{self, Turnover};
use gst_core::validate::{FilingContext, validate};

fn sec(code: &str) -> &'static SectionSpec {
    spec::section(code).unwrap_or_else(|| panic!("{code} is registered"))
}

fn ctx() -> FilingContext {
    FilingContext {
        supplier_gstin: "27AAPFU0939F1ZV".into(),
        period: ReturnPeriod::new(6, 2025).unwrap(),
        is_sez: false,
        aato_over_5cr: false,
    }
}

fn row(columns: &[&str], values: &[&str]) -> Row {
    Row::from_pairs(5, columns.iter().copied().zip(values.iter().copied()))
}

fn run(code: &str, rows: &[Row]) -> Generated {
    let spec = sec(code);
    let report = validate(spec, rows, &ctx());
    assert!(report.is_clean(), "{code} validation: {:?}", report.findings);
    let out = generate(spec, &report.records, &ctx());
    assert!(
        !out.findings.iter().any(|f| f.severity == Severity::Error),
        "{code} generation: {:?}",
        out.findings
    );
    out
}

const ECO: [&str; 8] = [
    "Nature of Supply",
    "GSTIN of E-Commerce Operator",
    "E-Commerce Operator Name",
    "Net value of supplies",
    "Integrated tax",
    "Central tax",
    "State/UT tax",
    "Cess",
];

fn eco_row(nature: &str, value: &str) -> Row {
    row(
        &ECO,
        &[
            nature,
            "12AJIPA1572E1C7",
            "Acme Marketplace",
            value,
            "18000",
            "0",
            "0",
            "0",
        ],
    )
}

#[test]
fn every_derivation_the_eco_specs_name_is_implemented() {
    for code in [
        "supeco",
        "supecoa",
        "ecomb2b",
        "ecomb2c",
        "ecomurp2b",
        "ecomurp2c",
        "ecomab2b",
        "ecomab2c",
        "ecomaurp2b",
        "ecomaurp2c",
    ] {
        assert!(
            gst_core::generate::unimplemented_derivations(sec(code)).is_empty(),
            "{code} names a derivation the engine does not implement"
        );
    }
}

#[test]
fn one_sheet_routes_its_rows_into_two_payload_members() {
    let out = run(
        "supeco",
        &[
            eco_row("Liable to collect tax u/s 52(TCS)", "100000"),
            eco_row("Liable to pay tax u/s 9(5)", "50000"),
        ],
    );
    assert_eq!(out.members, [Some("clttx".into()), Some("paytx".into())]);

    // The routing column is not itself emitted.
    let json = out.to_json();
    assert!(!json.contains("nat_supp"), "{json}");
    assert!(!json.contains("Nature"), "{json}");

    // And the envelope puts each row under its own member.
    let mut sections = HashMap::new();
    sections.insert("supeco".to_string(), out);
    let file = upload::build(&sections, &ctx(), Turnover::default()).to_json();
    assert!(
        file.contains(r#""supeco":{"clttx":[{"etin":"12AJIPA1572E1C7","suppval":100000"#),
        "{file}"
    );
    assert!(file.contains(r#""paytx":[{"etin":"12AJIPA1572E1C7","suppval":50000"#), "{file}");
}

#[test]
fn an_unknown_nature_of_supply_is_rejected() {
    let report = validate(sec("supeco"), &[eco_row("Liable to something", "1000")], &ctx());
    assert!(
        report
            .errors()
            .any(|f| f.column.as_deref() == Some("Nature of Supply")),
        "{:?}",
        report.findings
    );
}

#[test]
fn the_operator_name_never_reaches_the_payload() {
    let json = run("supeco", &[eco_row("Liable to pay tax u/s 9(5)", "50000")]).to_json();
    assert!(!json.contains("cname"), "{json}");
    assert!(!json.contains("Acme"), "{json}");
}

const B2C: [&str; 6] = [
    "Supplier GSTIN/UIN",
    "Supplier Name",
    "Place Of Supply",
    "Taxable Value",
    "Rate",
    "Cess Amount",
];

#[test]
fn the_unregistered_tables_emit_both_tax_halves_at_once() {
    // Every other section emits either iamt or camt+samt. These emit all three.
    let inter = run(
        "ecomb2c",
        &[row(
            &B2C,
            &[
                "29AAPFU0939F1ZR",
                "Seller Ltd",
                "37-Andhra Pradesh",
                "50000",
                "18",
                "0",
            ],
        )],
    )
    .to_json();
    assert!(inter.contains(r#""sply_ty":"INTER""#), "{inter}");
    assert!(inter.contains(r#""camt":0,"samt":0,"iamt":9000"#), "{inter}");

    let intra = run(
        "ecomb2c",
        &[row(
            &B2C,
            &[
                "29AAPFU0939F1ZR",
                "Seller Ltd",
                "27-Maharashtra",
                "50000",
                "18",
                "0",
            ],
        )],
    )
    .to_json();
    assert!(intra.contains(r#""sply_ty":"INTRA""#), "{intra}");
    assert!(intra.contains(r#""camt":4500,"samt":4500,"iamt":0"#), "{intra}");
}

#[test]
fn every_e_commerce_record_carries_a_flag_no_column_supplies() {
    let json = run(
        "ecomb2c",
        &[row(
            &B2C,
            &[
                "29AAPFU0939F1ZR",
                "Seller Ltd",
                "37-Andhra Pradesh",
                "50000",
                "18",
                "0",
            ],
        )],
    )
    .to_json();
    assert!(json.ends_with(r#""flag":"N"}]"#), "{json}");
}

#[test]
fn the_amended_b2c_table_nests_under_pos_items() {
    let columns = [
        "Financial Year",
        "Original Month",
        "Supplier GSTIN/UIN",
        "Supplier Name",
        "Place Of Supply",
        "Rate",
        "Taxable Value",
        "Cess Amount",
    ];
    let json = run(
        "ecomab2c",
        &[row(
            &columns,
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
        )],
    )
    .to_json();
    assert_eq!(
        json,
        r#"[{"pos":"37","posItms":[{"sply_ty":"INTER","omon":"072017","stin":"29AAPFU0939F1ZR","itms":[{"rt":18,"txval":50000,"iamt":9000,"csamt":0}],"flag":"N","ostin":"29AAPFU0939F1ZR"}]}]"#
    );
}

#[test]
fn the_amended_tables_disagree_on_key_order() {
    // Registered supplier: sply_ty leads, original document number first.
    let b2b = [
        "Supplier GSTIN/UIN",
        "Supplier Name",
        "Recipient GSTIN/UIN",
        "Recipient Name",
        "Original Document Number",
        "Original Document Date",
        "Revised Document Number",
        "Revised Document Date",
        "Value of supplies made",
        "Place Of Supply",
        "Document type",
        "Rate",
        "Taxable Value",
        "Cess Amount",
    ];
    let json = run(
        "ecomab2b",
        &[row(
            &b2b,
            &[
                "29AAPFU0939F1ZR",
                "Seller Ltd",
                "12GEOPS0823BBZH",
                "Buyer Ltd",
                "ECO-001",
                "14-Jul-2017",
                "ECO-001-R",
                "20-Jul-2017",
                "118000",
                "37-Andhra Pradesh",
                "Invoice",
                "18",
                "100000",
                "0",
            ],
        )],
    )
    .to_json();
    let sply = json.find("sply_ty").expect("sply_ty");
    assert!(sply < json.find("oinum").expect("oinum"), "{json}");
    assert!(json.find("oinum") < json.find("inum"), "{json}");

    // Unregistered supplier: revised number first, sply_ty last of all.
    let urp = [
        "Recipient GSTIN/UIN",
        "Recipient Name",
        "Original Document Number",
        "Original Document Date",
        "Revised Document Number",
        "Revised Document Date",
        "Value of supplies made",
        "Document type",
        "Place Of Supply",
        "Rate",
        "Taxable Value",
        "Cess Amount",
    ];
    let json = run(
        "ecomaurp2b",
        &[row(
            &urp,
            &[
                "12GEOPS0823BBZH",
                "Buyer Ltd",
                "ECO-002",
                "15-Jul-2017",
                "ECO-002-R",
                "21-Jul-2017",
                "59000",
                "Invoice",
                "37-Andhra Pradesh",
                "18",
                "50000",
                "0",
            ],
        )],
    )
    .to_json();
    assert!(json.find("inum") < json.find("oinum"), "{json}");
    assert!(json.find("flag") < json.find("sply_ty"), "{json}");
}

#[test]
fn a_blank_cess_is_accepted_and_emitted_as_zero() {
    // The reference rejects it as a missing header column; we default it,
    // producing the payload the reference gives an explicit 0.
    let json = run(
        "ecomurp2c",
        &[row(
            &["Place Of Supply", "Taxable Value", "Rate", "Cess Amount"],
            &["37-Andhra Pradesh", "50000", "18", ""],
        )],
    )
    .to_json();
    assert!(json.contains(r#""csamt":0"#), "{json}");
}

#[test]
fn the_shipped_eco_workbook_reproduces_the_captured_reference() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workbook = root.join("fixtures/gstr1/eco-workbook.xlsx");

    let mut sections: HashMap<String, Generated> = HashMap::new();
    for spec in spec::sections() {
        let rows = match gst_core::import::read(&workbook, spec) {
            Ok(rows) => rows,
            Err(gst_core::import::ImportError::SheetMissing { .. }) => continue,
            Err(e) => panic!("reading {}: {e}", spec.section),
        };
        if rows.is_empty() {
            continue;
        }
        sections.insert(spec.section.clone(), run(&spec.section, &rows));
    }
    assert_eq!(sections.len(), 10, "all ten ECO sheets should be read");

    let ours = upload::build(&sections, &ctx(), Turnover::default()).to_json();
    let golden =
        std::fs::read_to_string(root.join("fixtures/golden/gstr1-eco-062025-reference.json"))
            .expect("the ECO reference capture is present");
    assert_eq!(ours, golden.trim_end());
}
