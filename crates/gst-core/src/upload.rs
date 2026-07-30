//! Assembling a complete GSTR-1 upload file.
//!
//! Individual sections generate their own payloads; this wraps them in the
//! envelope the portal expects. The key order, the empty-value shapes, and the
//! literal `version`/`hash` values all come from
//! `spec/gstr1/upload-envelope.json`, so the wrapper is spec-driven like
//! everything else.

use std::collections::HashMap;
use std::path::Path;

use rust_decimal::Decimal;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::generate::{Generated, generate};
use crate::import::{ImportError, Workbook};
use crate::payload::Json;
use crate::spec::{self, Severity, period_as_yyyymm};
use crate::validate::{FilingContext, Finding, validate};

#[derive(Debug, Clone, Deserialize)]
struct EnvelopeKey {
    key: String,
    from: String,
    #[serde(default)]
    wrapper: Option<String>,
    #[serde(default)]
    members: Vec<EnvelopeMember>,
}

/// One member of a payload object such as `ecom` or `supeco`. Each names the
/// section it draws from; `member` additionally selects a subset of that
/// section's records, for the sheets that feed two members at once.
#[derive(Debug, Clone, Deserialize)]
struct EnvelopeMember {
    key: String,
    from: String,
    #[serde(default)]
    member: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Wrapped {
    wrapper: Option<String>,
    #[serde(default)]
    members: Vec<EnvelopeMember>,
}

#[derive(Debug, Clone, Deserialize)]
struct Chunking {
    max_bytes: usize,
}

/// The reduced key set a quarterly filer's first two months carry.
#[derive(Debug, Clone, Deserialize)]
struct Iff {
    keep_keys: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Filename {
    pattern: String,
}

#[derive(Debug, Clone, Deserialize)]
struct EnvelopeSpec {
    version: String,
    hash: String,
    hsn_bifurcation_start_period: String,
    filename: Filename,
    chunking: Chunking,
    iff: Iff,
    #[serde(default)]
    omit_empty_sections: bool,
    keys: Vec<EnvelopeKey>,
    hsn_before_bifurcation: Wrapped,
    hsn_from_bifurcation: Wrapped,
}

static ENVELOPE: LazyLock<EnvelopeSpec> = LazyLock::new(|| {
    crate::masters::embedded(
        "gstr1/upload-envelope.json",
        include_str!("../../../spec/gstr1/upload-envelope.json"),
    )
});

/// Turnover figures, which only some periods carry.
#[derive(Debug, Clone, Copy, Default)]
pub struct Turnover {
    pub gross: Option<Decimal>,
    pub current: Option<Decimal>,
}

impl Turnover {
    /// The pair, present only when both halves are. The reference emits them
    /// together or not at all; see the envelope builder.
    pub fn pair(&self) -> Option<(Decimal, Decimal)> {
        self.gross.zip(self.current)
    }
}

/// The size beyond which the reference splits an upload into chunks.
pub fn max_chunk_bytes() -> usize {
    ENVELOPE.chunking.max_bytes
}

/// The size the reference actually compares against its chunk limit.
///
/// It measures `jsonSize(JSON.stringify(gstfile))` (offline.js:5474), but
/// `gstfile` has already been serialized by then — so the string gets
/// stringified a SECOND time and what is measured is the UTF-8 length of the
/// JSON string *literal*: the body, plus the two enclosing quotes, plus one
/// escape byte for every `"` and `\` in it. GSTR-1 payloads run 15-20% quote
/// characters, so the reference splits at roughly 3.9-4.1 MiB of real JSON
/// rather than the 4.7 MiB the constant suggests.
pub fn reference_size(body: &str) -> usize {
    body.len() + 2 + body.bytes().filter(|b| *b == b'"' || *b == b'\\').count()
}

/// The `version` literal the reference stamps on every file it writes.
pub fn version_literal() -> &'static str {
    &ENVELOPE.version
}

/// The literal placeholder `hash` value the reference never fills in.
pub fn hash_literal() -> &'static str {
    &ENVELOPE.hash
}

