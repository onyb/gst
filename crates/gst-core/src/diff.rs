//! Semantic comparison of two portal upload files.
//!
//! Byte comparison already exists (the goldens own that contract); this
//! answers the different question of whether two files SAY the same thing.
//! Records are matched by the identities `spec/gstr1/diff.json` declares —
//! the section specs' grouping keys restated in payload terms — so a
//! reordered array, a reformatted number or a reordered key is no difference,
//! while a changed value names the exact record it changed in.

use std::collections::HashMap;

use serde::Deserialize;
use std::sync::LazyLock;

use crate::payload::Json;
use crate::spec::{period_as_yyyymm, period_window};
use crate::upload;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum KeyPart {
    Name(String),
    Gated {
        field: String,
        #[serde(default)]
        from_period: Option<String>,
        #[serde(default)]
        until_period: Option<String>,
    },
}

impl KeyPart {
    fn field(&self) -> &str {
        match self {
            KeyPart::Name(name) => name,
            KeyPart::Gated { field, .. } => field,
        }
    }

    fn active_for(&self, period: Option<u32>) -> bool {
        match (self, period) {
            (KeyPart::Name(_), _) => true,
            (KeyPart::Gated { .. }, None) => true,
            (
                KeyPart::Gated {
                    from_period,
                    until_period,
                    ..
                },
                Some(period),
            ) => period_window(from_period, until_period, period),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Level {
    label: String,
    #[serde(default)]
    array: Option<String>,
    keys: Vec<KeyPart>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    global: bool,
    #[serde(default)]
    multiset: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    path: String,
    levels: Vec<Level>,
    #[serde(default)]
    derived: Vec<String>,
    #[serde(default)]
    absent_means_zero: Vec<String>,
    #[serde(default)]
    from_period: Option<String>,
    #[serde(default)]
    until_period: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DiffSpec {
    sections: Vec<Entry>,
}

static DIFF: LazyLock<DiffSpec> = LazyLock::new(|| {
    crate::masters::embedded(
        "gstr1/diff.json",
        include_str!("../../../spec/gstr1/diff.json"),
    )
});

/// What sort of difference one entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// gstin/fp/gt/cur_gt disagree.
    Header,
    /// One side is an IFF (quarterly months 1-2) and the other a full return.
    ModeMismatch,
    /// A record exists on the right only.
    RecordAdded,
    /// A record exists on the left only.
    RecordRemoved,
    /// A value differs inside a matched record.
    ValueChanged,
    /// A key absent on one side against an explicit 0 on the other, where the
    /// spec says absence records a blank cell — tax-neutral but real.
    AbsentVsZero,
    /// Records sharing a lossy identity occur a different number of times.
    CountMismatch,
}

impl DiffKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DiffKind::Header => "header",
            DiffKind::ModeMismatch => "mode-mismatch",
            DiffKind::RecordAdded => "record-added",
            DiffKind::RecordRemoved => "record-removed",
            DiffKind::ValueChanged => "value-changed",
            DiffKind::AbsentVsZero => "absent-vs-zero",
            DiffKind::CountMismatch => "count-mismatch",
        }
    }
}

/// One semantic difference, locating the exact record and key it lives at.
#[derive(Debug, Clone)]
pub struct Difference {
    /// The diff.json entry path (`b2b`, `hsn.hsn_b2b`…); None for the header.
    pub section: Option<String>,
    /// Locator such as `b2b[ctin=27..].inv[inum=INV-1].itms[rt=18].itm_det.txval`.
    pub path: String,
    pub kind: DiffKind,
    /// The left side's value, rendered as compact JSON; None when one-sided.
    pub left: Option<String>,
    pub right: Option<String>,
    /// The key is computed from other values (num, sply_ty, doc_num…) — a
    /// consequence, not an independent edit.
    pub derived: bool,
    /// A header cause this difference follows from ("gstin", "fp"), if any.
    pub cause: Option<&'static str>,
}

