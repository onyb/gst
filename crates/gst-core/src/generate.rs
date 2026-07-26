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
use crate::spec::{ItemConflict, Level, PayloadObject, SectionSpec, Severity, Source};
use crate::validate::{FilingContext, Finding};

/// Half of a percent, i.e. the CGST or SGST share of a combined rate.
const HALF_RATE: Decimal = Decimal::from_parts(5, 0, 0, false, 3); // 0.005
const FULL_RATE: Decimal = Decimal::from_parts(1, 0, 0, false, 2); // 0.01

/// Grouped output for one section.
#[derive(Debug, Clone, Default)]
pub struct Generated {
    /// One entry per envelope, in first-seen order.
    pub envelopes: Vec<Json>,
    /// Problems that only surface once rows are grouped.
    pub findings: Vec<Finding>,
}

impl Generated {
    /// The section's payload as the portal carries it: an array of envelopes.
    pub fn to_json(&self) -> String {
        Json::Arr(self.envelopes.clone()).to_json()
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
    invoices: Vec<(String, InvoiceGroup)>,
}

/// Build the payload for a section from records that passed validation.
pub fn generate(spec: &SectionSpec, records: &[Record], ctx: &FilingContext) -> Generated {
    let mut out = Generated::default();

    // Flat sections have no invoice or item level: every validated row becomes
    // one payload object, in row order, with its tax computed on the row.
    if let Some(record_spec) = &spec.output.record {
        for (index, record) in records.iter().enumerate() {
            let json = build_object(record_spec, record, ctx, index, &mut out.findings);
            out.envelopes.push(json);
        }
        return out;
    }

    let mut envelopes: Vec<(String, EnvelopeGroup)> = Vec::new();
    let mut envelope_index: HashMap<String, usize> = HashMap::new();

    // Invoice-level fields are those the payload reads at the invoice or
    // envelope level. Rows sharing an invoice key must agree on all of them.
    let invoice_fields = spec
        .output
        .invoice
        .as_ref()
        .map(mapped_fields)
        .unwrap_or_default();
    let envelope_fields = spec
        .output
        .envelope
        .as_ref()
        .map(mapped_fields)
        .unwrap_or_default();

    for record in records {
        let env_key = group_key(spec, record, &spec.grouping.envelope_key);
        let env_pos = *envelope_index.entry(env_key.clone()).or_insert_with(|| {
            envelopes.push((
                env_key.clone(),
                EnvelopeGroup {
                    head: record.clone(),
                    invoices: Vec::new(),
                },
            ));
            envelopes.len() - 1
        });
        let envelope = &mut envelopes[env_pos].1;

        if let Some(finding) =
            disagreement(spec, &envelope.head, record, &envelope_fields, "recipient")
        {
            out.findings.push(finding);
            continue;
        }

        let inv_key = group_key(spec, record, &spec.grouping.invoice_key);
        let inv_pos = envelope.invoices.iter().position(|(k, _)| *k == inv_key);
        let inv_pos = match inv_pos {
            Some(pos) => pos,
            None => {
                envelope.invoices.push((
                    inv_key,
                    InvoiceGroup {
                        head: record.clone(),
                        items: Vec::new(),
                    },
                ));
                envelope.invoices.len() - 1
            }
        };
        let invoice = &mut envelope.invoices[inv_pos].1;

        if let Some(finding) = disagreement(spec, &invoice.head, record, &invoice_fields, "invoice")
        {
            out.findings.push(finding);
            continue;
        }

        if spec.grouping.item_key.is_empty() {
            continue;
        }
        let item_key = group_key(spec, record, &spec.grouping.item_key);
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
        for (_, envelope) in envelopes {
            for (_, invoice) in &envelope.invoices {
                out.envelopes
                    .push(build_invoice(spec, invoice, ctx, &mut out.findings));
            }
        }
        return out;
    }

    for (_, envelope) in envelopes {
        out.envelopes
            .push(build_envelope(spec, &envelope, ctx, &mut out.findings));
    }
    out
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

fn group_key(spec: &SectionSpec, record: &Record, fields: &[String]) -> String {
    fields
        .iter()
        .map(|id| {
            let text = record.text(id);
            if spec.grouping.invoice_key_case_insensitive {
                text.to_lowercase()
            } else {
                text
            }
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
        if head.text(id) != record.text(id) {
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
                    "'{column}' is '{}' here but '{}' on row {}, which belongs to the same {level}",
                    record.text(id),
                    head.text(id),
                    head.sheet_row
                ),
            });
        }
    }
    None
}

