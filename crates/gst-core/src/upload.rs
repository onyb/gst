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
    part_filename: Filename,
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

/// The envelope header alone — `{gstin, fp, version, hash}`, plus turnover
/// when supplied. This is [`build`] over no sections: omit-empty drops every
/// section key, leaving exactly the header the reference stamps on both the
/// upload file and the summary meta sidecar.
pub fn header(ctx: &FilingContext, turnover: Turnover) -> Json {
    build(&HashMap::new(), ctx, turnover)
}

/// The reference's date segment: day, month and year, no zero padding.
fn generated_dmy(generated_on: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    format!(
        "{}{}{}",
        generated_on.day(),
        generated_on.month(),
        generated_on.year()
    )
}

/// The filename the reference writes, e.g. `returns_2672026_R1_27AAA…_offline.json`.
///
/// The date segment is the date the file was GENERATED — day, month and year
/// concatenated with no zero padding — not the return period. The caller
/// supplies it so the core stays free of a clock.
pub fn filename(ctx: &FilingContext, generated_on: chrono::NaiveDate) -> String {
    ENVELOPE
        .filename
        .pattern
        .replace("{generated_dmy}", &generated_dmy(generated_on))
        .replace("{gstin}", &ctx.supplier_gstin)
}

/// The name of one part of a split upload, 1-based `part` of `parts` —
/// `returns_2672026_R1_27AAA…_offline_part2of3.json`. Deterministic where the
/// reference uses a random suffix; see `chunking.part_filename` in the
/// envelope spec for the recorded divergence.
pub fn chunk_filename(
    ctx: &FilingContext,
    generated_on: chrono::NaiveDate,
    part: usize,
    parts: usize,
) -> String {
    ENVELOPE
        .chunking
        .part_filename
        .pattern
        .replace("{generated_dmy}", &generated_dmy(generated_on))
        .replace("{gstin}", &ctx.supplier_gstin)
        .replace("{part}", &part.to_string())
        .replace("{parts}", &parts.to_string())
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

    /// The findings that are errors. Mirrors `Report::errors`.
    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == crate::spec::Severity::Error)
    }

    /// The upload, split into portal-sized parts when it must be.
    pub fn chunks(
        &self,
        ctx: &FilingContext,
        turnover: Turnover,
    ) -> Result<ChunkedUpload, ChunkError> {
        chunks(&self.sections, ctx, turnover)
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

/// An upload split (or not) into portal-sized parts. Each body is an
/// independent complete upload file: full header plus a subset of every
/// section's records, in envelope key order.
#[derive(Debug, Clone)]
pub struct ChunkedUpload {
    /// One serialized upload file per part; a single element iff the whole
    /// return fits the reference's chunk limit.
    pub bodies: Vec<String>,
    /// [`reference_size`] of the unsplit file — the number the split decision
    /// compared against the limit.
    pub unsplit_measure: usize,
}

/// Why an upload could not be split into portal-sized parts.
#[derive(Debug)]
pub enum ChunkError {
    /// One section envelope alone, with the header around it, exceeds the
    /// chunk limit. The reference silently drops or hangs on this; here it is
    /// a hard error so no data disappears.
    EnvelopeTooLarge {
        section: String,
        /// Position within the section's envelopes.
        index: usize,
        measured: usize,
        limit: usize,
    },
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChunkError::EnvelopeTooLarge {
                section,
                index,
                measured,
                limit,
            } => write!(
                f,
                "a single '{section}' envelope (record {index}) measures {measured} bytes \
                 against the {limit} byte chunk limit — no part can carry it; split its rows \
                 across smaller documents in the workbook"
            ),
        }
    }
}

impl std::error::Error for ChunkError {}

