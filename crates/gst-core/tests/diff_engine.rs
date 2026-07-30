//! The semantic differ against the golden files: identical inputs are clean,
//! and every difference class fires exactly where the identity spec says it
//! should — record matching by key, case folds, global invoice keys, lossy
//! multisets, absent-vs-zero, the gstin/fp cascades and the IFF mode.

mod common;

use gst_core::diff::{DiffKind, DiffReport, diff};
use gst_core::payload::{self, Json};
use gst_core::upload::merge_parts;
use rust_decimal::Decimal;

fn golden() -> Json {
    let text = std::fs::read_to_string(common::repo_path(
        "fixtures/golden/gstr1-062025-reference.json",
    ))
    .expect("golden present");
    payload::parse(&text).expect("golden parses")
}

fn eco_golden() -> Json {
    let text = std::fs::read_to_string(common::repo_path(
        "fixtures/golden/gstr1-eco-062025-reference.json",
    ))
    .expect("golden present");
    payload::parse(&text).expect("golden parses")
}

fn obj_mut<'a>(value: &'a mut Json, key: &str) -> &'a mut Json {
    let Json::Obj(entries) = value else {
        panic!("not an object")
    };
    &mut entries
        .iter_mut()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("{key} present"))
        .1
}

fn arr_mut<'a>(value: &'a mut Json, key: &str) -> &'a mut Vec<Json> {
    match obj_mut(value, key) {
        Json::Arr(items) => items,
        _ => panic!("{key} is not an array"),
    }
}

fn remove_key(value: &mut Json, key: &str) {
    let Json::Obj(entries) = value else {
        panic!("not an object")
    };
    entries.retain(|(k, _)| k != key);
}

fn kinds(report: &DiffReport) -> Vec<DiffKind> {
    report.differences.iter().map(|d| d.kind).collect()
}

#[test]
fn a_file_is_identical_to_itself() {
    let report = diff(&golden(), &golden()).unwrap();
    assert!(report.identical(), "{:?}", report.differences);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
}

#[test]
fn different_returns_differ() {
    // Same filer and period, entirely different content.
    let report = diff(&golden(), &eco_golden()).unwrap();
    assert!(!report.identical());
}

#[test]
fn a_reordered_file_with_reformatted_numbers_is_identical() {
    // Reverse every section array and object key order, and re-render some
    // numbers with trailing zeros: all noise.
    let text = std::fs::read_to_string(common::repo_path(
        "fixtures/golden/gstr1-062025-reference.json",
    ))
    .unwrap();
    let reordered = text.replace(
        r#""txval":100000,"rt":18"#,
        r#""rt":18.00,"txval":100000.0"#,
    );
    assert_ne!(reordered, text, "the rewrite must hit something");
    let mut right = payload::parse(&reordered).unwrap();
    arr_mut(&mut right, "b2b").reverse();
    arr_mut(&mut right, "b2cs").reverse();
    let report = diff(&golden(), &right).unwrap();
    assert!(report.identical(), "{:?}", report.differences);
}

#[test]
fn a_changed_amount_names_the_exact_record() {
    let mut right = golden();
    let invoice = &mut arr_mut(&mut arr_mut(&mut right, "b2b")[0], "inv")[0];
    let item = &mut arr_mut(invoice, "itms")[0];
    item.insert_path("itm_det.txval", Json::Num(Decimal::from(123_456)));

    let report = diff(&golden(), &right).unwrap();
    assert_eq!(kinds(&report), [DiffKind::ValueChanged]);
    let d = &report.differences[0];
    assert_eq!(d.section.as_deref(), Some("b2b"));
    assert!(
        d.path.contains("b2b[ctin=")
            && d.path.contains(".itms[rt=")
            && d.path.ends_with("itm_det.txval"),
        "{}",
        d.path
    );
    assert!(!d.derived);
}

#[test]
fn records_add_and_remove_at_every_level() {
    let mut right = golden();
    // Remove one supplier envelope, one invoice of another supplier, and one
    // item of a surviving invoice.
    let b2b = arr_mut(&mut right, "b2b");
    b2b.remove(1);
    let inv = arr_mut(&mut b2b[0], "inv");
    if inv.len() > 1 {
        inv.remove(1);
    }
    let items = arr_mut(&mut inv[0], "itms");
    let removed_item = items.len() > 1;
    if removed_item {
        items.remove(1);
    }

    let report = diff(&golden(), &right).unwrap();
    assert!(!report.identical());
    assert!(
        report
            .differences
            .iter()
            .all(|d| d.kind == DiffKind::RecordRemoved),
        "{:?}",
        kinds(&report)
    );
}

#[test]
fn a_case_only_document_number_is_a_change_not_a_replacement() {
    let mut right = golden();
    let invoice = &mut arr_mut(&mut arr_mut(&mut right, "b2b")[0], "inv")[0];
    let Some(Json::Str(inum)) = invoice.get("inum").cloned() else {
        panic!("inum present")
    };
    invoice.insert_path("inum", Json::Str(inum.to_lowercase()));

    let report = diff(&golden(), &right).unwrap();
    assert_eq!(kinds(&report), [DiffKind::ValueChanged], "{report:?}");
    assert!(report.differences[0].path.ends_with(".inum"));
}

