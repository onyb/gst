//! Grouping validated records into the portal upload payload.
//!
//! Workbook rows are one-per-tax-rate, while the payload is three levels deep:
//! an envelope per counterparty, an invoice per document, and a line item per
//! rate. Grouping collapses rows accordingly, then the spec's `output` mapping
//! and named derivations build the JSON.

use std::collections::HashMap;

use rust_decimal::{Decimal, RoundingStrategy};

use crate::payload::Json;
use crate::record::Record;
use crate::spec::{
    ItemConflict, Level, PayloadObject, RecordConflict, SectionSpec, Severity, Source,
};
use crate::validate::{FilingContext, Finding};

/// Half of a percent, i.e. the CGST or SGST share of a combined rate.
const HALF_RATE: f64 = 0.005;
const FULL_RATE: f64 = 0.01;

/// Grouped output for one section.
#[derive(Debug, Clone, Default)]
pub struct Generated {
    /// One entry per envelope, in first-seen order.
    pub envelopes: Vec<Json>,
    /// Which member of the payload object each envelope belongs to, for the
    /// sections that split one sheet across several. Empty otherwise.
    pub members: Vec<Option<String>>,
    /// Problems that only surface once rows are grouped.
    pub findings: Vec<Finding>,
}

impl Generated {
    /// The section's payload as the portal carries it: an array of envelopes.
    pub fn to_json(&self) -> String {
        let mut out = String::from("[");
        for (i, envelope) in self.envelopes.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            envelope.write(&mut out);
        }
        out.push(']');
        out
    }

    /// Whether grouping produced no errors. Mirrors `Report::is_clean`.
    pub fn is_clean(&self) -> bool {
        !self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// The findings that are errors.
    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
    }

    /// The one way an envelope enters the output, so `envelopes` and `members`
    /// cannot fall out of lockstep.
    fn push(&mut self, envelope: Json, member: Option<String>) {
        self.envelopes.push(envelope);
        self.members.push(member);
    }
}

/// An invoice under construction: its own field values plus its line items.
struct InvoiceGroup {
    /// The first row seen for this invoice, source of all invoice-level fields.
    head: Record,
    /// Item rows, keyed by the spec's `item_key` for conflict handling.
    items: Vec<(String, Record)>,
}

struct EnvelopeGroup {
    head: Record,
    /// The grouping key this envelope was opened under, so a row arriving via
    /// the section-wide invoice index can be checked against it.
    key: String,
    /// Invoices in first-seen order, with a key index beside them so grouping
    /// stays linear in the number of rows.
    invoices: Vec<InvoiceGroup>,
    invoice_index: HashMap<String, usize>,
}

