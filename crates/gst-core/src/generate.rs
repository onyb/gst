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
    let mut envelopes: Vec<(String, EnvelopeGroup)> = Vec::new();
    let mut envelope_index: HashMap<String, usize> = HashMap::new();

    // Invoice-level fields are those the payload reads at the invoice or
    // envelope level. Rows sharing an invoice key must agree on all of them.
    let invoice_fields = mapped_fields(&spec.output.invoice);
    let envelope_fields = mapped_fields(&spec.output.envelope);

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
    for key in &spec.output.envelope.keys {
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
        if key.omit_when_empty && value.is_empty() {
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
    for key in &spec.output.invoice.keys {
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
        if key.omit_when_empty && value.is_empty() {
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
        if key.omit_when_empty && value.is_empty() {
            continue;
        }
        out.insert_path(&key.key, value);
    }
    out
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
        "gstr1.tax_split" => {
            let split = tax_split(record, ctx);
            let amount = match leaf_key {
                "iamt" => split.iamt,
                "camt" => split.camt,
                "samt" => split.samt,
                other => {
                    findings.push(Finding {
                        sheet_row: record.sheet_row,
                        column: None,
                        field: None,
                        rule: Some("output.unknown_tax_component".into()),
                        severity: Severity::Error,
                        message: format!("spec maps '{other}' to gstr1.tax_split, which only provides iamt, camt and samt"),
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
    const KNOWN: &[&str] = &["gstr1.item_num", "gstr1.tax_split", "gstr1.cess"];
    spec.output
        .derivations
        .iter()
        .map(String::as_str)
        .filter(|d| !KNOWN.contains(d))
        .collect()
}