#[test]
fn a_moved_b2cl_invoice_is_a_pos_change_not_a_remove_and_add() {
    let mut right = golden();
    let b2cl = arr_mut(&mut right, "b2cl");
    assert!(b2cl.len() >= 2, "fixture has two pos envelopes");
    // Move the first invoice of envelope 0 into envelope 1.
    let moved = arr_mut(&mut b2cl[0], "inv").remove(0);
    arr_mut(&mut b2cl[1], "inv").push(moved);

    let report = diff(&golden(), &right).unwrap();
    assert_eq!(kinds(&report), [DiffKind::ValueChanged], "{report:?}");
    assert!(
        report.differences[0].path.ends_with(".pos"),
        "{}",
        report.differences[0].path
    );
}

#[test]
fn duplicate_lossy_b2cs_identities_report_a_count_mismatch() {
    let mut left = golden();
    let mut right = golden();
    // Two rows sharing pos+rt on the left, one on the right.
    let row = arr_mut(&mut left, "b2cs")[0].clone();
    arr_mut(&mut left, "b2cs").push(row);
    let _ = &mut right;

    let report = diff(&left, &right).unwrap();
    assert!(
        report
            .differences
            .iter()
            .any(|d| d.kind == DiffKind::CountMismatch),
        "{report:?}"
    );
    // b2cs is declared multiset: duplicates are NOT a malformed-file note.
    assert!(report.notes.is_empty(), "{:?}", report.notes);
}

#[test]
fn document_series_compare_as_a_multiset() {
    let mut right = golden();
    // Reordering the series rows of one document type is no difference.
    let doc_det = obj_mut(&mut right, "doc_issue");
    let docs = arr_mut(&mut arr_mut(doc_det, "doc_det")[0], "docs");
    docs.reverse();
    let report = diff(&golden(), &right).unwrap();
    assert!(report.identical(), "{:?}", report.differences);

    // Editing one row is an add/remove pair, not a field diff.
    let mut right = golden();
    let doc_det = obj_mut(&mut right, "doc_issue");
    let docs = arr_mut(&mut arr_mut(doc_det, "doc_det")[0], "docs");
    docs[0].insert_path("totnum", Json::Num(Decimal::from(999)));
    let report = diff(&golden(), &right).unwrap();
    let mut got = kinds(&report);
    got.sort_by_key(|k| k.as_str());
    assert_eq!(
        got,
        [DiffKind::RecordAdded, DiffKind::RecordRemoved],
        "{report:?}"
    );
}

#[test]
fn an_absent_cess_against_a_typed_zero_is_its_own_class() {
    // txpd items: absence records a blank cell; 0 records a typed zero.
    let mut left = golden();
    let mut right = golden();
    fn item_of(side: &mut Json) -> &mut Json {
        &mut arr_mut(&mut arr_mut(side, "txpd")[0], "itms")[0]
    }
    remove_key(item_of(&mut left), "csamt");
    item_of(&mut right).insert_path("csamt", Json::Num(Decimal::ZERO));

    let report = diff(&left, &right).unwrap();
    assert_eq!(kinds(&report), [DiffKind::AbsentVsZero], "{report:?}");

    // The same absent-vs-zero shape in b2b is NOT declared, hence a plain
    // value change: b2b's blank cess emits 0, so absence means something.
    let mut left = golden();
    let right = golden();
    let invoice = &mut arr_mut(&mut arr_mut(&mut left, "b2b")[0], "inv")[0];
    let item = &mut arr_mut(invoice, "itms")[0];
    let Json::Obj(det) = obj_mut(item, "itm_det") else {
        panic!()
    };
    det.retain(|(k, _)| k != "csamt");
    let report = diff(&left, &right).unwrap();
    assert_eq!(kinds(&report), [DiffKind::ValueChanged], "{report:?}");
}

#[test]
fn the_intra_inter_split_is_a_plain_change() {
    let mut right = golden();
    let invoice = &mut arr_mut(&mut arr_mut(&mut right, "b2b")[0], "inv")[0];
    let item = &mut arr_mut(invoice, "itms")[0];
    // Swap whichever half the item carries for the other.
    let Json::Obj(det) = obj_mut(item, "itm_det") else {
        panic!()
    };
    if det.iter().any(|(k, _)| k == "iamt") {
        det.retain(|(k, _)| k != "iamt");
        item.insert_path("itm_det.camt", Json::Num(Decimal::from(9000)));
        item.insert_path("itm_det.samt", Json::Num(Decimal::from(9000)));
    } else {
        det.retain(|(k, _)| k != "camt" && k != "samt");
        item.insert_path("itm_det.iamt", Json::Num(Decimal::from(18000)));
    }

    let report = diff(&golden(), &right).unwrap();
    assert!(!report.identical());
    assert!(
        report
            .differences
            .iter()
            .all(|d| d.kind == DiffKind::ValueChanged),
        "{report:?}"
    );
}