/// Build the payload for a section from records that passed validation.
pub fn generate(spec: &SectionSpec, records: &[Record], ctx: &FilingContext) -> Generated {
    let mut out = Generated::default();

    // Flat sections have no invoice or item level: every validated row becomes
    // one payload object, in row order, with its tax computed on the row —
    // after rows sharing a collapse key have been folded together.
    if let Some(record_spec) = &spec.output.record {
        let kept = collapse_records(spec, records, ctx, &mut out.findings);
        for (index, record) in kept.iter().enumerate() {
            let json = build_object(record_spec, record, ctx, index, &mut out.findings);
            out.push(json, member_of(spec, record));
        }
        return out;
    }

    let mut envelopes: Vec<EnvelopeGroup> = Vec::new();
    let mut envelope_index: HashMap<String, usize> = HashMap::new();

    // Invoice-level fields are those the payload reads at the invoice or
    // envelope level. Rows sharing an invoice key must agree on all of them.
    let mut invoice_fields = spec
        .output
        .invoice
        .as_ref()
        .map(mapped_fields)
        .unwrap_or_default();
    // Some fields must agree without ever being emitted.
    invoice_fields.extend(spec.grouping.agree_fields.iter().cloned());
    let mut envelope_fields = spec
        .output
        .envelope
        .as_ref()
        .map(mapped_fields)
        .unwrap_or_default();
    envelope_fields.extend(spec.grouping.agree_fields.iter().cloned());

    // Sections with no counterparty id match an invoice number across the whole
    // section rather than within one envelope: invoice number to the envelope
    // that first claimed it.
    let mut global_invoices: HashMap<String, usize> = HashMap::new();

    // Amendment sections: original identity to the revised number that first
    // claimed it (and its row), scoped like the invoice key.
    let mut original_claims: HashMap<(String, String), (String, usize)> = HashMap::new();

    for record in records {
        let env_key = group_key(spec, record, &spec.grouping.envelope_key);
        let inv_key = group_key(spec, record, &spec.grouping.invoice_key);

        // One original document cannot take two revised numbers: the
        // reference keeps the first row and flags the rest (verified by
        // capture — multiItmErrData names the later revised number).
        if !spec.grouping.original_key.is_empty() {
            let orig_key = group_key(spec, record, &spec.grouping.original_key);
            let scope = if spec.grouping.invoice_key_global {
                String::new()
            } else {
                env_key.clone()
            };
            let claim = original_claims
                .entry((scope, orig_key))
                .or_insert_with(|| (inv_key.clone(), record.sheet_row));
            if claim.0 != inv_key {
                let column = spec
                    .grouping
                    .original_key
                    .first()
                    .and_then(|id| spec.field(id).map(|f| f.column.clone()))
                    .unwrap_or_else(|| "original number".to_owned());
                out.findings.push(Finding {
                    sheet_row: record.sheet_row,
                    column: Some(column),
                    field: spec.grouping.original_key.first().cloned(),
                    rule: Some("grouping.original_number_conflict".into()),
                    severity: Severity::Error,
                    message: format!(
                        "row {} already amends this original document under a different \
                         revised number — one original cannot take two revised numbers",
                        claim.1
                    ),
                });
                continue;
            }
        }

        // In global mode an invoice number already seen decides which envelope
        // this row belongs to, before any new envelope is opened.
        let claimed = spec
            .grouping
            .invoice_key_global
            .then(|| global_invoices.get(&inv_key).copied())
            .flatten();

        let env_pos = match claimed {
            Some(pos) => pos,
            None => *envelope_index.entry(env_key.clone()).or_insert_with(|| {
                envelopes.push(EnvelopeGroup {
                    head: record.clone(),
                    key: env_key.clone(),
                    invoices: Vec::new(),
                    invoice_index: HashMap::new(),
                });
                envelopes.len() - 1
            }),
        };

        // Reaching an envelope through the invoice number means the envelope's
        // own key was never matched, so it has to be checked here. `disagreement`
        // deliberately skips grouping-key fields, which are matched by
        // construction on every other path.
        if claimed.is_some() && envelopes[env_pos].key != env_key {
            let column = spec
                .grouping
                .envelope_key
                .first()
                .and_then(|id| spec.field(id).map(|f| f.column.clone()))
                .unwrap_or_else(|| "envelope".to_owned());
            out.findings.push(Finding {
                sheet_row: record.sheet_row,
                column: Some(column.clone()),
                field: spec.grouping.envelope_key.first().cloned(),
                rule: Some("grouping.invoice_number_reused".into()),
                severity: Severity::Error,
                message: format!(
                    "this document number is already used on row {} with a different '{column}'. \
                     This section has no counterparty column, so a document number identifies a \
                     document on its own and cannot appear twice with different details.",
                    envelopes[env_pos].head.sheet_row,
                ),
            });
            continue;
        }

        let envelope = &mut envelopes[env_pos];

        if let Some(finding) =
            disagreement(spec, &envelope.head, record, &envelope_fields, "recipient")
        {
            out.findings.push(finding);
            continue;
        }

        let invoices = &mut envelope.invoices;
        let inv_pos = *envelope
            .invoice_index
            .entry(inv_key.clone())
            .or_insert_with(|| {
                invoices.push(InvoiceGroup {
                    head: record.clone(),
                    items: Vec::new(),
                });
                invoices.len() - 1
            });
        global_invoices.entry(inv_key).or_insert(env_pos);
        let invoice = &mut envelope.invoices[inv_pos];

        if let Some(finding) = disagreement(spec, &invoice.head, record, &invoice_fields, "invoice")
        {
            out.findings.push(finding);
            continue;
        }

        if spec.grouping.item_key.is_empty() {
            continue;
        }
        let item_key = group_key(spec, record, &spec.grouping.item_key);
        // Nothing to collide with: this level is keyed on a serial the
        // reference generates per row.
        if spec.grouping.item_conflict == Some(ItemConflict::Append) {
            invoice.items.push((item_key, record.clone()));
            continue;
        }
        match invoice.items.iter().position(|(k, _)| *k == item_key) {
            None => invoice.items.push((item_key, record.clone())),
            Some(existing) => match spec.grouping.item_conflict {
                // The reference implementation replaces silently. Reproduced,
                // but reported as a warning so the loss is at least visible.
                Some(ItemConflict::LastWins) | None => {
                    let previous = invoice.items[existing].1.sheet_row;
                    invoice.items[existing] = (item_key, record.clone());
                    out.findings.push(Finding {
                        sheet_row: record.sheet_row,
                        column: None,
                        field: None,
                        rule: Some("grouping.item_replaced".into()),
                        severity: Severity::Warning,
                        message: format!(
                            "this line replaces the one on row {previous}: same invoice and same rate. \
                             Amounts are not added together — combine them into one row if both were intended."
                        ),
                    });
                }
                Some(ItemConflict::Sum) => {
                    out.findings.push(Finding {
                        sheet_row: record.sheet_row,
                        column: None,
                        field: None,
                        rule: Some("grouping.item_sum_unsupported".into()),
                        severity: Severity::Error,
                        message: "summing duplicate line items is not implemented".into(),
                    });
                }
                // Handled before the lookup; a key collision never reaches here.
                Some(ItemConflict::Append) => {}
                Some(ItemConflict::Error) => out.findings.push(Finding {
                    sheet_row: record.sheet_row,
                    column: None,
                    field: None,
                    rule: Some("grouping.duplicate_item".into()),
                    severity: Severity::Error,
                    message: format!(
                        "duplicate line: the same invoice already has this rate on row {}",
                        invoice.items[existing].1.sheet_row
                    ),
                }),
            },
        }
    }

    // A section with an invoice level but no envelope puts its records at the
    // top level of the payload while still carrying line items.
    if spec.output.envelope.is_none() {
        for envelope in envelopes {
            for invoice in &envelope.invoices {
                let json = build_invoice(spec, invoice, ctx, &mut out.findings);
                out.push(json, member_of(spec, &invoice.head));
            }
        }
        return out;
    }

    for envelope in envelopes {
        let json = build_envelope(spec, &envelope, ctx, &mut out.findings);
        out.push(json, member_of(spec, &envelope.head));
    }
    out
}

