//! Section specifications loaded from `spec/`.
//!
//! These types mirror `spec/section.schema.json` one-for-one. Everything the
//! engine knows about a return section — its columns, its rules, how rows
//! group, and what the upload payload looks like — arrives through here, so
//! adding a section is a matter of writing JSON, not Rust.

use std::sync::LazyLock;

use rust_decimal::Decimal;
use serde::Deserialize;

/// A literal drawn from a spec file. Spec JSON mixes strings and numbers
/// freely (`"eq": "CBW"`, `"enum": [100, 65]`), and comparisons need to treat
/// `65` and `"65.00"` as equal, so both forms are kept and compared
/// numerically whenever both sides parse as numbers.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SpecValue {
    Number(Decimal),
    Text(String),
}

impl SpecValue {
    pub fn as_text(&self) -> String {
        match self {
            SpecValue::Number(d) => d.normalize().to_string(),
            SpecValue::Text(s) => s.clone(),
        }
    }

    /// Compare against a cell's text, numerically when both sides are numbers.
    /// `100` from the spec must match a cell reading `100.00`.
    pub fn matches_text(&self, text: &str) -> bool {
        match (self, text.parse::<Decimal>()) {
            (SpecValue::Number(spec), Ok(cell)) => *spec == cell,
            (SpecValue::Text(s), Ok(cell)) => s.parse::<Decimal>().is_ok_and(|s| s == cell),
            _ => self.as_text() == text,
        }
    }
}

/// A field's value domain — independent of which values are allowed, which
/// `enum`/`enum_ref` express. The distinction matters for output: a `Decimal`
/// becomes a JSON number, `Text` a JSON string, so a numeric field with an
/// enumerated domain (a tax rate) must still be typed `Decimal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Decimal,
    Date,
    Gstin,
    StateCode,
}

/// Which registration-number forms a `gstin` field accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GstinForm {
    Gstin,
    Uin,
    Tds,
    Nrtp,
    Eco,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    pub id: String,
    /// Column header text, matched exactly after trimming.
    pub column: String,
    /// 0-based column position in the template.
    pub order: usize,
    #[serde(rename = "type")]
    pub ty: FieldType,
    pub required: bool,
    /// Substituted when the cell is empty, before validation.
    pub default: Option<SpecValue>,
    pub pattern: Option<String>,
    #[serde(default)]
    pub accepts: Vec<GstinForm>,
    pub enum_ref: Option<String>,
    #[serde(rename = "enum")]
    pub allowed: Option<Vec<SpecValue>>,
    pub max_length: Option<usize>,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub transform: Option<String>,
    #[serde(default)]
    pub must_be_empty: bool,
    pub description: Option<String>,
}

/// Boolean expression over a row's fields.
///
/// Deliberately not an expression language: the shapes real sections need are
/// small, and a walked structure keeps spec files readable and the evaluator
/// free of a parser.
#[derive(Debug, Clone)]
pub enum Predicate {
    Eq {
        field: String,
        value: SpecValue,
    },
    Ne {
        field: String,
        value: SpecValue,
    },
    In {
        field: String,
        values: Vec<SpecValue>,
    },
    Empty {
        field: String,
        empty: bool,
    },
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
    Not(Box<Predicate>),
}

/// Wire form of [`Predicate`]: every branch is an optional key, validated on
/// conversion. Mirrors the `oneOf` in the meta-schema.
#[derive(Debug, Deserialize)]
pub struct RawPredicate {
    field: Option<String>,
    eq: Option<SpecValue>,
    ne: Option<SpecValue>,
    #[serde(rename = "in")]
    in_: Option<Vec<SpecValue>>,
    empty: Option<bool>,
    all: Option<Vec<RawPredicate>>,
    any: Option<Vec<RawPredicate>>,
    not: Option<Box<RawPredicate>>,
}

impl TryFrom<RawPredicate> for Predicate {
    type Error = String;