/// The filename the reference writes, e.g. `returns_2672026_R1_27AAA…_offline.json`.
///
/// The date segment is the date the file was GENERATED — day, month and year
/// concatenated with no zero padding — not the return period. The caller
/// supplies it so the core stays free of a clock.
pub fn filename(ctx: &FilingContext, generated_on: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let dmy = format!(
        "{}{}{}",
        generated_on.day(),
        generated_on.month(),
        generated_on.year()
    );
    ENVELOPE
        .filename
        .pattern
        .replace("{generated_dmy}", &dmy)
        .replace("{gstin}", &ctx.supplier_gstin)
}

/// One section's contribution to a whole-workbook run.
#[derive(Debug, Clone)]
pub struct SectionStat {
    pub section: &'static str,
    pub rows: usize,
    pub accepted: usize,
    pub envelopes: usize,
}

/// Every section of one workbook, read, validated and grouped — the input
/// [`build`] wraps into the upload file.
#[derive(Debug, Clone, Default)]
pub struct WorkbookRun {
    /// Section code to generated payload, for sections that produced records.
    pub sections: HashMap<String, Generated>,
    /// Validation and grouping findings across all sections.
    pub findings: Vec<Finding>,
    /// One entry per section sheet that had data, in return order.
    pub stats: Vec<SectionStat>,
}

impl WorkbookRun {
    /// The complete upload file for this run.
    pub fn build(&self, ctx: &FilingContext, turnover: Turnover) -> Json {
        build(&self.sections, ctx, turnover)
    }
}

/// Why a whole-workbook read stopped.
#[derive(Debug)]
pub enum WorkbookError {
    /// The file itself could not be opened.
    Open(ImportError),
    /// One section's sheet was unreadable. A *missing* sheet is not an error —
    /// a workbook legitimately lacks sheets for sections the filer does not use.
    Section {
        section: &'static str,
        error: ImportError,
    },
}

impl std::fmt::Display for WorkbookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkbookError::Open(e) => write!(f, "{e}"),
            WorkbookError::Section { section, error } => {
                write!(f, "cannot read section '{section}': {error}")
            }
        }
    }
}

impl std::error::Error for WorkbookError {}

/// Read every section the engine knows from one workbook, validating and
/// grouping each. The workbook is opened and parsed once, not per section.
/// Sections whose sheet is absent or empty contribute nothing.
pub fn read_workbook(path: &Path, ctx: &FilingContext) -> Result<WorkbookRun, WorkbookError> {
    let mut workbook = Workbook::open(path).map_err(WorkbookError::Open)?;
    let mut run = WorkbookRun::default();

    for section in spec::sections() {
        let rows = match workbook.read(section) {
            Ok(rows) => rows,
            Err(ImportError::SheetMissing { .. }) => continue,
            Err(error) => {
                return Err(WorkbookError::Section {
                    section: section.section.as_str(),
                    error,
                });
            }
        };
        if rows.is_empty() {
            continue;
        }
        // A sheet outside its section's period window has nowhere to go in the
        // upload file. Skipping it silently is exactly how the pre-bifurcation
        // HSN summary used to vanish, so say so instead.
        if !section.active_for(ctx.period.as_yyyymm()) {
            run.findings.push(Finding {
                sheet_row: 0,
                column: None,
                field: None,
                rule: Some("workbook.section_not_filed_this_period".into()),
                severity: Severity::Error,
                message: format!(
                    "sheet '{}' has {} row(s) but section '{}' is not filed for period {} — \
                     this is the wrong workbook template for the period",
                    section
                        .source
                        .excel
                        .as_ref()
                        .map_or(section.section.as_str(), |e| e.sheet.as_str()),
                    rows.len(),
                    section.section,
                    ctx.period.as_mmyyyy(),
                ),
            });
            continue;
        }
        let report = validate(section, &rows, ctx);
        let mut out = generate(section, &report.records, ctx);
        run.stats.push(SectionStat {
            section: section.section.as_str(),
            rows: rows.len(),
            accepted: report.records.len(),
            envelopes: out.envelopes.len(),
        });
        run.findings.extend(report.findings);
        run.findings.extend(std::mem::take(&mut out.findings));
        if !out.envelopes.is_empty() {
            run.sections.insert(section.section.clone(), out);
        }
    }
    Ok(run)
}