/// Split a return into portal-sized parts when it is too large.
///
/// The single-file decision is the reference's: one file iff the
/// double-stringified measure is under the 4.7 MiB limit, byte-identical to
/// [`build`]. The split itself deliberately diverges from the reference's
/// broken chunker (see `chunking.divergence` in the envelope spec): sections
/// pack greedily in envelope key order, splitting at envelope granularity,
/// and every part carries the full header. [`build`] itself prices each
/// candidate part, so the packer can never disagree with the serializer about
/// the IFF filter, the HSN branch, omit-empty or turnover.
pub fn chunks(
    sections: &HashMap<String, Generated>,
    ctx: &FilingContext,
    turnover: Turnover,
) -> Result<ChunkedUpload, ChunkError> {
    let whole = build(sections, ctx, turnover).to_json();
    let unsplit_measure = reference_size(&whole);
    let limit = max_chunk_bytes();
    if unsplit_measure <= limit {
        return Ok(ChunkedUpload {
            bodies: vec![whole],
            unsplit_measure,
        });
    }

    // One unit per envelope, in the order their sections appear in the file.
    let order = packing_order(ctx);
    let mut units: Vec<(&str, usize)> = Vec::new();
    for code in &order {
        if let Some(generated) = sections.get(code) {
            units.extend((0..generated.envelopes.len()).map(|i| (code.as_str(), i)));
        }
    }

    let mut bodies = Vec::new();
    let mut start = 0;
    while start < units.len() {
        let prefix = |end: usize| {
            let body = build(&slice(sections, &units[start..end]), ctx, turnover).to_json();
            (reference_size(&body), body)
        };
        let (first, _) = prefix(start + 1);
        if first > limit {
            let (section, index) = units[start];
            return Err(ChunkError::EnvelopeTooLarge {
                section: section.to_owned(),
                index,
                measured: first,
                limit,
            });
        }
        // The measure only grows as units are added, so the largest fitting
        // prefix is found by binary search; lo always names a fitting end.
        let (mut lo, mut hi) = (start + 1, units.len());
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if prefix(mid).0 <= limit {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        bodies.push(prefix(lo).1);
        start = lo;
    }
    Ok(ChunkedUpload {
        bodies,
        unsplit_measure,
    })
}

/// A whole upload reassembled from its parts.
#[derive(Debug, Clone)]
pub struct MergedParts {
    pub whole: Json,
    /// Anomalies tolerated during the merge — a part missing its header, say.
    /// Notes never make the merge fail.
    pub notes: Vec<String>,
}

/// Why a set of parts could not be reassembled into one upload.
#[derive(Debug)]
pub enum MergeError {
    /// A header scalar disagrees between parts — these are not parts of the
    /// same return.
    HeaderConflict {
        key: String,
        first: String,
        conflicting: String,
    },
    /// The same location holds structurally different values in two parts.
    ShapeConflict { path: String },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::HeaderConflict {
                key,
                first,
                conflicting,
            } => write!(
                f,
                "the parts disagree on '{key}' ({first} vs {conflicting}) — not parts of one return"
            ),
            MergeError::ShapeConflict { path } => {
                write!(f, "the parts hold incompatible shapes at '{path}'")
            }
        }
    }
}

impl std::error::Error for MergeError {}

/// The keys the envelope draws from filing context or literals — the header
/// every part repeats, as opposed to section content that unions.
pub(crate) fn header_keys() -> Vec<&'static str> {
    ENVELOPE
        .keys
        .iter()
        .filter(|e| e.from.starts_with("context:") || e.from.starts_with("literal:"))
        .map(|e| e.key.as_str())
        .collect()
}

/// Reassemble a split upload — the inverse of [`chunks`], per the
/// `chunking.divergence` invariant: section arrays concatenate, nested
/// objects merge member-wise, and the header must agree everywhere. A part
/// MISSING header keys is tolerated with a note rather than rejected,
/// because the reference's own chunker loses the header from its second
/// chunk onward; the first part carrying a key is authoritative.
pub fn merge_parts(parts: Vec<Json>) -> Result<MergedParts, MergeError> {
    let header = header_keys();
    let mut whole = Json::obj();
    let mut notes = Vec::new();

    for (index, part) in parts.iter().enumerate() {
        let Json::Obj(entries) = part else {
            return Err(MergeError::ShapeConflict {
                path: format!("part {}", index + 1),
            });
        };
        for (key, value) in entries {
            let Json::Obj(merged) = &mut whole else {
                unreachable!("whole starts as an object")
            };
            let existing = merged.iter_mut().find(|(k, _)| k == key);
            match existing {
                None => merged.push((key.clone(), value.clone())),
                Some((_, slot)) => {
                    if header.contains(&key.as_str()) {
                        if slot != value {
                            return Err(MergeError::HeaderConflict {
                                key: key.clone(),
                                first: slot.to_json(),
                                conflicting: value.to_json(),
                            });
                        }
                    } else {
                        merge_into(slot, value, key)?;
                    }
                }
            }
        }
    }

    // A part without the header it should repeat was not written by this
    // implementation; say so once per part rather than failing the merge.
    for (index, part) in parts.iter().enumerate() {
        let missing: Vec<&str> = header
            .iter()
            .filter(|key| whole.get(key).is_some() && part.get(key).is_none())
            .copied()
            .collect();
        if !missing.is_empty() {
            notes.push(format!(
                "part {} carries no {} — its header was taken from another part \
                 (the reference's own splitter loses headers)",
                index + 1,
                missing.join("/")
            ));
        }
    }

    Ok(MergedParts { whole, notes })
}