/// Fold flat rows that share the section's collapse key.
///
/// The reference does this on the way into its working file rather than in the
/// payload builder, which is why it is invisible in the row mapping. Nothing is
/// summed: one of the two rows is discarded outright, so each collapse is
/// reported as a warning even though it reproduces the reference exactly.
///
/// A section with no `record_key` keeps every row, which is the common case.
fn collapse_records<'a>(
    spec: &SectionSpec,
    records: &'a [Record],
    ctx: &FilingContext,
    findings: &mut Vec<Finding>,
) -> Vec<&'a Record> {
    let period = ctx.period.as_yyyymm();
    let key_fields: Vec<&str> = spec
        .grouping
        .record_key
        .iter()
        .filter(|part| part.applies_to(period))
        .map(|part| part.field.as_str())
        .collect();
    if key_fields.is_empty() {
        return records.iter().collect();
    }

    // The reference compares these case-insensitively (it lower-cases the
    // description and the unit before testing them), and the remaining
    // components are codes and numbers where folding case changes nothing.
    let key_of = |record: &Record| -> String {
        key_fields
            .iter()
            .map(|id| record.text(id).to_lowercase())
            .collect::<Vec<_>>()
            .join("\u{1f}")
    };

    let mut kept: Vec<&Record> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for record in records {
        let key = key_of(record);
        let Some(&at) = index.get(&key) else {
            index.insert(key, kept.len());
            kept.push(record);
            continue;
        };
        let (winner, loser) = match spec.grouping.record_conflict {
            // Replaced in place, so the surviving row keeps the earlier row's
            // position — and therefore its serial number.
            Some(RecordConflict::LastWins) | None => {
                let displaced = kept[at];
                kept[at] = record;
                (record, displaced)
            }
            Some(RecordConflict::FirstWins) => (kept[at], record),
        };
        findings.push(Finding {
            sheet_row: loser.sheet_row,
            column: None,
            field: None,
            rule: Some("grouping.record_replaced".into()),
            severity: Severity::Warning,
            message: format!(
                "row {} is dropped: it has the same {} as row {}, and the reference keeps only one. \
                 Amounts are not added together — combine them into one row if both were intended.",
                loser.sheet_row,
                key_fields.join(" + "),
                winner.sheet_row,
            ),
        });
    }
    kept
}