/// Build the complete upload file.
///
/// `sections` maps a section code to the envelopes that section generated.
/// Sections the caller has nothing for are still emitted, empty, because the
/// reference always writes the full key set.
pub fn build(
    sections: &HashMap<String, Generated>,
    ctx: &FilingContext,
    turnover: Turnover,
) -> Json {
    let spec = &*ENVELOPE;
    let bifurcated = period_as_yyyymm(&spec.hsn_bifurcation_start_period)
        .is_some_and(|start| ctx.period.as_yyyymm() >= start);

    let take = |code: &str| -> Vec<Json> { section_envelopes(sections, code, None) };
    // A member draws from one section, optionally taking only the records that
    // section tagged for it.
    let take_member = |member: &EnvelopeMember| -> Vec<Json> {
        let Some(code) = member.from.strip_prefix("section:") else {
            panic!(
                "envelope member `{}` names unknown source `{}`",
                member.key, member.from
            );
        };
        section_envelopes(sections, code, member.member.as_deref())
    };

    // A quarterly filer's first two months are an Invoice Furnishing Facility
    // return, which carries four tables and the header keys and nothing else —
    // not even the turnover. The reference deletes the rest after building the
    // full object (offline.js:5464), so the filter is applied the same way here.
    let iff = ctx.is_quarterly && !ctx.period.month.is_multiple_of(3);
    let keeps = |key: &str| !iff || spec.iff.keep_keys.iter().any(|k| k == key);

    let mut out = Json::obj();
    for entry in &spec.keys {
        if !keeps(&entry.key) {
            continue;
        }
        let mut value = match entry.from.as_str() {
            "context:gstin" => Json::Str(ctx.supplier_gstin.clone()),
            "context:period" => Json::Str(ctx.period.as_mmyyyy()),
            // Both or neither, and both integers.
            //
            // The reference reads them with parseInt (common.js:168), so a
            // fractional turnover is truncated before it is written. And it
            // decides on `isNaN(gt)` alone (common.js:178): with a gross
            // turnover it emits BOTH keys, without one it emits neither. There
            // is no reachable input that produces cur_gt on its own — supplying
            // only that yields the literal text `"cur_gt":NaN`, which is not
            // JSON and blows up the tool's own parse a few lines later.
            "context:gross_turnover" => match turnover.pair() {
                Some((gross, _)) => Json::Num(gross.trunc()),
                None => continue,
            },
            "context:current_gross_turnover" => match turnover.pair() {
                Some((_, current)) => Json::Num(current.trunc()),
                None => continue,
            },
            "literal:version" => Json::Str(spec.version.clone()),
            "literal:hash" => Json::Str(spec.hash.clone()),
            "empty:array" => Json::Arr(Vec::new()),
            "hsn" => {
                let mut hsn = Json::obj();
                if bifurcated {
                    for member in &spec.hsn_from_bifurcation.members {
                        hsn.insert_path(&member.key, Json::Arr(take_member(member)));
                    }
                } else if let Some(wrapper) = &spec.hsn_before_bifurcation.wrapper {
                    hsn.insert_path(wrapper, Json::Arr(take("hsn")));
                }
                hsn
            }
            "object" => {
                let mut obj = Json::obj();
                for member in &entry.members {
                    obj.insert_path(&member.key, Json::Arr(take_member(member)));
                }
                obj
            }
            other => {
                if let Some(code) = other.strip_prefix("section:") {
                    Json::Arr(take(code))
                } else if let Some(code) = other.strip_prefix("wrapped:") {
                    let wrapper = entry
                        .wrapper
                        .as_deref()
                        .expect("wrapped envelope key declares a wrapper");
                    let mut obj = Json::obj();
                    obj.insert_path(wrapper, Json::Arr(take(code)));
                    obj
                } else {
                    panic!("envelope spec names unknown source `{other}`");
                }
            }
        };
        // The upload file passes through omit-empty, so a section with no
        // records is absent entirely rather than present as an empty array.
        // omit-empty is recursive, so empty members of a nested object (an
        // unused half of the HSN summary, say) drop the same way.
        if spec.omit_empty_sections {
            prune_empty(&mut value);
            if value.is_empty_recursive() {
                continue;
            }
        }
        out.insert_path(&entry.key, value);
    }
    out
}

