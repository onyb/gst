//! Guards over `spec/gstr1/diff.json` — the identity declarations `gst diff`
//! matches records by.
//!
//! The file restates each section's grouping keys in payload terms; these
//! tests keep the restatement honest: every record array the envelope can
//! emit has exactly one identity entry in file order, the identity keys agree
//! with the section's grouping declaration, and every absent-means-zero claim
//! points at a key the section spec actually omits when empty.

mod common;

use serde_json::Value;

fn load(rel: &str) -> Value {
    let text = std::fs::read_to_string(common::repo_path(rel)).expect("spec present");
    serde_json::from_str(&text).expect("spec parses")
}

fn diff_entries() -> Vec<Value> {
    load("spec/gstr1/diff.json")["sections"]
        .as_array()
        .expect("sections array")
        .clone()
}

/// (path, from_section) pairs the envelope key walk can emit, in file order —
/// the same derivation `scripts/validation_differential.py::record_paths`
/// does for the Python harness.
fn envelope_record_paths() -> Vec<(String, String)> {
    let envelope = load("spec/gstr1/upload-envelope.json");
    let mut out = Vec::new();
    for entry in envelope["keys"].as_array().expect("keys") {
        let key = entry["key"].as_str().expect("key");
        let from = entry["from"].as_str().expect("from");
        if let Some(code) = from.strip_prefix("section:") {
            out.push((key.to_owned(), code.to_owned()));
        } else if let Some(code) = from.strip_prefix("wrapped:") {
            let wrapper = entry["wrapper"].as_str().expect("wrapper");
            out.push((format!("{key}.{wrapper}"), code.to_owned()));
        } else if from == "object" {
            for member in entry["members"].as_array().expect("members") {
                let code = member["from"]
                    .as_str()
                    .and_then(|f| f.strip_prefix("section:"))
                    .expect("member section");
                out.push((
                    format!("{key}.{}", member["key"].as_str().expect("member key")),
                    code.to_owned(),
                ));
            }
        } else if from == "hsn" {
            // Both period regimes: the merged pre-bifurcation table and the
            // bifurcated pair.
            let wrapper = envelope["hsn_before_bifurcation"]["wrapper"]
                .as_str()
                .expect("hsn wrapper");
            out.push((format!("{key}.{wrapper}"), "hsn".to_owned()));
            for member in envelope["hsn_from_bifurcation"]["members"]
                .as_array()
                .expect("hsn members")
            {
                let code = member["from"]
                    .as_str()
                    .and_then(|f| f.strip_prefix("section:"))
                    .expect("hsn member section");
                out.push((
                    format!("{key}.{}", member["key"].as_str().expect("member key")),
                    code.to_owned(),
                ));
            }
        }
        // context:/literal:/empty: keys carry no records.
    }
    out
}

#[test]
fn every_envelope_record_array_has_exactly_one_identity_entry_in_file_order() {
    let expected = envelope_record_paths();
    let declared: Vec<(String, String)> = diff_entries()
        .iter()
        .map(|e| {
            (
                e["path"].as_str().expect("path").to_owned(),
                e["from_section"].as_str().expect("from_section").to_owned(),
            )
        })
        .collect();
    assert_eq!(declared, expected);
}

#[test]
fn every_registered_section_is_reachable_from_an_identity_entry() {
    let declared: Vec<String> = diff_entries()
        .iter()
        .map(|e| e["from_section"].as_str().unwrap().to_owned())
        .collect();
    for section in gst_core::spec::sections() {
        assert!(
            declared.contains(&section.section),
            "{} has no entry in spec/gstr1/diff.json",
            section.section
        );
    }
}

/// The leaf name of an identity key part ("itm_det.rt" → "rt"), whether the
/// part is a bare string or the period-gated object form.
fn leaf(part: &Value) -> String {
    let field = match part {
        Value::String(s) => s.as_str(),
        other => other["field"].as_str().expect("field"),
    };
    field.rsplit('.').next().expect("leaf").to_owned()
}

