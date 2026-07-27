//! Every section spec uses only vocabulary the meta-schema declares.
//!
//! Serde ignores keys it does not know, so a misspelled or invented spec key
//! costs nothing at load time and simply does nothing — a spec can claim a rule
//! the engine never applies and still deserialize cleanly. This walks the spec
//! files against `spec/section.schema.json`, which is the written contract, and
//! fails on any key that contract does not declare.
//!
//! It reads the schema rather than restating it, so the schema stays the single
//! place the vocabulary is defined.

mod common;

use std::collections::BTreeSet;

use serde_json::Value;

/// Property names declared at a schema location, when that location closes
/// itself to anything else. `None` means the schema allows extra keys there and
/// there is nothing to check.
fn declared(schema: &Value) -> Option<BTreeSet<String>> {
    if schema.get("additionalProperties") != Some(&Value::Bool(false)) {
        return None;
    }
    Some(
        schema
            .get("properties")?
            .as_object()?
            .keys()
            .cloned()
            .collect(),
    )
}

/// Resolve a local `$ref` such as `#/$defs/field`.
fn resolve<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    let Some(reference) = node.get("$ref").and_then(Value::as_str) else {
        return node;
    };
    reference
        .trim_start_matches("#/")
        .split('/')
        .fold(root, |acc, part| &acc[part])
}

/// Walk a spec value against its schema, collecting undeclared keys as
/// `path -> key`.
fn check(root: &Value, schema: &Value, value: &Value, path: &str, out: &mut Vec<String>) {
    let schema = resolve(root, schema);

    // A `oneOf` matches if any branch does; only report when none accepts the
    // value, which the per-branch walk below cannot express, so branches are
    // simply not descended into.
    if schema.get("oneOf").is_some() {
        return;
    }

    match value {
        Value::Object(fields) => {
            if let Some(allowed) = declared(schema) {
                for key in fields.keys() {
                    if !allowed.contains(key) {
                        out.push(format!("{path}.{key}"));
                    }
                }
            }
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (key, child) in fields {
                if let Some(child_schema) = properties.get(key) {
                    check(root, child_schema, child, &format!("{path}.{key}"), out);
                }
            }
        }
        Value::Array(items) => {
            let Some(item_schema) = schema.get("items") else {
                return;
            };
            for (i, item) in items.iter().enumerate() {
                check(root, item_schema, item, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_section_spec_uses_only_declared_vocabulary() {
    let schema_path = common::repo_path("spec/section.schema.json");
    let schema: Value =
        serde_json::from_str(&std::fs::read_to_string(&schema_path).expect("schema"))
            .expect("schema is valid JSON");

    let dir = common::repo_path("spec/gstr1");
    let mut checked = 0;
    let mut problems: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&dir).expect("spec/gstr1 exists") {
        let path = entry.expect("readable entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        // The envelope is a different document with its own shape.
        if !name.ends_with(".json") || name == "upload-envelope.json" {
            continue;
        }
        let spec: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("spec")).expect(name);

        let mut undeclared = Vec::new();
        check(&schema, &schema, &spec, "", &mut undeclared);
        for key in undeclared {
            problems.push(format!("{name}: {key}"));
        }
        checked += 1;
    }

    assert!(checked >= 30, "only {checked} specs found in {dir:?}");
    assert!(
        problems.is_empty(),
        "spec keys the meta-schema does not declare — serde would ignore these silently:\n  {}",
        problems.join("\n  ")
    );
}

/// The registry and the directory must agree: a spec file nobody registers is
/// dead weight, and a registered spec with no file cannot compile.
#[test]
fn every_spec_file_is_registered() {
    let dir = common::repo_path("spec/gstr1");
    let mut files: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(&dir).expect("spec/gstr1 exists") {
        let path = entry.expect("readable entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.ends_with(".json") || name == "upload-envelope.json" {
            continue;
        }
        let spec: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("spec")).expect(name);
        files.insert(
            spec["section"]
                .as_str()
                .unwrap_or_else(|| panic!("{name} has no section code"))
                .to_owned(),
        );
    }

    let registered: BTreeSet<String> = gst_core::spec::section_codes()
        .into_iter()
        .map(str::to_owned)
        .collect();

    let unregistered: Vec<&String> = files.difference(&registered).collect();
    assert!(
        unregistered.is_empty(),
        "spec files exist for these sections but nothing registers them, \
         so they are never read: {unregistered:?}"
    );
}