    fn try_from(raw: RawPredicate) -> Result<Self, Self::Error> {
        if let Some(preds) = raw.all {
            return Ok(Predicate::All(convert_all(preds)?));
        }
        if let Some(preds) = raw.any {
            return Ok(Predicate::Any(convert_all(preds)?));
        }
        if let Some(pred) = raw.not {
            return Ok(Predicate::Not(Box::new(Predicate::try_from(*pred)?)));
        }
        let field = raw
            .field
            .ok_or("predicate has no `field` and is not all/any/not")?;
        match (raw.eq, raw.ne, raw.in_, raw.empty) {
            (Some(value), None, None, None) => Ok(Predicate::Eq { field, value }),
            (None, Some(value), None, None) => Ok(Predicate::Ne { field, value }),
            (None, None, Some(values), None) => Ok(Predicate::In { field, values }),
            (None, None, None, Some(empty)) => Ok(Predicate::Empty { field, empty }),
            _ => Err(format!(
                "predicate on `{field}` must have exactly one of eq/ne/in/empty"
            )),
        }
    }
}

fn convert_all(preds: Vec<RawPredicate>) -> Result<Vec<Predicate>, String> {
    preds.into_iter().map(Predicate::try_from).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    #[default]
    Error,
    Warning,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    /// Section-namespaced identifier, e.g. `b2b.cbw_requires_rchrg`.
    pub id: String,
    pub description: Option<String>,
    /// Guard. Absent means the assertion always applies.
    #[serde(default, deserialize_with = "opt_predicate")]
    pub when: Option<Predicate>,
    #[serde(deserialize_with = "predicate")]
    pub assert: Predicate,
    pub message: String,
    #[serde(default)]
    pub severity: Severity,
}

fn predicate<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Predicate, D::Error> {
    let raw = RawPredicate::deserialize(d)?;
    Predicate::try_from(raw).map_err(serde::de::Error::custom)
}