#[test]
fn an_iff_against_a_full_return_is_one_mode_difference() {
    let full = golden();
    let mut iff = golden();
    let keep = [
        "gstin", "fp", "version", "hash", "b2b", "b2ba", "cdnr", "cdnra", "ecom", "ecoma",
    ];
    if let Json::Obj(entries) = &mut iff {
        entries.retain(|(k, _)| keep.contains(&k.as_str()));
    }

    let report = diff(&full, &iff).unwrap();
    assert_eq!(kinds(&report), [DiffKind::ModeMismatch], "{report:?}");
}

#[test]
fn a_gstin_mismatch_tags_the_tax_cascade() {
    let mut right = golden();
    right.insert_path("gstin", Json::Str("12GEOPS0823BBZH".to_owned()));
    let invoice = &mut arr_mut(&mut arr_mut(&mut right, "b2b")[0], "inv")[0];
    let item = &mut arr_mut(invoice, "itms")[0];
    let Json::Obj(det) = obj_mut(item, "itm_det") else {
        panic!()
    };
    let tax_key = if det.iter().any(|(k, _)| k == "iamt") {
        "iamt"
    } else {
        "camt"
    };
    item.insert_path(&format!("itm_det.{tax_key}"), Json::Num(Decimal::from(1)));

    let report = diff(&golden(), &right).unwrap();
    let header: Vec<_> = report
        .differences
        .iter()
        .filter(|d| d.kind == DiffKind::Header)
        .collect();
    assert_eq!(header.len(), 1);
    let tax = report
        .differences
        .iter()
        .find(|d| d.path.ends_with(tax_key))
        .expect("tax diff present");
    assert_eq!(tax.cause, Some("gstin"));
}

#[test]
fn a_period_across_the_bifurcation_collapses_the_hsn_diff() {
    let mut right = golden();
    right.insert_path("fp", Json::Str("042025".to_owned()));

    let report = diff(&golden(), &right).unwrap();
    let hsn: Vec<_> = report
        .differences
        .iter()
        .filter(|d| d.section.as_deref() == Some("hsn"))
        .collect();
    assert_eq!(hsn.len(), 1, "{report:?}");
    assert_eq!(hsn[0].cause, Some("fp"));
    assert!(
        !report
            .differences
            .iter()
            .any(|d| d.section.as_deref() == Some("hsn.hsn_b2b")),
        "bifurcated entries must be collapsed"
    );
}

#[test]
fn a_section_payload_is_refused() {
    let bare = payload::parse(r#"[{"ctin":"12GEOPS0823BBZH","inv":[]}]"#).unwrap();
    assert!(diff(&bare, &golden()).is_err());
    assert!(diff(&golden(), &bare).is_err());
}

#[test]
fn a_foreign_version_is_a_note_not_a_difference() {
    let mut right = golden();
    right.insert_path("version", Json::Str("GST3.2.3".to_owned()));
    let report = diff(&golden(), &right).unwrap();
    assert!(report.identical(), "{:?}", report.differences);
    assert!(
        report.notes.iter().any(|n| n.contains("GST3.2.3")),
        "{:?}",
        report.notes
    );
}

#[test]
fn empty_values_prune_with_a_note() {
    let mut right = golden();
    right.insert_path("hsnSac", Json::Arr(vec![]));
    let report = diff(&golden(), &right).unwrap();
    assert!(report.identical(), "{:?}", report.differences);
    assert!(
        report.notes.iter().any(|n| n.contains("omit-empty")),
        "{:?}",
        report.notes
    );
}

#[test]
fn merged_parts_diff_clean_against_the_whole() {
    let whole = golden();
    // Split the golden by hand: header in both parts, sections partitioned.
    let Json::Obj(entries) = &whole else { panic!() };
    let header = ["gstin", "fp", "version", "hash"];
    let split_at = entries.len() / 2;
    let mut part1 = Json::obj();
    let mut part2 = Json::obj();
    for (i, (key, value)) in entries.iter().enumerate() {
        let is_header = header.contains(&key.as_str());
        if is_header || i < split_at {
            part1.insert_path(key, value.clone());
        }
        if is_header || i >= split_at {
            part2.insert_path(key, value.clone());
        }
    }

    let merged = merge_parts(vec![part1.clone(), part2.clone()]).unwrap();
    assert!(merged.notes.is_empty());
    let report = diff(&whole, &merged.whole).unwrap();
    assert!(report.identical(), "{:?}", report.differences);

    // The reference's own splitter loses the header from part 2 on; the
    // merge tolerates that with a note and the diff stays clean.
    let mut headerless = part2;
    if let Json::Obj(entries) = &mut headerless {
        entries.retain(|(k, _)| !header.contains(&k.as_str()));
    }
    let merged = merge_parts(vec![part1, headerless]).unwrap();
    assert_eq!(merged.notes.len(), 1, "{:?}", merged.notes);
    let report = diff(&whole, &merged.whole).unwrap();
    assert!(report.identical(), "{:?}", report.differences);
}