/// Field ids the payload reads directly at this level.
fn mapped_fields(object: &PayloadObject) -> Vec<String> {
    object
        .keys
        .iter()
        .filter_map(|k| match &k.from {
            Source::Field(id) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// Build a grouping key from a record's fields.
///
/// Case folding applies to the INVOICE key only. The reference lower-cases the
/// invoice number when it compares one (`iNum.toLowerCase() == ...`) and
/// compares the counterparty id and export type with `===`, so folding the
/// envelope key too would merge envelopes the reference keeps apart.
fn group_key(spec: &SectionSpec, record: &Record, fields: &[String]) -> String {
    let fold = spec.grouping.invoice_key_case_insensitive
        && std::ptr::eq(fields, spec.grouping.invoice_key.as_slice());
    fields
        .iter()
        .map(|id| {
            let text = record.text(id);
            if fold { text.to_lowercase() } else { text }
        })
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

/// Rows grouped together must agree on the level's own fields.
fn disagreement(
    spec: &SectionSpec,
    head: &Record,
    record: &Record,
    fields: &[String],
    level: &str,
) -> Option<Finding> {
    if spec.grouping.invoice_field_conflict != Some(crate::spec::InvoiceFieldConflict::Error) {
        return None;
    }
    for id in fields {
        // Item-level fields legitimately differ between rows.
        if spec.grouping.item_key.contains(id) {
            continue;
        }
        // Grouping-key fields matched by construction, under whatever folding
        // the grouping rule applies. Comparing them literally here would flag
        // 'inv-005' against 'INV-005' as a conflict when the spec says invoice
        // numbers group case-insensitively.
        if spec.grouping.invoice_key.contains(id) || spec.grouping.envelope_key.contains(id) {
            continue;
        }
        // Equal cells render to equal text, so the usual agreeing row costs no
        // text rendering at all.
        if head.get(id) == record.get(id) {
            continue;
        }
        let (head_text, record_text) = (head.text(id), record.text(id));
        if head_text != record_text {
            let column = spec
                .field(id)
                .map(|f| f.column.clone())
                .unwrap_or_else(|| id.clone());
            return Some(Finding {
                sheet_row: record.sheet_row,
                column: Some(column.clone()),
                field: Some(id.clone()),
                rule: Some("grouping.field_disagreement".into()),
                severity: Severity::Error,
                message: format!(
                    "'{column}' is '{record_text}' here but '{head_text}' on row {}, which belongs to the same {level}",
                    head.sheet_row
                ),
            });
        }
    }
    None
}

/// The one loop that turns a payload-object spec into JSON: resolve each key's
/// source, drop what the omit rules say to drop, insert at the dotted path.
/// `nested` expands whichever child level the caller has; levels it does not
/// have yield `Json::Null`.
fn build(
    object: &PayloadObject,
    record: &Record,
    ctx: &FilingContext,
    index: usize,
    mut nested: impl FnMut(Level, &mut Vec<Finding>) -> Json,
    findings: &mut Vec<Finding>,
) -> Json {
    let mut out = Json::obj();
    for key in &object.keys {
        // A key whose shape changed at a cutover is only emitted for the
        // periods it belongs to.
        if !key.applies_to(ctx.period.as_yyyymm()) {
            continue;
        }
        let value = match &key.from {
            Source::Field(id) => cell_json(record, id),
            Source::Derive(name) => derive(name, leaf(&key.key), record, ctx, index, findings),
            Source::Literal(v) => Json::Str(v.clone()),
            Source::Nested(level) => nested(*level, findings),
        };
        if omitted(key, &value) {
            continue;
        }
        out.insert_path(&key.key, value);
    }
    out
}

fn build_envelope(
    spec: &SectionSpec,
    envelope: &EnvelopeGroup,
    ctx: &FilingContext,
    findings: &mut Vec<Finding>,
) -> Json {
    let Some(envelope_spec) = &spec.output.envelope else {
        return Json::obj();
    };
    let nested = |level: Level, findings: &mut Vec<Finding>| match level {
        Level::Invoice => Json::Arr(
            envelope
                .invoices
                .iter()
                .map(|inv| build_invoice(spec, inv, ctx, findings))
                .collect(),
        ),
        Level::Item => Json::Null,
    };
    build(envelope_spec, &envelope.head, ctx, 0, nested, findings)
}

fn build_invoice(
    spec: &SectionSpec,
    invoice: &InvoiceGroup,
    ctx: &FilingContext,
    findings: &mut Vec<Finding>,
) -> Json {
    let Some(invoice_spec) = &spec.output.invoice else {
        return Json::obj();
    };
    let nested = |level: Level, findings: &mut Vec<Finding>| match level {
        Level::Item => Json::Arr(
            invoice
                .items
                .iter()
                .enumerate()
                .map(|(i, (_, rec))| build_item(spec, rec, ctx, i, findings))
                .collect(),
        ),
        Level::Invoice => Json::Null,
    };
    build(invoice_spec, &invoice.head, ctx, 0, nested, findings)
}

fn build_item(
    spec: &SectionSpec,
    record: &Record,
    ctx: &FilingContext,
    index: usize,
    findings: &mut Vec<Finding>,
) -> Json {
    let Some(item_spec) = &spec.output.item else {
        return Json::obj();
    };
    build(item_spec, record, ctx, index, |_, _| Json::Null, findings)
}

/// Build one flat payload object from a single row. Flat sections compute their
/// tax on the row itself, so there is no item level to descend into.
fn build_object(
    object: &PayloadObject,
    record: &Record,
    ctx: &FilingContext,
    index: usize,
    findings: &mut Vec<Finding>,
) -> Json {
    build(object, record, ctx, index, |_, _| Json::Null, findings)
}

/// Which member of its payload object a record belongs to, for sections that
/// split one sheet across several. `None` when the section does not split.
fn member_of(spec: &SectionSpec, record: &Record) -> Option<String> {
    let split = spec.output.member_from.as_ref()?;
    split.map.get(&record.text(&split.field)).cloned()
}

/// Whether a key is dropped rather than emitted: either because it is empty and
/// declared omit-when-empty, or because it carries exactly the value the
/// reference drops it at.
fn omitted(key: &crate::spec::PayloadKey, value: &Json) -> bool {
    if key.omit_when_empty && value.is_empty() {
        return true;
    }
    let as_text = match value {
        Json::Num(n) => Some(n.normalize().to_string()),
        Json::Str(s) => Some(s.clone()),
        _ => None,
    };
    if let (Some(only), Some(text)) = (&key.only_when_value, as_text.as_deref())
        && !only.matches_text(text)
    {
        return true;
    }
    match (&key.omit_when_value, as_text.as_deref()) {
        (Some(spec_value), Some(text)) => spec_value.matches_text(text),
        _ => false,
    }
}

/// The last segment of a dotted payload key — what a derivation switches on.
fn leaf(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

fn cell_json(record: &Record, field: &str) -> Json {
    use crate::record::Cell;
    match record.get(field) {
        Cell::Empty => Json::Str(String::new()),
        Cell::Number(d) => Json::Num(*d),
        Cell::Text(s) => Json::Str(s.clone()),
        Cell::Date(_) => Json::Str(record.text(field)),
    }
}

/// Whether a line is taxed as intra-state.
///
/// Turns on the *supplier's* state, so the same row yields different output
/// for different filers. An SEZ supplier is always inter-state, and only
/// regular and deemed-export supplies can be intra-state at all.
pub fn is_intra_state(record: &Record, ctx: &FilingContext) -> bool {
    let inv_typ = record.text("inv_typ");
    ctx.supplier_state() == Some(record.text("pos").as_str())
        && matches!(inv_typ.as_str(), "R" | "DE")
        && !ctx.is_sez
}

/// Tax amounts for one line, split by supply type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaxSplit {
    pub iamt: Option<Decimal>,
    pub camt: Option<Decimal>,
    pub samt: Option<Decimal>,
    pub csamt: Decimal,
}

/// The three factors a tax component is built from, held as `f64` because the
/// reference computes in JavaScript numbers and the result is observable.
///
/// Kept unmultiplied so each component can reproduce the reference's own
/// multiplication order — `txval * rt * component * diff_percent`. Floating
/// point multiplication is not associative, so folding `rt * diff_percent`
/// first would land on a different last digit for a `diff_percent` of 0.65.
#[derive(Debug, Clone, Copy)]
struct TaxBase {
    base: f64,
    rate: f64,
    factor: f64,
}

impl TaxBase {
    /// The amount tax is computed on, for one component of the rate.
    /// `diff_percent` is 1 when absent, which it is in every section whose
    /// derivation takes no factor.
    fn of(record: &Record, base_field: &str) -> Self {
        TaxBase {
            base: as_f64(record.number(base_field).unwrap_or_default()),
            rate: as_f64(record.number("rt").unwrap_or_default()),
            factor: record.number("diff_percent").map_or(1.0, as_f64),
        }
    }

    fn component(&self, component: f64) -> Decimal {
        round2(self.base * self.rate * component * self.factor)
    }
}

/// A row's cess, rounded the way every guarded section rounds it.
fn row_cess(record: &Record) -> Decimal {
    round2(as_f64(record.number("csamt").unwrap_or_default()))
}

fn as_f64(value: Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    value.to_f64().unwrap_or(0.0)
}

/// Zero tax throughout — what a supply made without payment of tax carries.
/// Always inter-state, so the zero sits on the integrated component.
const WITHOUT_PAYMENT: TaxSplit = TaxSplit {
    iamt: Some(Decimal::ZERO),
    camt: None,
    samt: None,
    csamt: Decimal::ZERO,
};

/// The arithmetic every split shares: an intra-state line carries central and
/// state tax at half the rate each, an inter-state line integrated tax at the
/// full rate. `both_halves` also emits the inapplicable components as explicit
/// zeros, which only the e-commerce summary does.
fn assemble(taxed: TaxBase, cess: Decimal, intra: bool, both_halves: bool) -> TaxSplit {
    let zero = both_halves.then_some(Decimal::ZERO);
    if intra {
        let half = taxed.component(HALF_RATE);
        TaxSplit {
            iamt: zero,
            camt: Some(half),
            samt: Some(half),
            csamt: cess,
        }
    } else {
        TaxSplit {
            iamt: Some(taxed.component(FULL_RATE)),
            camt: zero,
            samt: zero,
            csamt: cess,
        }
    }
}

/// Compute a line's tax.
///
/// SEZ supplies without payment of tax carry zero throughout, and are always
/// inter-state because the invoice type excludes them from the intra-state
/// branch.
pub fn tax_split(record: &Record, ctx: &FilingContext) -> TaxSplit {
    if record.text("inv_typ") == "SEWOP" {
        return WITHOUT_PAYMENT;
    }
    assemble(
        TaxBase::of(record, "txval"),
        row_cess(record),
        is_intra_state(record, ctx),
        false,
    )
}

/// Whether a line is intra-state judged on place of supply alone.
///
/// Sections without an invoice-type column (B2C small) use this: intra-state
/// when the place of supply is the supplier's own state and the supplier is not
/// an SEZ unit. Unlike [`is_intra_state`], no invoice type gates it, so both
/// branches are genuinely reachable.
pub fn is_intra_state_by_pos(record: &Record, ctx: &FilingContext) -> bool {
    ctx.supplier_state() == Some(record.text("pos").as_str()) && !ctx.is_sez
}

/// Tax split for sections with no invoice-type column.
pub fn tax_split_by_pos(record: &Record, ctx: &FilingContext) -> TaxSplit {
    assemble(
        TaxBase::of(record, "txval"),
        row_cess(record),
        is_intra_state_by_pos(record, ctx),
        false,
    )
}

/// Tax on an e-commerce supply reported by the operator.
///
/// Unlike every other section, both halves are emitted at once: an inter-state
/// row carries `iamt` alongside `camt: 0` and `samt: 0`, and an intra-state row
/// carries the split alongside `iamt: 0`. Confirmed against a captured file.
pub fn tax_split_ecom(record: &Record, ctx: &FilingContext) -> TaxSplit {
    assemble(
        TaxBase::of(record, "txval"),
        row_cess(record),
        is_intra_state_by_pos(record, ctx),
        true,
    )
}

/// Tax on an advance received or adjusted.
///
/// Same central/state-versus-integrated choice as [`tax_split_by_pos`], but
/// computed on the advance (`ad_amt`) rather than a taxable value, because
/// these tables carry no invoice.
pub fn tax_split_advance(record: &Record, ctx: &FilingContext) -> TaxSplit {
    assemble(
        TaxBase::of(record, "ad_amt"),
        row_cess(record),
        is_intra_state_by_pos(record, ctx),
        false,
    )
}

/// Tax for a note issued to an unregistered person.
///
/// Always inter-state: the reference has no central/state branch for these at
/// all. An export without payment of tax (`EXPWOP`) zeroes both the integrated
/// tax and the cess.
pub fn tax_split_unregistered(record: &Record, _ctx: &FilingContext) -> TaxSplit {
    if record.text("typ") == "EXPWOP" {
        return WITHOUT_PAYMENT;
    }
    assemble(TaxBase::of(record, "txval"), row_cess(record), false, false)
}

/// Cess for the two B2C(Large) tables, which compute it without the empty-cell
/// guard every other section uses.
///
/// A blank cell there yields NaN rather than zero, which the reference's working
/// file writes as `null` and the upload step's omit-empty then drops — so the
/// key is simply absent. An explicit `0` in the cell is a real zero and is
/// emitted. Confirmed against a file captured from the tool.
pub fn cess_unguarded(record: &Record) -> Json {
    if record.text("csamt").trim().is_empty() {
        return Json::Null;
    }
    Json::Num(round2(as_f64(record.number("csamt").unwrap_or_default())))
}

/// Tax on an export invoice.
///
/// Always inter-state — a supply out of India has no central/state branch to
/// take. An export made *without* payment of tax (`WOPAY`) zeroes both the
/// integrated tax and the cess whatever rate the row carries, the same way a
/// note to an unregistered person does.
pub fn tax_export(record: &Record, _ctx: &FilingContext) -> TaxSplit {
    if record.text("exp_typ") == "WOPAY" {
        return WITHOUT_PAYMENT;
    }
    assemble(TaxBase::of(record, "txval"), row_cess(record), false, false)
}

/// Money rounds to two places exactly as the reference does it.
///
/// The reference is JavaScript: `parseFloat(x.toFixed(2))`, where `x` is a
/// binary double. `toFixed` rounds half away from zero on the double's *exact*
/// value, not on the decimal literal the double was written as — and a double
/// almost never sits exactly on a decimal midpoint. `10000.10 * 5 * 0.01` reads
/// as 500.005 but the double is 500.00499999999999545…, so the reference emits
/// 500.00 where exact-decimal arithmetic gives 500.01.
///
/// Reproducing that means expanding the double to its exact decimal value and
/// rounding half-away-from-zero there. `from_f64_retain` gives that exact
/// expansion, so the midpoint test lands on the same side as the reference's.
fn round2(value: f64) -> Decimal {
    Decimal::from_f64_retain(value)
        .unwrap_or_default()
        .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// Whether a row's HSN column holds a SAC — a service code. The tables mark
/// these by a leading `99`, and the reference tests exactly that prefix.
fn is_service_code(record: &Record) -> bool {
    record.text("hsn_sc").starts_with("99")
}

/// A line item's number, derived from its rate rather than its position:
/// `rt * 100 + 1`. Rate 18 gives 1801.
pub fn item_num(record: &Record) -> Decimal {
    let rate = record.number("rt").unwrap_or_default();
    (rate * Decimal::from(100)).trunc() + Decimal::ONE
}

type TaxSplitFn = fn(&Record, &FilingContext) -> TaxSplit;

/// The tax-split derivations, one per sectioning of the intra/inter rule.
/// This table is the single list `derive` dispatches on and
/// `unimplemented_derivations` checks against.
const TAX_SPLITS: &[(&str, TaxSplitFn)] = &[
    ("gstr1.tax_split", tax_split),
    ("gstr1.tax_split_by_pos", tax_split_by_pos),
    ("gstr1.tax_split_unregistered", tax_split_unregistered),
    ("gstr1.tax_export", tax_export),
    ("gstr1.tax_split_advance", tax_split_advance),
    ("gstr1.tax_split_ecom", tax_split_ecom),
];

fn derive(
    name: &str,
    leaf_key: &str,
    record: &Record,
    ctx: &FilingContext,
    index: usize,
    findings: &mut Vec<Finding>,
) -> Json {
    if let Some((_, split_fn)) = TAX_SPLITS.iter().find(|(n, _)| *n == name) {
        let split = split_fn(record, ctx);
        let amount = match leaf_key {
            "iamt" => split.iamt,
            "camt" => split.camt,
            "samt" => split.samt,
            "csamt" => Some(split.csamt),
            other => {
                findings.push(Finding {
                    sheet_row: record.sheet_row,
                    column: None,
                    field: None,
                    rule: Some("output.unknown_tax_component".into()),
                    severity: Severity::Error,
                    message: format!(
                        "spec maps '{other}' to {name}, which only provides iamt, camt, samt and csamt"
                    ),
                });
                None
            }
        };
        // Absent components are omitted, which is what makes the payload
        // carry either iamt or camt+samt but never both.
        return amount.map_or(Json::Null, Json::Num);
    }
    match name {
        "gstr1.item_num" => Json::Num(item_num(record)),
        "gstr1.cess" => Json::Num(tax_split(record, ctx).csamt),
        "gstr1.cess_unguarded" => cess_unguarded(record),
        // The official description for the row's HSN code, looked up rather
        // than entered — the template has no column for it.
        "gstr1.hsn_description" => match crate::masters::hsn_description(&record.text("hsn_sc")) {
            Some(desc) => Json::Str(desc.to_owned()),
            None => Json::Str(String::new()),
        },
        // 1-based position of this record in row order. Used where the payload
        // carries a serial rather than a rate-derived number.
        "gstr1.record_serial" => Json::Num(Decimal::from(index + 1)),
        // A SAC — a service code, which every HSN table marks by a leading 99 —
        // has no unit and no quantity. The reference rewrites both cells before
        // validation rather than checking them (offline2.js:390, :399), so a
        // filer's own unit and quantity on a service line are discarded.
        "gstr1.hsn_uqc" => Json::Str(if is_service_code(record) {
            "NA".to_owned()
        } else {
            record.text("uqc")
        }),
        "gstr1.hsn_quantity" => {
            if is_service_code(record) {
                Json::Num(Decimal::ZERO)
            } else {
                cell_json(record, "qty")
            }
        }
        // 1-based index of the document nature in the master's fixed order.
        "gstr1.document_number" => {
            let typ = record.text("doc_typ");
            match crate::masters::DOCUMENT_TYPES
                .iter()
                .position(|t| *t == typ)
            {
                Some(i) => Json::Num(Decimal::from(i + 1)),
                None => {
                    findings.push(Finding {
                        sheet_row: record.sheet_row,
                        column: None,
                        field: None,
                        rule: Some("output.unknown_document_type".into()),
                        severity: Severity::Error,
                        message: format!("'{typ}' is not a known nature of document"),
                    });
                    Json::Null
                }
            }
        }
        // Documents actually issued: total less cancelled.
        "gstr1.net_issue" => {
            let total = record.number("totnum").unwrap_or_default();
            let cancelled = record.number("cancel").unwrap_or_default();
            Json::Num(total - cancelled)
        }
        // The MMYYYY period an amendment corrects, from its financial year and
        // spelled-out month.
        "gstr1.original_period" => {
            let fy = record.text("fy");
            let month = record.text("omonth");
            match crate::date::period_from_financial_year(&fy, &month) {
                Some(period) => Json::Str(period),
                None => {
                    findings.push(Finding {
                        sheet_row: record.sheet_row,
                        column: None,
                        field: None,
                        rule: Some("output.original_period_unresolvable".into()),
                        severity: Severity::Error,
                        message: format!(
                            "cannot work out which period '{month}' of '{fy}' refers to"
                        ),
                    });
                    Json::Null
                }
            }
        }
        // 'INTRA' or 'INTER', which flat sections carry as a field of their own.
        "gstr1.supply_type" => Json::Str(
            if is_intra_state_by_pos(record, ctx) {
                "INTRA"
            } else {
                "INTER"
            }
            .to_owned(),
        ),
        other => {
            findings.push(Finding {
                sheet_row: record.sheet_row,
                column: None,
                field: None,
                rule: Some("output.unknown_derivation".into()),
                severity: Severity::Error,
                message: format!(
                    "spec names derivation '{other}', which the engine does not implement"
                ),
            });
            Json::Null
        }
    }
}

/// Confirm every derivation a spec names is implemented. Cheap guard against a
/// spec file referring to a computation that does not exist.
pub fn unimplemented_derivations(spec: &SectionSpec) -> Vec<&str> {
    /// The scalar derivations `derive` matches by name; the tax splits come
    /// from [`TAX_SPLITS`], so a new split registers in one place.
    const SCALAR: &[&str] = &[
        "gstr1.item_num",
        "gstr1.record_serial",
        "gstr1.hsn_description",
        "gstr1.hsn_uqc",
        "gstr1.hsn_quantity",
        "gstr1.document_number",
        "gstr1.net_issue",
        "gstr1.original_period",
        "gstr1.supply_type",
        "gstr1.cess",
        "gstr1.cess_unguarded",
    ];
    spec.output
        .derivations
        .iter()
        .map(String::as_str)
        .filter(|d| !SCALAR.contains(d) && !TAX_SPLITS.iter().any(|(n, _)| n == d))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cases measured against the running reference tool. Each is a taxable
    /// value whose exact product lands on a decimal midpoint, where the
    /// reference's double does not.
    #[test]
    fn rounding_follows_the_reference_not_exact_decimal_arithmetic() {
        // (base, rate, component, what the reference emits)
        let cases = [
            (10000.10, 5.0, FULL_RATE, "500"),
            (10000.75, 18.0, FULL_RATE, "1800.13"),
            (10000.20, 5.0, HALF_RATE, "250"),
            (10001.50, 18.0, HALF_RATE, "900.13"),
            // Not every midpoint falls the same way: this double sits *above*
            // its decimal midpoint, so here the reference rounds up and the two
            // models happen to agree.
            (10000.50, 18.0, HALF_RATE, "900.05"),
        ];
        for (base, rate, component, expected) in cases {
            let taxed = TaxBase {
                base,
                rate,
                factor: 1.0,
            };
            assert_eq!(
                taxed.component(component).normalize().to_string(),
                expected,
                "{base} at {rate}% x {component}"
            );
        }
    }

    #[test]
    fn rounding_is_symmetric_about_zero() {
        assert_eq!(round2(-0.125).to_string(), "-0.13");
        assert_eq!(round2(0.125).to_string(), "0.13");
        assert_eq!(round2(0.0).normalize().to_string(), "0");
    }

    /// `diff_percent` is applied last, as the reference writes it. Folding it
    /// into the rate first is a different double and can differ in the last
    /// place.
    /// `diff_percent` is applied last, as the reference writes it. Folding it
    /// into the rate first is a different double, and the difference survives
    /// rounding: 18.00 at 5% and 65% is 0.59 in the reference's order and 0.58
    /// with the factor folded in early.
    #[test]
    fn the_applicable_percentage_is_applied_in_the_reference_order() {
        let taxed = TaxBase {
            base: 18.0,
            rate: 5.0,
            factor: 0.65,
        };
        assert_eq!(taxed.component(FULL_RATE).to_string(), "0.59");

        let folded = round2(18.0f64 * (5.0 * 0.65) * FULL_RATE);
        assert_eq!(folded.to_string(), "0.58");
    }
}