fn merge_into(existing: &mut Json, add: &Json, path: &str) -> Result<(), MergeError> {
    match (existing, add) {
        (Json::Arr(a), Json::Arr(b)) => {
            a.extend(b.iter().cloned());
            Ok(())
        }
        (Json::Obj(a), Json::Obj(b)) => {
            for (key, value) in b {
                match a.iter_mut().find(|(k, _)| k == key) {
                    None => a.push((key.clone(), value.clone())),
                    Some((_, slot)) => merge_into(slot, value, &format!("{path}.{key}"))?,
                }
            }
            Ok(())
        }
        (existing, add) if *existing == *add => Ok(()),
        _ => Err(MergeError::ShapeConflict {
            path: path.to_owned(),
        }),
    }
}

/// Section codes in the order their content appears in the upload file,
/// derived from the envelope spec's key walk so the packer and [`build`]
/// cannot disagree — txpd/txpda place atadj/atadja last, the HSN branch
/// follows the bifurcation, and IFF-dropped keys contribute nothing.
fn packing_order(ctx: &FilingContext) -> Vec<String> {
    let spec = &*ENVELOPE;
    let bifurcated = period_as_yyyymm(&spec.hsn_bifurcation_start_period)
        .is_some_and(|start| ctx.period.as_yyyymm() >= start);
    let iff = ctx.is_quarterly && !ctx.period.month.is_multiple_of(3);
    let keeps = |key: &str| !iff || spec.iff.keep_keys.iter().any(|k| k == key);
    let push = |order: &mut Vec<String>, code: &str| {
        // supeco/supecoa name one section from two members; count it once.
        if !order.iter().any(|c| c == code) {
            order.push(code.to_owned());
        }
    };

    let mut order = Vec::new();
    for entry in &spec.keys {
        if !keeps(&entry.key) {
            continue;
        }
        match entry.from.as_str() {
            "hsn" => {
                if bifurcated {
                    for member in &spec.hsn_from_bifurcation.members {
                        if let Some(code) = member.from.strip_prefix("section:") {
                            push(&mut order, code);
                        }
                    }
                } else {
                    push(&mut order, "hsn");
                }
            }
            "object" => {
                for member in &entry.members {
                    if let Some(code) = member.from.strip_prefix("section:") {
                        push(&mut order, code);
                    }
                }
            }
            other => {
                // context:/literal:/empty: keys carry no section content.
                if let Some(code) = other
                    .strip_prefix("section:")
                    .or_else(|| other.strip_prefix("wrapped:"))
                {
                    push(&mut order, code);
                }
            }
        }
    }
    order
}