fn build_envelope(
    spec: &SectionSpec,
    envelope: &EnvelopeGroup,
    ctx: &FilingContext,
    findings: &mut Vec<Finding>,
) -> Json {
    let mut out = Json::obj();
    let Some(envelope_spec) = &spec.output.envelope else {
        return out;
    };
    for key in &envelope_spec.keys {
        let value = match &key.from {
            Source::Field(id) => cell_json(&envelope.head, id),
            Source::Derive(name) => derive(name, leaf(&key.key), &envelope.head, ctx, 0, findings),
            Source::Nested(Level::Invoice) => Json::Arr(
                envelope
                    .invoices
                    .iter()
                    .map(|(_, inv)| build_invoice(spec, inv, ctx, findings))
                    .collect(),
            ),
            Source::Nested(Level::Item) => Json::Null,
        };
        if omitted(key, &value) {
            continue;
        }
        out.insert_path(&key.key, value);
    }
    out
}

fn build_invoice(
    spec: &SectionSpec,
    invoice: &InvoiceGroup,
    ctx: &FilingContext,
    findings: &mut Vec<Finding>,
) -> Json {
    let mut out = Json::obj();
    let Some(invoice_spec) = &spec.output.invoice else {
        return out;
    };
    for key in &invoice_spec.keys {
        let value = match &key.from {
            Source::Field(id) => cell_json(&invoice.head, id),
            Source::Derive(name) => derive(name, leaf(&key.key), &invoice.head, ctx, 0, findings),
            Source::Nested(Level::Item) => Json::Arr(
                invoice
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, (_, rec))| build_item(spec, rec, ctx, i, findings))
                    .collect(),
            ),
            Source::Nested(Level::Invoice) => Json::Null,
        };
        if omitted(key, &value) {
            continue;
        }
        out.insert_path(&key.key, value);
    }
    out
}

fn build_item(
    spec: &SectionSpec,
    record: &Record,
    ctx: &FilingContext,
    index: usize,
    findings: &mut Vec<Finding>,
) -> Json {
    let mut out = Json::obj();
    let Some(item_spec) = &spec.output.item else {
        return out;
    };
    for key in &item_spec.keys {
        let value = match &key.from {
            Source::Field(id) => cell_json(record, id),
            Source::Derive(name) => derive(name, leaf(&key.key), record, ctx, index, findings),
            Source::Nested(_) => Json::Null,
        };
        if omitted(key, &value) {
            continue;
        }
        out.insert_path(&key.key, value);
    }
    out
}

/// Build one flat payload object from a single row. Flat sections compute their
/// tax on the row itself, so there is no item level to descend into.
fn build_object(
    object: &crate::spec::PayloadObject,
    record: &Record,
    ctx: &FilingContext,
    index: usize,
    findings: &mut Vec<Finding>,
) -> Json {
    let mut out = Json::obj();
    for key in &object.keys {
        let value = match &key.from {
            Source::Field(id) => cell_json(record, id),
            Source::Derive(name) => derive(name, leaf(&key.key), record, ctx, index, findings),
            Source::Nested(_) => Json::Null,
        };
        if omitted(key, &value) {
            continue;
        }
        out.insert_path(&key.key, value);
    }
    out
}