/// The envelopes one section generated, optionally only those the section
/// tagged for a given payload member. An absent section yields nothing.
fn section_envelopes(
    sections: &HashMap<String, Generated>,
    code: &str,
    tag: Option<&str>,
) -> Vec<Json> {
    let Some(generated) = sections.get(code) else {
        return Vec::new();
    };
    match tag {
        None => generated.envelopes.clone(),
        Some(tag) => generated
            .envelopes
            .iter()
            .zip(generated.members.iter())
            .filter(|(_, m)| m.as_deref() == Some(tag))
            .map(|(json, _)| json.clone())
            .collect(),
    }
}

/// Drop empty members from a nested object, recursively.
///
/// Inferred rather than observed: the captured reference file had both halves of
/// the HSN summary populated, so it could not show whether a half-empty object
/// keeps its empty member. omit-empty is recursive by construction and the
/// top-level behaviour is confirmed, so the same rule is applied inside.
fn prune_empty(value: &mut Json) {
    match value {
        Json::Obj(entries) => {
            for (_, v) in entries.iter_mut() {
                prune_empty(v);
            }
            entries.retain(|(_, v)| !v.is_empty_recursive());
        }
        // The reference's omit-empty walks array elements as well
        // (node_modules/omit-empty/index.js:59), so an empty key inside an
        // invoice or a line item is dropped at every depth, not just at the
        // top level.
        Json::Arr(items) => {
            for item in items.iter_mut() {
                prune_empty(item);
            }
            items.retain(|item| !item.is_empty_recursive());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::ReturnPeriod;

    fn ctx(month: u32, year: i32) -> FilingContext {
        FilingContext {
            supplier_gstin: "27AAPFU0939F1ZV".into(),
            period: ReturnPeriod::new(month, year).unwrap(),
            is_sez: false,
            aato_over_5cr: false,
            is_quarterly: false,
        }
    }

    #[test]
    #[ignore = "disproven by fixtures/golden: empty sections are omitted, not emitted as []"]
    fn an_empty_return_still_carries_every_section_key() {
        let json = build(&HashMap::new(), &ctx(7, 2017), Turnover::default()).to_json();
        for key in [
            "gstin",
            "fp",
            "version",
            "hash",
            "b2b",
            "b2ba",
            "b2cl",
            "b2cla",
            "b2cs",
            "b2csa",
            "nil",
            "exp",
            "expa",
            "hsnSac",
            "cdnra",
            "at",
            "ata",
            "cdnr",
            "cdnur",
            "cdnura",
            "atadj",
            "atadja",
            "doc_issue",
            "hsn",
            "supeco",
            "supecoa",
            "ecom",
            "ecoma",
        ] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "missing {key}: {json}"
            );
        }
        // The literals are fixed by the tool release, not by the return.
        assert!(json.contains(r#""version":"GST3.2.4""#), "{json}");
        assert!(json.contains(r#""hash":"hash""#), "{json}");
        // Turnover is omitted entirely when not supplied.
        assert!(!json.contains("\"gt\""), "{json}");
        assert!(!json.contains("\"cur_gt\""), "{json}");
    }

    #[test]
    #[ignore = "superseded by the byte-for-byte golden test, which covers key order on real data"]
    fn the_key_order_matches_the_reference() {
        let json = build(&HashMap::new(), &ctx(7, 2017), Turnover::default()).to_json();
        let order = [
            "\"gstin\"",
            "\"fp\"",
            "\"version\"",
            "\"hash\"",
            "\"b2b\"",
            "\"b2ba\"",
            "\"b2cl\"",
            "\"b2cla\"",
            "\"b2cs\"",
            "\"b2csa\"",
            "\"nil\"",
            "\"exp\"",
            "\"expa\"",
            "\"hsnSac\"",
            "\"cdnra\"",
            "\"at\"",
            "\"ata\"",
            "\"cdnr\"",
            "\"cdnur\"",
            "\"cdnura\"",
            "\"atadj\"",
            "\"atadja\"",
            "\"doc_issue\"",
            "\"hsn\"",
            "\"supeco\"",
            "\"supecoa\"",
            "\"ecom\"",
            "\"ecoma\"",
        ];
        let mut last = 0usize;
        for key in order {
            let at = json.find(key).unwrap_or_else(|| panic!("missing {key}"));
            assert!(at > last, "{key} is out of order in {json}");
            last = at;
        }
    }

    #[test]
    fn turnover_appears_right_after_the_period_when_supplied() {
        let json = build(
            &HashMap::new(),
            &ctx(7, 2017),
            Turnover {
                gross: Some(Decimal::from(5000000)),
                current: Some(Decimal::from(1200000)),
            },
        )
        .to_json();
        assert!(
            json.starts_with(
                r#"{"gstin":"27AAPFU0939F1ZV","fp":"072017","gt":5000000,"cur_gt":1200000,"version""#
            ),
            "{json}"
        );
    }

    /// The HSN summary changes shape at 05-2025, and the section feeding it
    /// changes with it.
    ///
    /// Driven through the real pipeline — spec, validation, generation — rather
    /// than by planting a section key in the map. An earlier version of this
    /// test inserted `"hsn"` by hand, which no import could ever produce: no
    /// spec registered that code, `take("hsn")` therefore always came back
    /// empty, and every return before 05-2025 silently shipped without its HSN
    /// summary while this test stayed green.
    #[test]
    fn hsn_changes_shape_at_the_bifurcation_period() {
        use crate::record::Row;

        /// Every template column, blank except the ones named — so a column
        /// this section declares but the caller forgot still reads as empty
        /// rather than as a missing column.
        fn generated(code: &str, ctx: &FilingContext, cells: &[(&str, &str)]) -> Generated {
            let spec = spec::section(code).expect("registered");
            let row = Row::from_pairs(
                5,
                spec.columns().into_iter().map(|column| {
                    let value = cells
                        .iter()
                        .find(|(name, _)| *name == column)
                        .map_or("", |(_, value)| *value);
                    (column, value)
                }),
            );
            let report = validate(spec, &[row], ctx);
            assert!(report.is_clean(), "{code}: {:?}", report.findings);
            generate(spec, &report.records, ctx)
        }

        // Before May 2025: one `hsn` sheet feeding a single `data` array. The
        // period also predates 05-2021, so the record carries `val` and no rate.
        let before_ctx = ctx(4, 2021);
        let mut before_sections = HashMap::new();
        before_sections.insert(
            "hsn".to_string(),
            generated(
                "hsn",
                &before_ctx,
                &[
                    ("HSN", "0101"),
                    ("Description", "Live horses"),
                    ("UQC", "NOS-NUMBERS"),
                    ("Total Quantity", "10"),
                    ("Total Value", "118000"),
                    ("Taxable Value", "100000"),
                    ("Integrated Tax Amount", "18000"),
                ],
            ),
        );
        let before = build(&before_sections, &before_ctx, Turnover::default()).to_json();
        assert!(before.contains(r#""hsn":{"data":["#), "{before}");
        assert!(before.contains(r#""hsn_sc":"0101""#), "{before}");
        assert!(before.contains(r#""val":118000"#), "{before}");
        assert!(
            !before.contains(r#""rt""#),
            "pre-05-2021 has no rate: {before}"
        );

        // From 05-2021, still the single sheet, but now carrying the rate in
        // place of the total value.
        let mid_ctx = ctx(6, 2021);
        let mut mid_sections = HashMap::new();
        mid_sections.insert(
            "hsn".to_string(),
            generated(
                "hsn",
                &mid_ctx,
                &[
                    ("HSN", "0101"),
                    ("Description", "Live horses"),
                    ("UQC", "NOS-NUMBERS"),
                    ("Total Quantity", "10"),
                    ("Rate", "18"),
                    ("Taxable Value", "100000"),
                    ("Integrated Tax Amount", "18000"),
                ],
            ),
        );
        let mid = build(&mid_sections, &mid_ctx, Turnover::default()).to_json();
        assert!(mid.contains(r#""hsn":{"data":["#), "{mid}");
        assert!(mid.contains(r#""rt":18"#), "{mid}");
        assert!(
            !mid.contains(r#""val""#),
            "from 05-2021 there is no val: {mid}"
        );

        // From May 2025, split into B2B and B2C halves.
        let after_ctx = ctx(5, 2025);
        let mut after_sections = HashMap::new();
        after_sections.insert(
            "hsn(b2b)".to_string(),
            generated(
                "hsn(b2b)",
                &after_ctx,
                &[
                    ("HSN", "0101"),
                    ("Description", "Live horses"),
                    ("UQC", "NOS-NUMBERS"),
                    ("Total Quantity", "10"),
                    ("Total Value", "118000"),
                    ("Rate", "18"),
                    ("Taxable Value", "100000"),
                    ("Integrated Tax Amount", "18000"),
                ],
            ),
        );
        let after = build(&after_sections, &after_ctx, Turnover::default()).to_json();
        assert!(after.contains(r#""hsn":{"hsn_b2b":["#), "{after}");
        // The empty half is dropped rather than emitted.
        assert!(!after.contains("hsn_b2c"), "{after}");
    }

    /// The regression the fabricated test above used to hide: a section the
    /// envelope draws from must actually be registered under that code.
    #[test]
    fn every_section_the_envelope_names_is_a_registered_section() {
        let mut named: Vec<&str> = Vec::new();
        for entry in &ENVELOPE.keys {
            if let Some(code) = entry.from.strip_prefix("section:") {
                named.push(code);
            }
            for member in &entry.members {
                if let Some(code) = member.from.strip_prefix("section:") {
                    named.push(code);
                }
            }
        }
        for member in &ENVELOPE.hsn_from_bifurcation.members {
            if let Some(code) = member.from.strip_prefix("section:") {
                named.push(code);
            }
        }
        // The pre-bifurcation branch draws from the bare `hsn` code.
        named.push("hsn");

        for code in named {
            assert!(
                spec::section(code).is_some(),
                "the upload envelope draws from section '{code}', which no spec registers — \
                 that key would silently be empty in every generated file"
            );
        }
    }

    #[test]
    #[ignore = "wrappers only appear once those sections have records; covered by the golden test"]
    fn nil_and_doc_issue_are_wrapped_not_bare_arrays() {
        let json = build(&HashMap::new(), &ctx(7, 2017), Turnover::default()).to_json();
        assert!(json.contains(r#""nil":{"inv":[]}"#), "{json}");
        assert!(json.contains(r#""doc_issue":{"doc_det":[]}"#), "{json}");
    }

    #[test]
    fn section_payloads_land_under_their_own_key() {
        let mut sections = HashMap::new();
        sections.insert(
            "b2b".to_string(),
            Generated {
                envelopes: vec![Json::Obj(vec![(
                    "ctin".to_string(),
                    Json::Str("12GEOPS0823BBZH".into()),
                )])],
                ..Default::default()
            },
        );
        let json = build(&sections, &ctx(7, 2017), Turnover::default()).to_json();
        assert!(
            json.contains(r#""b2b":[{"ctin":"12GEOPS0823BBZH"}]"#),
            "{json}"
        );
        // Untouched sections are absent entirely, not emitted as [].
        assert!(!json.contains("b2ba"), "{json}");
    }

    #[test]
    fn the_filename_carries_the_generation_date_not_the_period() {
        // Day and month are unpadded, and the period plays no part: a return for
        // July 2017 generated on 26 July 2026 is named for the latter.
        let generated = chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap();
        assert_eq!(
            filename(&ctx(7, 2017), generated),
            "returns_2672026_R1_27AAPFU0939F1ZV_offline.json"
        );
        // Single-digit day and month stay single-digit.
        let early = chrono::NaiveDate::from_ymd_opt(2026, 2, 5).unwrap();
        assert_eq!(
            filename(&ctx(7, 2017), early),
            "returns_522026_R1_27AAPFU0939F1ZV_offline.json"
        );
    }

    #[test]
    fn empty_sections_are_omitted_entirely() {
        // An empty return carries only the four header keys — every section is
        // dropped by omit-empty rather than emitted as [].
        let json = build(&HashMap::new(), &ctx(7, 2017), Turnover::default()).to_json();
        assert_eq!(
            json,
            r#"{"gstin":"27AAPFU0939F1ZV","fp":"072017","version":"GST3.2.4","hash":"hash"}"#
        );
    }

    #[test]
    fn the_chunk_threshold_is_the_tools_actual_limit() {
        // 4.7 MiB, not the 5 MB the portal documentation mentions.
        assert_eq!(max_chunk_bytes(), 4_928_307);
    }
}