/// The subset of `sections` holding exactly the given envelopes, members
/// sliced in lockstep. `generate` always keeps the two vectors the same
/// length; hand-built values with no members stay member-less.
fn slice(
    sections: &HashMap<String, Generated>,
    units: &[(&str, usize)],
) -> HashMap<String, Generated> {
    let mut out: HashMap<String, Generated> = HashMap::new();
    for (code, index) in units {
        let source = &sections[*code];
        let target = out.entry((*code).to_owned()).or_default();
        target.envelopes.push(source.envelopes[*index].clone());
        if source.members.len() == source.envelopes.len() {
            target.members.push(source.members[*index].clone());
        }
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

    // ---- chunk splitting ----

    /// A synthetic envelope of roughly `bytes` serialized size, tagged so
    /// union checks can track every record across parts.
    fn padded(id: &str, bytes: usize) -> Json {
        let mut o = Json::obj();
        o.insert_path("id", Json::Str(id.to_owned()));
        o.insert_path("pad", Json::Str("A".repeat(bytes)));
        o
    }

    fn section_of(envelopes: Vec<Json>, members: Vec<Option<String>>) -> Generated {
        Generated {
            envelopes,
            members,
            findings: vec![],
        }
    }

    /// `n` untagged envelopes of ~`bytes` each, ids `code-0..n`.
    fn bulk(code: &str, n: usize, bytes: usize) -> Generated {
        let envelopes: Vec<Json> = (0..n)
            .map(|i| padded(&format!("{code}-{i}"), bytes))
            .collect();
        let members = vec![None; n];
        section_of(envelopes, members)
    }

    /// The parts reassembled through the public API. The reassembled whole
    /// should be BYTE-identical to the unsplit build: parts are prefixes in
    /// envelope key order, so first-seen key order and concatenation order
    /// both reproduce the original.
    fn merged(bodies: &[String]) -> Json {
        let parts = bodies
            .iter()
            .map(|body| crate::payload::parse(body).expect("part parses"))
            .collect();
        let merged = merge_parts(parts).expect("parts merge");
        assert!(merged.notes.is_empty(), "{:?}", merged.notes);
        merged.whole
    }

    #[test]
    fn an_under_limit_return_is_a_single_identical_chunk() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 3, 1000));
        let c = ctx(7, 2017);
        let chunked = chunks(&sections, &c, Turnover::default()).unwrap();
        let whole = build(&sections, &c, Turnover::default()).to_json();
        assert_eq!(chunked.bodies, vec![whole.clone()]);
        assert_eq!(chunked.unsplit_measure, reference_size(&whole));
    }

    #[test]
    fn every_part_fits_and_carries_the_full_header() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 8, 1_000_000));
        let c = ctx(7, 2017);
        let turnover = Turnover {
            gross: Some(Decimal::from(5_000_000)),
            current: Some(Decimal::from(1_200_000)),
        };
        let chunked = chunks(&sections, &c, turnover).unwrap();
        assert!(
            chunked.bodies.len() >= 2,
            "{} part(s)",
            chunked.bodies.len()
        );
        for body in &chunked.bodies {
            assert!(reference_size(body) <= max_chunk_bytes());
            // Full header — including the turnover pair — on EVERY part; the
            // reference loses it from the second chunk onward.
            assert!(
                body.starts_with(
                    r#"{"gstin":"27AAPFU0939F1ZV","fp":"072017","gt":5000000,"cur_gt":1200000,"version":"GST3.2.4","hash":"hash""#
                ),
                "{}",
                &body[..120.min(body.len())]
            );
        }
    }

    #[test]
    fn the_union_of_parts_equals_the_unsplit_file() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 6, 900_000));
        sections.insert("b2cs".to_owned(), bulk("b2cs", 4, 200_000));
        sections.insert("atadj".to_owned(), bulk("atadj", 2, 100_000));
        sections.insert("nil".to_owned(), bulk("nil", 1, 1000));
        sections.insert("hsn(b2b)".to_owned(), bulk("hsnb", 2, 1000));
        sections.insert(
            "supeco".to_owned(),
            section_of(
                vec![padded("eco-0", 1000), padded("eco-1", 1000)],
                vec![Some("clttx".to_owned()), Some("paytx".to_owned())],
            ),
        );
        let c = ctx(6, 2025);
        let chunked = chunks(&sections, &c, Turnover::default()).unwrap();
        assert!(chunked.bodies.len() >= 2);
        let whole = build(&sections, &c, Turnover::default()).to_json();
        // The object-valued keys (nil, hsn, supeco) and the renamed txpd merge
        // back together with everything else.
        assert_eq!(merged(&chunked.bodies).to_json(), whole);
    }

    #[test]
    fn parts_pack_in_envelope_key_order() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 2, 3_000_000));
        sections.insert("b2cs".to_owned(), bulk("b2cs", 1, 1000));
        sections.insert("atadj".to_owned(), bulk("atadj", 1, 1000));
        let c = ctx(7, 2017);
        let chunked = chunks(&sections, &c, Turnover::default()).unwrap();
        assert_eq!(chunked.bodies.len(), 2);
        // First part is the b2b prefix alone; the trailing sections — b2cs and
        // atadj's renamed txpd — ride in the last part, in key order.
        assert!(chunked.bodies[0].contains(r#""b2b""#));
        assert!(!chunked.bodies[0].contains(r#""b2cs""#));
        assert!(!chunked.bodies[0].contains(r#""txpd""#));
        assert!(chunked.bodies[1].contains(r#""b2cs""#));
        assert!(chunked.bodies[1].ends_with(r#"}]}"#) && chunked.bodies[1].contains(r#""txpd""#));
        let b2cs_at = chunked.bodies[1].find(r#""b2cs""#).unwrap();
        let txpd_at = chunked.bodies[1].find(r#""txpd""#).unwrap();
        assert!(b2cs_at < txpd_at, "txpd must stay last");
    }

    #[test]
    fn member_tagged_sections_stay_in_lockstep() {
        // Alternating clttx/paytx rows big enough to force a split: each
        // part's supeco object must carry exactly its slice's tags.
        let n = 8;
        let envelopes: Vec<Json> = (0..n)
            .map(|i| padded(&format!("eco-{i}"), 900_000))
            .collect();
        let members: Vec<Option<String>> = (0..n)
            .map(|i| Some(if i % 2 == 0 { "clttx" } else { "paytx" }.to_owned()))
            .collect();
        let mut sections = HashMap::new();
        sections.insert("supeco".to_owned(), section_of(envelopes, members));
        let c = ctx(6, 2025);
        let chunked = chunks(&sections, &c, Turnover::default()).unwrap();
        assert!(chunked.bodies.len() >= 2);
        let whole = build(&sections, &c, Turnover::default()).to_json();
        assert_eq!(merged(&chunked.bodies).to_json(), whole);
        // And no part mixes a record into the wrong member.
        for body in &chunked.bodies {
            let part: serde_json::Value = serde_json::from_str(body).unwrap();
            for (member, prefix) in [("clttx", 0), ("paytx", 1)] {
                for record in part["supeco"][member].as_array().into_iter().flatten() {
                    let id = record["id"].as_str().unwrap();
                    let i: usize = id.strip_prefix("eco-").unwrap().parse().unwrap();
                    assert_eq!(i % 2, prefix, "{id} landed under {member}");
                }
            }
        }
    }

    #[test]
    fn an_envelope_too_large_to_fit_is_a_hard_error() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 1, 1000));
        let mut big = bulk("b2cs", 3, 1000);
        big.envelopes[1] = padded("b2cs-huge", 6_000_000);
        sections.insert("b2cs".to_owned(), big);
        let c = ctx(7, 2017);
        let err = chunks(&sections, &c, Turnover::default()).unwrap_err();
        match err {
            ChunkError::EnvelopeTooLarge {
                ref section, index, ..
            } => {
                assert_eq!(section, "b2cs");
                assert_eq!(index, 1);
            }
        }
        let message = err.to_string();
        assert!(
            message.contains("b2cs") && message.contains("4928307"),
            "{message}"
        );
    }

    #[test]
    fn iff_dropped_sections_never_reach_a_part() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 8, 1_000_000));
        sections.insert("b2cs".to_owned(), bulk("b2cs", 3, 500_000));
        let mut c = ctx(7, 2025); // month 7: an IFF month for a quarterly filer
        c.is_quarterly = true;
        let chunked = chunks(&sections, &c, Turnover::default()).unwrap();
        assert!(chunked.bodies.len() >= 2);
        for body in &chunked.bodies {
            assert!(!body.contains(r#""b2cs""#), "IFF drops b2cs");
        }
        let whole = build(&sections, &c, Turnover::default()).to_json();
        assert_eq!(merged(&chunked.bodies).to_json(), whole);
    }

    #[test]
    fn chunking_is_deterministic() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 7, 1_100_000));
        let c = ctx(7, 2017);
        let a = chunks(&sections, &c, Turnover::default()).unwrap();
        let b = chunks(&sections, &c, Turnover::default()).unwrap();
        assert_eq!(a.bodies, b.bodies);
    }

    #[test]
    fn parts_missing_their_header_merge_with_a_note() {
        let mut sections = HashMap::new();
        sections.insert("b2b".to_owned(), bulk("b2b", 6, 1_000_000));
        let c = ctx(7, 2017);
        let chunked = chunks(&sections, &c, Turnover::default()).unwrap();
        assert!(chunked.bodies.len() >= 2);

        // Strip the header from every part after the first — the shape the
        // reference's own broken splitter produces.
        let mut parts: Vec<Json> = chunked
            .bodies
            .iter()
            .map(|body| crate::payload::parse(body).unwrap())
            .collect();
        for part in parts.iter_mut().skip(1) {
            if let Json::Obj(entries) = part {
                entries.retain(|(k, _)| !["gstin", "fp", "version", "hash"].contains(&k.as_str()));
            }
        }
        let merged = merge_parts(parts).expect("still merges");
        assert_eq!(
            merged.notes.len(),
            chunked.bodies.len() - 1,
            "{:?}",
            merged.notes
        );
        assert_eq!(
            merged.whole.to_json(),
            build(&sections, &c, Turnover::default()).to_json()
        );
    }

    #[test]
    fn parts_of_different_returns_refuse_to_merge() {
        let part = |fp: &str| {
            crate::payload::parse(&format!(
                r#"{{"gstin":"27AAPFU0939F1ZV","fp":"{fp}","version":"GST3.2.4","hash":"hash","b2b":[]}}"#
            ))
            .unwrap()
        };
        match merge_parts(vec![part("062025"), part("072025")]) {
            Err(MergeError::HeaderConflict { key, .. }) => assert_eq!(key, "fp"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_part_filename_numbers_the_parts() {
        let c = ctx(6, 2025);
        let date = chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap();
        assert_eq!(
            chunk_filename(&c, date, 2, 3),
            "returns_2672026_R1_27AAPFU0939F1ZV_offline_part2of3.json"
        );
    }
}