/// Whether a key is dropped rather than emitted: either because it is empty and
/// declared omit-when-empty, or because it carries exactly the value the
/// reference drops it at.
fn omitted(key: &crate::spec::PayloadKey, value: &Json) -> bool {
    if key.omit_when_empty && value.is_empty() {
        return true;
    }
    match (&key.omit_when_value, value) {
        (Some(spec_value), Json::Num(n)) => spec_value.matches_text(&n.normalize().to_string()),
        (Some(spec_value), Json::Str(s)) => spec_value.matches_text(s),
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

/// Compute a line's tax.
///
/// Intra-state lines carry central and state tax at half the rate each;
/// inter-state lines carry integrated tax at the full rate. SEZ supplies
/// without payment of tax carry zero throughout, and are always inter-state
/// because the invoice type excludes them from the intra-state branch.
pub fn tax_split(record: &Record, ctx: &FilingContext) -> TaxSplit {
    let txval = record.number("txval").unwrap_or_default();
    let rate = record.number("rt").unwrap_or_default();
    let factor = record.number("diff_percent").unwrap_or(Decimal::ONE);
    let without_payment = record.text("inv_typ") == "SEWOP";

    let cess = if without_payment {
        Decimal::ZERO
    } else {
        round2(record.number("csamt").unwrap_or_default())
    };

    if is_intra_state(record, ctx) {
        let half = round2(txval * rate * HALF_RATE * factor);
        TaxSplit {
            iamt: None,
            camt: Some(half),
            samt: Some(half),
            csamt: cess,
        }
    } else {
        let full = if without_payment {
            Decimal::ZERO
        } else {
            round2(txval * rate * FULL_RATE * factor)
        };
        TaxSplit {
            iamt: Some(full),
            camt: None,
            samt: None,
            csamt: cess,
        }
    }
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
    let txval = record.number("txval").unwrap_or_default();
    let rate = record.number("rt").unwrap_or_default();
    let factor = record.number("diff_percent").unwrap_or(Decimal::ONE);
    let cess = round2(record.number("csamt").unwrap_or_default());

    if is_intra_state_by_pos(record, ctx) {
        let half = round2(txval * rate * HALF_RATE * factor);
        TaxSplit {
            iamt: None,
            camt: Some(half),
            samt: Some(half),
            csamt: cess,
        }
    } else {
        TaxSplit {
            iamt: Some(round2(txval * rate * FULL_RATE * factor)),
            camt: None,
            samt: None,
            csamt: cess,
        }
    }
}

/// Tax for a note issued to an unregistered person.
///
/// Always inter-state: the reference has no central/state branch for these at
/// all. An export without payment of tax (`EXPWOP`) zeroes both the integrated
/// tax and the cess.
pub fn tax_split_unregistered(record: &Record, _ctx: &FilingContext) -> TaxSplit {
    let without_payment = record.text("typ") == "EXPWOP";
    let cess = if without_payment {
        Decimal::ZERO
    } else {
        round2(record.number("csamt").unwrap_or_default())
    };
    let iamt = if without_payment {
        Decimal::ZERO
    } else {
        let txval = record.number("txval").unwrap_or_default();
        let rate = record.number("rt").unwrap_or_default();
        let factor = record.number("diff_percent").unwrap_or(Decimal::ONE);
        round2(txval * rate * FULL_RATE * factor)
    };
    TaxSplit {
        iamt: Some(iamt),
        camt: None,
        samt: None,
        csamt: cess,
    }
}

/// Money rounds to two places, away from zero at the midpoint.
fn round2(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// A line item's number, derived from its rate rather than its position:
/// `rt * 100 + 1`. Rate 18 gives 1801.
pub fn item_num(record: &Record) -> Decimal {
    let rate = record.number("rt").unwrap_or_default();
    (rate * Decimal::from(100)).trunc() + Decimal::ONE
}

fn derive(
    name: &str,
    leaf_key: &str,
    record: &Record,
    ctx: &FilingContext,
    _index: usize,
    findings: &mut Vec<Finding>,
) -> Json {
    match name {
        "gstr1.item_num" => Json::Num(item_num(record)),
        "gstr1.cess" => Json::Num(tax_split(record, ctx).csamt),
        // The official description for the row's HSN code, looked up rather
        // than entered — the template has no column for it.
        "gstr1.hsn_description" => match crate::masters::hsn_description(&record.text("hsn_sc")) {
            Some(desc) => Json::Str(desc.to_owned()),
            None => Json::Str(String::new()),
        },
        // 1-based position of this record in row order. Used where the payload
        // carries a serial rather than a rate-derived number.
        "gstr1.record_serial" => Json::Num(Decimal::from(_index + 1)),
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
        "gstr1.tax_split" | "gstr1.tax_split_by_pos" | "gstr1.tax_split_unregistered" => {
            let split = match name {
                "gstr1.tax_split_by_pos" => tax_split_by_pos(record, ctx),
                "gstr1.tax_split_unregistered" => tax_split_unregistered(record, ctx),
                _ => tax_split(record, ctx),
            };
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
            amount.map_or(Json::Null, Json::Num)
        }
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
    const KNOWN: &[&str] = &[
        "gstr1.item_num",
        "gstr1.record_serial",
        "gstr1.hsn_description",
        "gstr1.document_number",
        "gstr1.net_issue",
        "gstr1.original_period",
        "gstr1.tax_split",
        "gstr1.tax_split_by_pos",
        "gstr1.tax_split_unregistered",
        "gstr1.supply_type",
        "gstr1.cess",
    ];
    spec.output
        .derivations
        .iter()
        .map(String::as_str)
        .filter(|d| !KNOWN.contains(d))
        .collect()
}