/// A section's grouping keys as the payload leaf names each diff level must
/// carry: fy+omonth compose into omon, nat_supp routes members and vanishes,
/// and an item_conflict of `append` means nothing identifies an item.
fn declared_levels(section: &Value) -> Vec<Vec<String>> {
    let grouping = &section["grouping"];
    let transform = |ids: &Value| -> Vec<String> {
        let mut out = Vec::new();
        let mut saw_period_pair = false;
        for id in ids.as_array().into_iter().flatten() {
            match id.as_str().expect("key id") {
                "fy" | "omonth" => {
                    if !saw_period_pair {
                        out.push("omon".to_owned());
                        saw_period_pair = true;
                    }
                }
                "nat_supp" => {}
                other => out.push(other.to_owned()),
            }
        }
        out
    };

    let mut levels = Vec::new();
    if !grouping["envelope_key"].is_null() {
        levels.push(transform(&grouping["envelope_key"]));
    }
    if !grouping["invoice_key"].is_null() {
        levels.push(transform(&grouping["invoice_key"]));
    }
    if !grouping["record_key"].is_null() {
        levels.push(transform(&grouping["record_key"]));
    }
    if !grouping["item_key"].is_null() {
        if section["grouping"]["item_conflict"].as_str() == Some("append") {
            levels.push(Vec::new());
        } else {
            levels.push(transform(&grouping["item_key"]));
        }
    }
    levels
}

#[test]
fn identity_keys_agree_with_the_sections_grouping_declarations() {
    for entry in diff_entries() {
        let code = entry["from_section"].as_str().unwrap();
        assert!(gst_core::spec::section(code).is_some(), "{code} registered");
        let raw = load(&format!("spec/gstr1/{}", section_file(code)));
        let declared = declared_levels(&raw);

        let levels = entry["levels"].as_array().expect("levels");
        if declared.is_empty() {
            // A section with no grouping at all (nil) declares its identity
            // here alone; there is nothing to cross-check.
            continue;
        }
        assert_eq!(
            levels.len(),
            declared.len(),
            "{}: {} levels declared vs {} in grouping",
            entry["path"],
            levels.len(),
            declared.len()
        );
        for (level, expected) in levels.iter().zip(&declared) {
            let keys: Vec<String> = level["keys"]
                .as_array()
                .expect("keys")
                .iter()
                .map(leaf)
                .collect();
            // The entry may add payload-safe keys beyond the declared ones
            // (ata/txpda add omon, backed by agree_fields) but must carry
            // every declared key.
            for want in expected {
                assert!(
                    keys.contains(want),
                    "{} level {:?} lacks grouping key {want}",
                    entry["path"],
                    level["label"]
                );
            }
            let grouping = &raw["grouping"];
            let ci = level["case_insensitive"].as_bool().unwrap_or(false);
            let declared_ci = grouping["invoice_key_case_insensitive"]
                .as_bool()
                .unwrap_or(false);
            if ci {
                assert!(
                    declared_ci,
                    "{}: case_insensitive not backed by the grouping",
                    entry["path"]
                );
            }
            let global = level["global"].as_bool().unwrap_or(false);
            if global {
                assert!(
                    grouping["invoice_key_global"].as_bool().unwrap_or(false),
                    "{}: global not backed by the grouping",
                    entry["path"]
                );
            }
        }
    }
}

/// Registry section code → spec file name under spec/gstr1/.
fn section_file(code: &str) -> String {
    match code {
        "nil" => "exemp.json".into(),
        "doc_issue" => "docs.json".into(),
        "supeco" => "eco.json".into(),
        "supecoa" => "ecoa.json".into(),
        "hsn(b2b)" => "hsn-b2b.json".into(),
        "hsn(b2c)" => "hsn-b2c.json".into(),
        code if code.starts_with("ecom") => format!("{}.json", code.replacen("ecom", "eco", 1)),
        code => format!("{code}.json"),
    }
}

#[test]
fn absent_means_zero_claims_point_at_keys_the_spec_omits_when_empty() {
    for entry in diff_entries() {
        let Some(claims) = entry["absent_means_zero"].as_array() else {
            continue;
        };
        let code = entry["from_section"].as_str().unwrap();
        let raw = load(&format!("spec/gstr1/{}", section_file(code)));
        for claim in claims {
            let leaf = claim.as_str().unwrap().rsplit('.').next().unwrap();
            assert!(
                key_omits_when_empty(&raw["output"], leaf),
                "{}: '{leaf}' claims absent-means-zero but the spec declares no \
                 omit_when_empty output key ending in it",
                entry["path"]
            );
        }
    }
}

/// Whether the output mapping anywhere declares a key ending in `leaf` (keys
/// may be dotted payload paths like `itm_det.csamt`) with `omit_when_empty`.
fn key_omits_when_empty(node: &Value, leaf: &str) -> bool {
    match node {
        Value::Object(map) => {
            let matches = map
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|k| k.rsplit('.').next() == Some(leaf))
                && map
                    .get("omit_when_empty")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            matches || map.values().any(|v| key_omits_when_empty(v, leaf))
        }
        Value::Array(items) => items.iter().any(|v| key_omits_when_empty(v, leaf)),
        _ => false,
    }
}