fn opt_predicate<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Predicate>, D::Error> {
    let raw = Option::<RawPredicate>::deserialize(d)?;
    raw.map(Predicate::try_from)
        .transpose()
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemConflict {
    /// The later row silently replaces the earlier one — no summing, no error.
    LastWins,
    Sum,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceFieldConflict {
    Error,
    FirstWins,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Grouping {
    #[serde(default)]
    pub envelope_key: Vec<String>,
    pub invoice_key: Vec<String>,
    #[serde(default)]
    pub invoice_key_case_insensitive: bool,
    #[serde(default)]
    pub item_key: Vec<String>,
    pub item_conflict: Option<ItemConflict>,
    pub invoice_field_conflict: Option<InvoiceFieldConflict>,
}

/// Where a payload key's content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Field(String),
    Derive(String),
    Nested(Level),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Invoice,
    Item,
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.split_once(':') {
            Some(("field", name)) => Ok(Source::Field(name.to_owned())),
            Some(("derive", name)) => Ok(Source::Derive(name.to_owned())),
            Some(("nested", "invoice")) => Ok(Source::Nested(Level::Invoice)),
            Some(("nested", "item")) => Ok(Source::Nested(Level::Item)),
            _ => Err(serde::de::Error::custom(format!(
                "unrecognized payload source `{s}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verify {
    /// Presence or exact form unconfirmed; must be settled against the
    /// official tool before the section is trusted.
    Oracle,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PayloadKey {
    /// Key name in the upload JSON. A dot nests: `itm_det.txval`.
    pub key: String,
    pub from: Source,
    #[serde(default)]
    pub omit_when_empty: bool,
    pub verify: Option<Verify>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PayloadObject {
    pub keys: Vec<PayloadKey>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Output {
    pub envelope: PayloadObject,
    pub invoice: PayloadObject,
    pub item: Option<PayloadObject>,
    #[serde(default)]
    pub derivations: Vec<String>,
    #[serde(default)]
    pub derivation_notes: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// Reference behavior that is surprising but reproduced deliberately.
    Quirk,
    /// Unresolved; needs oracle capture.
    OpenQuestion,
    /// Where this implementation intentionally differs.
    Divergence,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Note {
    pub kind: NoteKind,
    pub text: String,
    #[serde(default)]
    pub affects: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExcelSource {
    pub sheet: String,
    pub header_row: usize,
    /// 1-based row of the first record. A 0-based record index `i` sits on
    /// sheet row `i + first_data_row`, which is what error reports quote.
    pub first_data_row: usize,
    pub max_data_row: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CsvSource {
    pub file: String,
    pub header_row: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceSpec {
    pub excel: Option<ExcelSource>,
    pub csv: Option<CsvSource>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provenance {
    pub offline_tool: Option<String>,
    pub excel_template: Option<String>,
    pub csv_template: Option<String>,
    pub verified_on: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SectionSpec {
    pub section: String,
    #[serde(rename = "return")]
    pub return_type: String,
    pub title: String,
    pub provenance: Option<Provenance>,
    pub source: SourceSpec,
    pub fields: Vec<Field>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    pub grouping: Grouping,
    pub output: Output,
    #[serde(default)]
    pub notes: Vec<Note>,
}

impl SectionSpec {
    pub fn field(&self, id: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// Column headers in template order — what an importer matches against.
    pub fn columns(&self) -> Vec<&str> {
        let mut fields: Vec<&Field> = self.fields.iter().collect();
        fields.sort_by_key(|f| f.order);
        fields.iter().map(|f| f.column.as_str()).collect()
    }

    /// Payload keys still awaiting differential capture. A section with any of
    /// these is not yet trustworthy for real filing.
    pub fn unverified_keys(&self) -> Vec<&str> {
        let objects = [
            Some(&self.output.envelope),
            Some(&self.output.invoice),
            self.output.item.as_ref(),
        ];
        objects
            .into_iter()
            .flatten()
            .flat_map(|o| &o.keys)
            .filter(|k| k.verify.is_some())
            .map(|k| k.key.as_str())
            .collect()
    }
}

pub static GSTR1_B2B: LazyLock<SectionSpec> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../spec/gstr1/b2b.json"))
        .expect("embedded spec gstr1/b2b.json is invalid")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b2b_spec_loads_with_template_column_order() {
        let spec = &*GSTR1_B2B;
        assert_eq!(spec.section, "b2b");
        assert_eq!(
            spec.columns(),
            [
                "GSTIN/UIN of Recipient",
                "Receiver Name",
                "Invoice Number",
                "Invoice date",
                "Invoice Value",
                "Place Of Supply",
                "Reverse Charge",
                "Applicable % of Tax Rate",
                "Invoice Type",
                "E-Commerce GSTIN",
                "Rate",
                "Taxable Value",
                "Cess Amount",
            ]
        );
    }

    #[test]
    fn b2b_rules_parse_into_predicates() {
        let spec = &*GSTR1_B2B;
        let cbw = spec
            .rules
            .iter()
            .find(|r| r.id == "b2b.cbw_requires_rchrg")
            .expect("cbw rule present");
        assert!(matches!(cbw.when, Some(Predicate::Eq { .. })));
        assert!(matches!(cbw.assert, Predicate::Eq { .. }));

        // The unconditional rule has no guard.
        let etin = spec
            .rules
            .iter()
            .find(|r| r.id == "b2b.etin_not_allowed")
            .expect("etin rule present");
        assert!(etin.when.is_none());
    }

    #[test]
    fn b2b_grouping_records_the_last_wins_merge() {
        let g = &GSTR1_B2B.grouping;
        assert_eq!(g.invoice_key, ["inum"]);
        assert!(g.invoice_key_case_insensitive);
        assert_eq!(g.item_key, ["rt"]);
        assert_eq!(g.item_conflict, Some(ItemConflict::LastWins));
    }

    #[test]
    fn b2b_still_has_keys_awaiting_oracle_capture() {
        // Guards against the section being treated as settled while the
        // payload shape is still partly unconfirmed.
        assert_eq!(GSTR1_B2B.unverified_keys(), ["etin"]);
    }

    #[test]
    fn spec_values_compare_numerically_across_forms() {
        let hundred = SpecValue::Number(Decimal::from(100));
        assert!(hundred.matches_text("100"));
        assert!(hundred.matches_text("100.00"));
        assert!(!hundred.matches_text("65"));

        let text = SpecValue::Text("Y".into());
        assert!(text.matches_text("Y"));
        assert!(!text.matches_text("N"));
    }
}