/// The comparison's outcome: differences decide the exit code, notes never do.
#[derive(Debug, Clone, Default)]
pub struct DiffReport {
    pub differences: Vec<Difference>,
    /// Foreign-file signals and tolerated anomalies.
    pub notes: Vec<String>,
}

impl DiffReport {
    pub fn identical(&self) -> bool {
        self.differences.is_empty()
    }
}

/// Why two inputs could not be compared at all.
#[derive(Debug)]
pub enum DiffError {
    /// The input is not a whole upload file (a bare section payload, say).
    NotAnUpload(String),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::NotAnUpload(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DiffError {}

/// Compare two upload files semantically.
pub fn diff(left: &Json, right: &Json) -> Result<DiffReport, DiffError> {
    let mut report = DiffReport::default();

    for (name, side) in [("left", left), ("right", right)] {
        if !matches!(side, Json::Obj(_)) || side.get("gstin").is_none() || side.get("fp").is_none()
        {
            return Err(DiffError::NotAnUpload(format!(
                "the {name} file carries no gstin/fp header — it looks like a section payload, \
                 not a whole upload file; gst diff compares whole portal upload files"
            )));
        }
    }

    // Absent, "", null, [] and recursively-empty objects are all the same
    // thing under the reference's omit-empty; a file that NEEDED pruning was
    // not written by the reference or by this tool.
    let mut left = left.clone();
    let mut right = right.clone();
    for (name, side) in [("left", &mut left), ("right", &mut right)] {
        let before = side.to_json();
        upload::prune_empty(side);
        if side.to_json() != before {
            report.notes.push(format!(
                "the {name} file carries empty values the reference's omit-empty would have \
                 dropped — not written by the reference tool or gst"
            ));
        }
    }

    let header = compare_header(&left, &right, &mut report);

    // One side an IFF, the other a full return: the missing sections are a
    // filing-mode consequence, reported once.
    let keep = upload::iff_keep_keys();
    let left_iff = iff_shaped(&left, keep);
    let right_iff = iff_shaped(&right, keep);
    let mode_mismatch = left_iff != right_iff;
    if mode_mismatch {
        let (iff_side, _) = if left_iff {
            ("left", &left)
        } else {
            ("right", &right)
        };
        report.differences.push(Difference {
            section: None,
            path: "return mode".to_owned(),
            kind: DiffKind::ModeMismatch,
            left: Some(if left_iff { "IFF" } else { "full return" }.to_owned()),
            right: Some(if right_iff { "IFF" } else { "full return" }.to_owned()),
            derived: false,
            cause: None,
        });
        report.notes.push(format!(
            "the {iff_side} file is IFF-shaped (quarterly, months 1-2): sections outside \
             B2B/B2BA/CDNR/CDNRA/ECOM/ECOMA are absent by filing mode, not deleted"
        ));
    }

    for entry in &DIFF.sections {
        if !period_window(
            &entry.from_period,
            &entry.until_period,
            header.left_period.unwrap_or(0),
        ) && header.left_period.is_some()
        {
            continue;
        }
        if header.hsn_crossing && entry.path.starts_with("hsn.") {
            continue; // collapsed into one caused difference already
        }
        let top = entry.path.split('.').next().expect("path");
        if mode_mismatch && !keep.iter().any(|k| k == top) {
            continue;
        }
        let left_records = records_at(&left, &entry.path);
        let right_records = records_at(&right, &entry.path);
        if left_records.is_empty() && right_records.is_empty() {
            continue;
        }
        let cmp = EntryCompare {
            entry,
            period: header.left_period,
            gstin_mismatch: header.gstin_mismatch,
            report: &mut report,
        };
        cmp.run(&left_records, &right_records);
    }

    compare_unknown_keys(&left, &right, &mut report);

    Ok(report)
}

struct HeaderOutcome {
    gstin_mismatch: bool,
    hsn_crossing: bool,
    left_period: Option<u32>,
}

fn compare_header(left: &Json, right: &Json, report: &mut DiffReport) -> HeaderOutcome {
    let mut gstin_mismatch = false;
    for key in ["gstin", "fp", "gt", "cur_gt"] {
        let l = left.get(key);
        let r = right.get(key);
        if json_eq(l, r) {
            continue;
        }
        if key == "gstin" {
            gstin_mismatch = true;
        }
        report.differences.push(Difference {
            section: None,
            path: key.to_owned(),
            kind: DiffKind::Header,
            left: l.map(Json::to_json),
            right: r.map(Json::to_json),
            derived: false,
            cause: None,
        });
    }
    if gstin_mismatch {
        report.notes.push(
            "the files carry different filer GSTINs — tax-split and supply-type differences \
             marked (follows gstin) are the expected intra/inter-state cascade"
                .to_owned(),
        );
    }

    let (version, hash) = upload::envelope_literals();
    for (key, expected) in [("version", version), ("hash", hash)] {
        for (name, side) in [("left", left), ("right", right)] {
            if let Some(Json::Str(actual)) = side.get(key)
                && actual != expected
            {
                report.notes.push(format!(
                    "the {name} file carries {key} \"{actual}\" (expected \"{expected}\") — \
                     written by a different tool release?"
                ));
            }
        }
    }

    let period_of = |side: &Json| match side.get("fp") {
        Some(Json::Str(fp)) => period_as_yyyymm(fp),
        _ => None,
    };
    let left_period = period_of(left);
    let right_period = period_of(right);
    let bifurcation = period_as_yyyymm("052025").expect("constant parses");
    let hsn_crossing = match (left_period, right_period) {
        (Some(l), Some(r)) => (l < bifurcation) != (r < bifurcation),
        _ => false,
    };
    if hsn_crossing {
        report.differences.push(Difference {
            section: Some("hsn".to_owned()),
            path: "hsn".to_owned(),
            kind: DiffKind::ValueChanged,
            left: Some(hsn_shape(left_period.expect("crossing"), bifurcation)),
            right: Some(hsn_shape(right_period.expect("crossing"), bifurcation)),
            derived: false,
            cause: Some("fp"),
        });
    }
    HeaderOutcome {
        gstin_mismatch,
        hsn_crossing,
        left_period,
    }
}

fn hsn_shape(period: u32, bifurcation: u32) -> String {
    if period < bifurcation {
        "pre-05-2025 merged shape (hsn.data)".to_owned()
    } else {
        "bifurcated shape (hsn_b2b/hsn_b2c)".to_owned()
    }
}

fn iff_shaped(side: &Json, keep: &[String]) -> bool {
    let Json::Obj(entries) = side else {
        return false;
    };
    let header = upload::header_keys();
    entries.iter().all(|(k, _)| keep.iter().any(|kk| kk == k))
        && entries.iter().any(|(k, _)| !header.contains(&k.as_str()))
}

/// The record array at a dotted path; absent means empty per omit-empty.
fn records_at(side: &Json, path: &str) -> Vec<Json> {
    match side.get(path) {
        Some(Json::Arr(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn json_eq(left: Option<&Json>, right: Option<&Json>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(l), Some(r)) => value_eq(l, r),
        _ => false,
    }
}

/// Semantic equality: numbers by value, objects by key set regardless of
/// order, arrays element-wise in order.
fn value_eq(left: &Json, right: &Json) -> bool {
    match (left, right) {
        (Json::Obj(l), Json::Obj(r)) => {
            l.len() == r.len()
                && l.iter().all(|(k, lv)| {
                    r.iter()
                        .find(|(rk, _)| rk == k)
                        .is_some_and(|(_, rv)| value_eq(lv, rv))
                })
        }
        (Json::Arr(l), Json::Arr(r)) => {
            l.len() == r.len() && l.iter().zip(r).all(|(lv, rv)| value_eq(lv, rv))
        }
        (l, r) => l == r,
    }
}

/// Canonical rendering for multiset matching: object keys sorted, numbers
/// normalized — two records serialize equally iff they are semantically equal.
fn canonical(value: &Json) -> String {
    match value {
        Json::Obj(entries) => {
            let mut parts: Vec<(String, String)> = entries
                .iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect();
            parts.sort();
            let body: Vec<String> = parts
                .into_iter()
                .map(|(k, v)| format!("\"{k}\":{v}"))
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Json::Arr(items) => {
            let body: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", body.join(","))
        }
        leaf => leaf.to_json(),
    }
}

struct EntryCompare<'a> {
    entry: &'a Entry,
    period: Option<u32>,
    gstin_mismatch: bool,
    report: &'a mut DiffReport,
}

impl EntryCompare<'_> {
    fn run(mut self, left: &[Json], right: &[Json]) {
        // A `global` child level identifies across the whole section: flatten
        // one grouping level away and demote the parent's keys to compared
        // fields, so a moved invoice is a changed pos, not remove-plus-add.
        if self.entry.levels.len() >= 2 && self.entry.levels[1].global {
            let parent = &self.entry.levels[0];
            let child_array = self.entry.levels[1]
                .array
                .as_deref()
                .expect("global level names its array");
            let flatten = |records: &[Json]| -> Vec<Json> {
                let mut out = Vec::new();
                for envelope in records {
                    let parent_keys: Vec<(String, Json)> = parent
                        .keys
                        .iter()
                        .map(|k| {
                            (
                                k.field().to_owned(),
                                envelope.get(k.field()).cloned().unwrap_or(Json::Null),
                            )
                        })
                        .collect();
                    if let Some(Json::Arr(children)) = envelope.get(child_array) {
                        for child in children {
                            // Prepend the parent key as a compared field.
                            let mut merged = child.clone();
                            if let Json::Obj(entries) = &mut merged {
                                for (k, v) in parent_keys.iter().rev() {
                                    entries.insert(0, (k.clone(), v.clone()));
                                }
                            }
                            out.push(merged);
                        }
                    }
                }
                out
            };
            let left_flat = flatten(left);
            let right_flat = flatten(right);
            let levels: Vec<Level> = self.entry.levels[1..].to_vec();
            let locator = format!("{}[].{}", self.entry.path, child_array);
            self.compare_level(&levels, 0, &left_flat, &right_flat, &locator, "");
            return;
        }
        let levels = self.entry.levels.clone();
        let path = self.entry.path.clone();
        self.compare_level(&levels, 0, left, right, &path, "");
    }

    fn compare_level(
        &mut self,
        levels: &[Level],
        depth: usize,
        left: &[Json],
        right: &[Json],
        locator: &str,
        rel: &str,
    ) {
        let level = &levels[depth];
        let keys: Vec<&KeyPart> = level
            .keys
            .iter()
            .filter(|k| k.active_for(self.period))
            .collect();

        let tuple_of = |record: &Json| -> String {
            if keys.is_empty() {
                return canonical(record);
            }
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    let value = record.get(k.field()).map(Json::to_json).unwrap_or_default();
                    if level.case_insensitive {
                        value.to_uppercase()
                    } else {
                        value
                    }
                })
                .collect();
            parts.join("\u{1f}")
        };

        // Group both sides by identity tuple, keeping first-seen order.
        let group = |records: &[Json]| -> (Vec<String>, HashMap<String, Vec<Json>>) {
            let mut order = Vec::new();
            let mut map: HashMap<String, Vec<Json>> = HashMap::new();
            for record in records {
                let tuple = tuple_of(record);
                if !map.contains_key(&tuple) {
                    order.push(tuple.clone());
                }
                map.entry(tuple).or_default().push(record.clone());
            }
            (order, map)
        };
        let (left_order, left_map) = group(left);
        let (right_order, mut right_map) = group(right);

        let describe = |record: &Json| -> String {
            let parts: Vec<String> = keys
                .iter()
                .filter_map(|k| {
                    let value = record.get(k.field())?.to_json();
                    Some(format!(
                        "{}={}",
                        k.field().rsplit('.').next().expect("leaf"),
                        value.trim_matches('"')
                    ))
                })
                .collect();
            parts.join(",")
        };

        for tuple in &left_order {
            let l = &left_map[tuple];
            match right_map.remove(tuple) {
                None => {
                    for record in l {
                        self.push(
                            format!("{locator}[{}]", describe(record)),
                            DiffKind::RecordRemoved,
                            Some(record.to_json()),
                            None,
                            rel,
                            "",
                        );
                    }
                }
                Some(r) if l.len() == 1 && r.len() == 1 => {
                    let here = format!("{locator}[{}]", describe(&l[0]));
                    self.compare_record(levels, depth, &l[0], &r[0], &here, rel, &keys, level);
                }
                Some(r) => {
                    // A lossy identity with multiple holders: match whole
                    // records canonically; report the rest as adds/removes.
                    if !level.multiset {
                        // Only the declared-lossy identities may repeat.
                        self.report.notes.push(format!(
                            "duplicate {} identity at {locator}[{}] in a unique-keyed \
                             section — foreign or malformed file?",
                            level.label,
                            describe(&l[0]),
                        ));
                    }
                    if l.len() != r.len() {
                        self.push(
                            format!("{locator}[{}]", describe(&l[0])),
                            DiffKind::CountMismatch,
                            Some(l.len().to_string()),
                            Some(r.len().to_string()),
                            rel,
                            "",
                        );
                    }
                    let mut right_canon: Vec<(String, Json)> =
                        r.iter().map(|x| (canonical(x), x.clone())).collect();
                    for record in l {
                        let canon = canonical(record);
                        if let Some(at) = right_canon.iter().position(|(c, _)| *c == canon) {
                            right_canon.remove(at);
                        } else {
                            self.push(
                                format!("{locator}[{}]", describe(record)),
                                DiffKind::RecordRemoved,
                                Some(record.to_json()),
                                None,
                                rel,
                                "",
                            );
                        }
                    }
                    for (_, record) in right_canon {
                        self.push(
                            format!("{locator}[{}]", describe(&record)),
                            DiffKind::RecordAdded,
                            None,
                            Some(record.to_json()),
                            rel,
                            "",
                        );
                    }
                }
            }
        }
        for tuple in &right_order {
            if let Some(r) = right_map.remove(tuple) {
                for record in r {
                    self.push(
                        format!("{locator}[{}]", describe(&record)),
                        DiffKind::RecordAdded,
                        None,
                        Some(record.to_json()),
                        rel,
                        "",
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compare_record(
        &mut self,
        levels: &[Level],
        depth: usize,
        left: &Json,
        right: &Json,
        locator: &str,
        rel: &str,
        keys: &[&KeyPart],
        level: &Level,
    ) {
        let child = levels.get(depth + 1).and_then(|l| l.array.as_deref());
        let (Json::Obj(l_entries), Json::Obj(r_entries)) = (left, right) else {
            if !value_eq(left, right) {
                self.push(
                    locator.to_owned(),
                    DiffKind::ValueChanged,
                    Some(left.to_json()),
                    Some(right.to_json()),
                    rel,
                    "",
                );
            }
            return;
        };

        let mut names: Vec<&str> = l_entries.iter().map(|(k, _)| k.as_str()).collect();
        for (k, _) in r_entries {
            if !names.contains(&k.as_str()) {
                names.push(k);
            }
        }

        for name in names {
            if Some(name) == child {
                let empty = Vec::new();
                let as_arr = |side: &Json| match side.get(name) {
                    Some(Json::Arr(items)) => items.clone(),
                    _ => empty.clone(),
                };
                let next_rel = if rel.is_empty() {
                    name.to_owned()
                } else {
                    format!("{rel}.{name}")
                };
                self.compare_level(
                    levels,
                    depth + 1,
                    &as_arr(left),
                    &as_arr(right),
                    &format!("{locator}.{name}"),
                    &next_rel,
                );
                continue;
            }
            // Identity fields matched by construction — except a fold: a
            // case-insensitive key can still differ in its raw text.
            let is_key = keys.iter().any(|k| k.field() == name);
            if is_key && !level.case_insensitive {
                continue;
            }
            self.compare_value(
                left.get(name),
                right.get(name),
                &format!("{locator}.{name}"),
                rel,
                name,
            );
        }
    }

    fn compare_value(
        &mut self,
        left: Option<&Json>,
        right: Option<&Json>,
        locator: &str,
        rel: &str,
        name: &str,
    ) {
        match (left, right) {
            (None, None) => {}
            (Some(Json::Obj(l)), Some(Json::Obj(r))) => {
                let mut names: Vec<&str> = l.iter().map(|(k, _)| k.as_str()).collect();
                for (k, _) in r {
                    if !names.contains(&k.as_str()) {
                        names.push(k);
                    }
                }
                let (l, r) = (Json::Obj(l.clone()), Json::Obj(r.clone()));
                let next_rel = self.joined(rel, name);
                for inner in names {
                    self.compare_value(
                        l.get(inner),
                        r.get(inner),
                        &format!("{locator}.{inner}"),
                        &next_rel,
                        inner,
                    );
                }
            }
            (l, r) if json_eq(l, r) => {}
            (l, r) => {
                let full_rel = self.joined(rel, name);
                let zero = Json::Num(rust_decimal::Decimal::ZERO);
                let absent_zero = self.entry.absent_means_zero.contains(&full_rel)
                    && matches!(
                        (l, r),
                        (None, Some(v)) | (Some(v), None) if value_eq(v, &zero)
                    );
                let kind = if absent_zero {
                    DiffKind::AbsentVsZero
                } else {
                    DiffKind::ValueChanged
                };
                self.push(
                    locator.to_owned(),
                    kind,
                    l.map(Json::to_json),
                    r.map(Json::to_json),
                    rel,
                    name,
                );
            }
        }
    }

    fn joined(&self, rel: &str, name: &str) -> String {
        if rel.is_empty() {
            name.to_owned()
        } else {
            format!("{rel}.{name}")
        }
    }

    fn push(
        &mut self,
        path: String,
        kind: DiffKind,
        left: Option<String>,
        right: Option<String>,
        rel: &str,
        name: &str,
    ) {
        let full_rel = if name.is_empty() {
            rel.to_owned()
        } else {
            self.joined(rel, name)
        };
        let derived = self.entry.derived.contains(&full_rel);
        let leaf = name.rsplit('.').next().unwrap_or(name);
        let cause = if self.gstin_mismatch
            && matches!(leaf, "iamt" | "camt" | "samt" | "sply_ty")
            && kind == DiffKind::ValueChanged
        {
            Some("gstin")
        } else {
            None
        };
        self.report.differences.push(Difference {
            section: Some(self.entry.path.clone()),
            path,
            kind,
            left,
            right,
            derived,
            cause,
        });
    }
}

/// Top-level keys named by neither the header nor any identity entry: a
/// foreign-file signal, compared generically.
fn compare_unknown_keys(left: &Json, right: &Json, report: &mut DiffReport) {
    let known: Vec<&str> = upload::header_keys();
    let entry_tops: Vec<&str> = DIFF
        .sections
        .iter()
        .map(|e| e.path.split('.').next().expect("path"))
        .collect();
    let mut names: Vec<&str> = Vec::new();
    for side in [left, right] {
        if let Json::Obj(entries) = side {
            for (k, _) in entries {
                if !known.contains(&k.as_str())
                    && !entry_tops.contains(&k.as_str())
                    && !names.contains(&k.as_str())
                {
                    names.push(k);
                }
            }
        }
    }
    for name in names {
        report.notes.push(format!(
            "unknown key '{name}' — not part of the upload contract; foreign file?"
        ));
        let (l, r) = (left.get(name), right.get(name));
        if !json_eq(l, r) {
            report.differences.push(Difference {
                section: None,
                path: name.to_owned(),
                kind: DiffKind::ValueChanged,
                left: l.map(Json::to_json),
                right: r.map(Json::to_json),
                derived: false,
                cause: None,
            });
        }
    }
}
