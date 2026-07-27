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

/// A named field-level check, optionally parameterized and optionally scoped to
/// a range of return periods.
///
/// Period scoping is what keeps rules like the B2C(Large) value threshold —
/// which changed with the August 2024 period — expressed in the spec, numbers
/// and cutover date included, rather than buried in the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub name: String,
    pub value: Option<Decimal>,
    /// Applies when the return period is this period or later.
    pub from_period: Option<String>,
    /// Applies when the return period is strictly earlier than this period.
    pub until_period: Option<String>,
}

impl Constraint {
    /// Whether this constraint applies to a return period, as YYYYMM.
    pub fn applies_to(&self, period_yyyymm: u32) -> bool {
        let bound = |p: &Option<String>| p.as_deref().and_then(period_as_yyyymm);
        if let Some(from) = bound(&self.from_period)
            && period_yyyymm < from
        {
            return false;
        }
        if let Some(until) = bound(&self.until_period)
            && period_yyyymm >= until
        {
            return false;
        }
        true
    }
}

/// `MMYYYY` (as spec files and the portal write it) to comparable `YYYYMM`.
pub fn period_as_yyyymm(text: &str) -> Option<u32> {
    crate::date::ReturnPeriod::parse(text).map(|p| p.as_yyyymm())
}

/// Wire form: a bare string names a parameterless check; the object form
/// carries the parameter and period scope.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawConstraint {
    Name(String),
    Detailed {
        name: String,
        value: Option<Decimal>,
        from_period: Option<String>,
        until_period: Option<String>,
        #[allow(dead_code)]
        description: Option<String>,
    },
}

impl<'de> Deserialize<'de> for Constraint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match RawConstraint::deserialize(d)? {
            RawConstraint::Name(name) => Constraint {
                name,
                value: None,
                from_period: None,
                until_period: None,
            },
            RawConstraint::Detailed {
                name,
                value,
                from_period,
                until_period,
                ..
            } => Constraint {
                name,
                value,
                from_period,
                until_period,
            },
        })
    }
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
    /// Decimal places this amount is rounded to before its pattern is
    /// checked, mirroring the reference's numeric conversion.
    pub round_to: Option<u32>,
    /// Narrows `required` to a range of return periods, for columns a section
    /// began or stopped insisting on at a cutover. Absent means `required`
    /// applies to every period.
    pub required_from_period: Option<String>,
    pub required_until_period: Option<String>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    pub transform: Option<String>,
    #[serde(default)]
    pub must_be_empty: bool,
    pub description: Option<String>,
}

/// Whether a period-scoped thing applies to a return period, as YYYYMM.
/// `from` is inclusive, `until` exclusive — the same convention [`Constraint`]
/// uses, so a cutover period is named once and means the same everywhere.
fn within(from: &Option<String>, until: &Option<String>, period_yyyymm: u32) -> bool {
    let bound = |p: &Option<String>| p.as_deref().and_then(period_as_yyyymm);
    if let Some(from) = bound(from)
        && period_yyyymm < from
    {
        return false;
    }
    if let Some(until) = bound(until)
        && period_yyyymm >= until
    {
        return false;
    }
    true
}

impl Field {
    /// Whether this column must carry a value for the period being filed.
    pub fn is_required(&self, period_yyyymm: u32) -> bool {
        self.required
            && within(
                &self.required_from_period,
                &self.required_until_period,
                period_yyyymm,
            )
    }
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
    /// Field-to-field numeric comparison: this field must be >= another.
    GteField {
        field: String,
        other: String,
    },
    /// Field-to-field numeric equality — central tax must equal state tax.
    EqField {
        field: String,
        other: String,
    },
    /// The field's text matches a regular expression. Used where a value's
    /// allowed shape depends on another cell, which a field-level `pattern`
    /// cannot express.
    Matches {
        field: String,
        pattern: String,
    },
    /// Field-to-field sign agreement: both positive, both negative, or either
    /// one zero. The reference uses this to keep a cess from contradicting the
    /// amount it is charged on.
    SignAgreesWith {
        field: String,
        other: String,
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
    gte_field: Option<String>,
    eq_field: Option<String>,
    matches: Option<String>,
    sign_agrees_with: Option<String>,
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
        match (
            raw.eq,
            raw.ne,
            raw.in_,
            raw.empty,
            raw.gte_field,
            raw.eq_field,
            raw.matches,
            raw.sign_agrees_with,
        ) {
            (Some(value), None, None, None, None, None, None, None) => {
                Ok(Predicate::Eq { field, value })
            }
            (None, Some(value), None, None, None, None, None, None) => {
                Ok(Predicate::Ne { field, value })
            }
            (None, None, Some(values), None, None, None, None, None) => {
                Ok(Predicate::In { field, values })
            }
            (None, None, None, Some(empty), None, None, None, None) => {
                Ok(Predicate::Empty { field, empty })
            }
            (None, None, None, None, Some(other), None, None, None) => {
                Ok(Predicate::GteField { field, other })
            }
            (None, None, None, None, None, Some(other), None, None) => {
                Ok(Predicate::EqField { field, other })
            }
            (None, None, None, None, None, None, Some(pattern), None) => {
                Ok(Predicate::Matches { field, pattern })
            }
            (None, None, None, None, None, None, None, Some(other)) => {
                Ok(Predicate::SignAgreesWith { field, other })
            }
            _ => Err(format!(
                "predicate on `{field}` must have exactly one of eq/ne/in/empty/gte_field/eq_field/matches/sign_agrees_with"
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
    /// Both rows are kept. For levels the reference keys on a freshly minted
    /// serial rather than on any cell, so a collision is impossible there.
    Append,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceFieldConflict {
    Error,
    FirstWins,
}

/// What the reference does with a second flat record carrying the same key.
///
/// Both variants discard one of the two rows outright — no summing — so the
/// engine reproduces the choice and warns, rather than quietly losing money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordConflict {
    /// The later row replaces the earlier one, in the earlier one's position.
    LastWins,
    /// The later row is dropped.
    FirstWins,
}

/// One component of a collapse key, optionally scoped to a range of periods —
/// the pre-bifurcation HSN summary added the rate to its key at 05-2021.
#[derive(Debug, Clone)]
pub struct KeyPart {
    pub field: String,
    pub from_period: Option<String>,
    pub until_period: Option<String>,
}

impl KeyPart {
    pub fn applies_to(&self, period_yyyymm: u32) -> bool {
        within(&self.from_period, &self.until_period, period_yyyymm)
    }
}

/// Wire form: a bare string names an always-applicable component.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawKeyPart {
    Field(String),
    Detailed {
        field: String,
        from_period: Option<String>,
        until_period: Option<String>,
        #[allow(dead_code)]
        description: Option<String>,
    },
}

impl<'de> Deserialize<'de> for KeyPart {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match RawKeyPart::deserialize(d)? {
            RawKeyPart::Field(field) => KeyPart {
                field,
                from_period: None,
                until_period: None,
            },
            RawKeyPart::Detailed {
                field,
                from_period,
                until_period,
                ..
            } => KeyPart {
                field,
                from_period,
                until_period,
            },
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Grouping {
    #[serde(default)]
    pub envelope_key: Vec<String>,
    /// Empty for flat sections, which have no invoice level.
    #[serde(default)]
    pub invoice_key: Vec<String>,
    #[serde(default)]
    pub invoice_key_case_insensitive: bool,
    #[serde(default)]
    pub item_key: Vec<String>,
    /// Fields that must agree across grouped rows even though the payload does
    /// not carry them.
    #[serde(default)]
    pub agree_fields: Vec<String>,
    pub item_conflict: Option<ItemConflict>,
    pub invoice_field_conflict: Option<InvoiceFieldConflict>,
    /// Flat sections only: the key rows are collapsed on before they become
    /// payload records. Empty means every row is its own record.
    #[serde(default)]
    pub record_key: Vec<KeyPart>,
    pub record_conflict: Option<RecordConflict>,
    /// Whether an invoice number is matched across the whole section rather
    /// than within its envelope. Sections with no counterparty id — B2C large,
    /// exports, notes to unregistered persons — do exactly that in the
    /// reference, because the absent id normalises to the empty string on both
    /// sides and drops out of the comparison.
    #[serde(default)]
    pub invoice_key_global: bool,
}

/// Where a payload key's content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Field(String),
    Derive(String),
    Nested(Level),
    /// A constant the reference writes with no column behind it — the
    /// e-commerce sections stamp every record with `flag: "N"`.
    Literal(String),
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
            Some(("literal", value)) => Ok(Source::Literal(value.to_owned())),
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
    /// Dropped when the value would be exactly this — for keys the reference
    /// omits at a default rather than at empty.
    pub omit_when_value: Option<SpecValue>,
    /// Emitted ONLY when the value is exactly this, and dropped otherwise.
    pub only_when_value: Option<SpecValue>,
    /// Emitted only for return periods in this range, for sections whose
    /// payload shape changed at a cutover.
    pub from_period: Option<String>,
    pub until_period: Option<String>,
    pub verify: Option<Verify>,
    pub description: Option<String>,
}

impl PayloadKey {
    pub fn applies_to(&self, period_yyyymm: u32) -> bool {
        within(&self.from_period, &self.until_period, period_yyyymm)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PayloadObject {
    pub keys: Vec<PayloadKey>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Output {
    /// Flat sections: one payload object per validated row. When present, the
    /// nested levels below are absent.
    pub record: Option<PayloadObject>,
    pub envelope: Option<PayloadObject>,
    pub invoice: Option<PayloadObject>,
    pub item: Option<PayloadObject>,
    #[serde(default)]
    pub derivations: Vec<String>,
    #[serde(default)]
    pub derivation_notes: std::collections::HashMap<String, String>,
    /// Sections whose rows are split across several members of one payload
    /// object — the e-commerce summary sends each row to `clttx` or `paytx`
    /// depending on its nature of supply. The tag never appears in the payload.
    pub member_from: Option<MemberFrom>,
}

/// How a row picks which member of its payload object it belongs to.
#[derive(Debug, Clone, Deserialize)]
pub struct MemberFrom {
    /// Field whose value selects the member.
    pub field: String,
    /// Field value to member name.
    pub map: std::collections::HashMap<String, String>,
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
    /// Return periods this section is filed for at all. The HSN summary is the
    /// only one so far: a single sheet up to the 05-2025 bifurcation, a B2B/B2C
    /// pair from it. Absent means every period.
    pub active_from_period: Option<String>,
    pub active_until_period: Option<String>,
    #[serde(default)]
    pub notes: Vec<Note>,
}

impl SectionSpec {
    pub fn field(&self, id: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// Whether this section is filed for the given period at all. A sheet
    /// outside its window is not read: its records would have nowhere to go in
    /// the upload file, and dropping them silently is how the pre-bifurcation
    /// HSN summary went missing for every period before 05-2025.
    pub fn active_for(&self, period_yyyymm: u32) -> bool {
        within(
            &self.active_from_period,
            &self.active_until_period,
            period_yyyymm,
        )
    }

    /// Column headers in template order — what an importer matches against.
    pub fn columns(&self) -> Vec<&str> {
        let mut fields: Vec<&Field> = self.fields.iter().collect();
        fields.sort_by_key(|f| f.order);
        fields.iter().map(|f| f.column.as_str()).collect()
    }

    /// Whether this section emits one flat object per row rather than a
    /// nested envelope/invoice/item structure.
    pub fn is_flat(&self) -> bool {
        self.output.record.is_some()
    }

    /// Payload keys still awaiting differential capture. A section with any of
    /// these is not yet trustworthy for real filing.
    pub fn unverified_keys(&self) -> Vec<&str> {
        let objects = [
            self.output.record.as_ref(),
            self.output.envelope.as_ref(),
            self.output.invoice.as_ref(),
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

/// One list declares the registry: each entry emits its static, and the same
/// entries populate `sections()` in declaration order, so a section cannot be
/// embedded yet missing from the lookup.
macro_rules! sections {
    ($($name:ident => $file:literal),+ $(,)?) => {
        $(
            pub static $name: LazyLock<SectionSpec> = LazyLock::new(|| {
                crate::masters::embedded($file, include_str!(concat!("../../../spec/", $file)))
            });
        )+

        /// Every section the engine knows, in the order a return reports them.
        pub fn sections() -> &'static [&'static SectionSpec] {
            static ALL: LazyLock<Vec<&'static SectionSpec>> =
                LazyLock::new(|| vec![$(&$name),+]);
            &ALL
        }
    };
}

sections! {
    GSTR1_B2B => "gstr1/b2b.json",
    GSTR1_B2BA => "gstr1/b2ba.json",
    GSTR1_B2CL => "gstr1/b2cl.json",
    GSTR1_B2CLA => "gstr1/b2cla.json",
    GSTR1_B2CS => "gstr1/b2cs.json",
    GSTR1_B2CSA => "gstr1/b2csa.json",
    GSTR1_CDNR => "gstr1/cdnr.json",
    GSTR1_CDNRA => "gstr1/cdnra.json",
    GSTR1_CDNUR => "gstr1/cdnur.json",
    GSTR1_CDNURA => "gstr1/cdnura.json",
    GSTR1_EXP => "gstr1/exp.json",
    GSTR1_EXPA => "gstr1/expa.json",
    GSTR1_AT => "gstr1/at.json",
    GSTR1_ATA => "gstr1/ata.json",
    GSTR1_ATADJ => "gstr1/atadj.json",
    GSTR1_ATADJA => "gstr1/atadja.json",
    GSTR1_NIL => "gstr1/exemp.json",
    GSTR1_SUPECO => "gstr1/eco.json",
    GSTR1_SUPECOA => "gstr1/ecoa.json",
    GSTR1_ECOM_B2B => "gstr1/ecob2b.json",
    GSTR1_ECOM_B2C => "gstr1/ecob2c.json",
    GSTR1_ECOM_URP2B => "gstr1/ecourp2b.json",
    GSTR1_ECOM_URP2C => "gstr1/ecourp2c.json",
    GSTR1_ECOMA_B2B => "gstr1/ecoab2b.json",
    GSTR1_ECOMA_B2C => "gstr1/ecoab2c.json",
    GSTR1_ECOMA_URP2B => "gstr1/ecoaurp2b.json",
    GSTR1_ECOMA_URP2C => "gstr1/ecoaurp2c.json",
    GSTR1_DOC_ISSUE => "gstr1/docs.json",
    GSTR1_HSN => "gstr1/hsn.json",
    GSTR1_HSN_B2B => "gstr1/hsn-b2b.json",
    GSTR1_HSN_B2C => "gstr1/hsn-b2c.json",
}

/// Look up a section by its code, e.g. `b2b`.
pub fn section(code: &str) -> Option<&'static SectionSpec> {
    sections().iter().find(|s| s.section == code).copied()
}

/// Section codes, for listing what is available in a usage message.
pub fn section_codes() -> Vec<&'static str> {
    sections().iter().map(|s| s.section.as_str()).collect()
}

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
    fn b2ba_spec_loads_with_template_column_order() {
        let spec = section("b2ba").expect("b2ba is registered");
        assert_eq!(spec.return_type, "gstr1");
        assert_eq!(
            spec.columns(),
            [
                "GSTIN/UIN of Recipient",
                "Receiver Name",
                "Original Invoice Number",
                "Original Invoice date",
                "Revised Invoice Number",
                "Revised Invoice date",
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
        // The amendment keys the original invoice it corrects.
        assert!(spec.field("oinum").is_some());
        assert!(spec.field("oidt").is_some());
    }

    #[test]
    fn every_registered_section_is_internally_consistent() {
        for spec in sections() {
            // Column orders must be a gapless 0..n, or an importer matching by
            // position would silently read the wrong cells.
            let mut orders: Vec<usize> = spec.fields.iter().map(|f| f.order).collect();
            orders.sort_unstable();
            assert_eq!(
                orders,
                (0..spec.fields.len()).collect::<Vec<_>>(),
                "{} has gapped or duplicated column orders",
                spec.section
            );

            // Field ids unique.
            let mut ids: Vec<&str> = spec.fields.iter().map(|f| f.id.as_str()).collect();
            ids.sort_unstable();
            let count = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), count, "{} has duplicate field ids", spec.section);

            // Rules and grouping may only name fields that exist.
            for key in spec
                .grouping
                .envelope_key
                .iter()
                .chain(&spec.grouping.invoice_key)
                .chain(&spec.grouping.item_key)
            {
                assert!(
                    spec.field(key).is_some(),
                    "{} groups on unknown field '{key}'",
                    spec.section
                );
            }

            // Rule ids are namespaced by their own section.
            for rule in &spec.rules {
                assert!(
                    rule.id.starts_with(&format!("{}.", spec.section)),
                    "rule '{}' is not namespaced to section '{}'",
                    rule.id,
                    spec.section
                );
            }
        }
    }

    #[test]
    fn period_scoped_constraints_apply_only_within_their_range() {
        let until = Constraint {
            name: "min_exclusive".into(),
            value: Some(Decimal::from(250000)),
            from_period: None,
            until_period: Some("082024".into()),
        };
        let from = Constraint {
            name: "min_exclusive".into(),
            value: Some(Decimal::from(100000)),
            from_period: Some("082024".into()),
            until_period: None,
        };
        // July 2024 is before the cutover; August 2024 is on it.
        assert!(until.applies_to(202407));
        assert!(!from.applies_to(202407));
        assert!(!until.applies_to(202408));
        assert!(from.applies_to(202408));
        assert!(from.applies_to(202512));

        // Unscoped constraints always apply.
        let always = Constraint {
            name: "pos_code_range".into(),
            value: None,
            from_period: None,
            until_period: None,
        };
        assert!(always.applies_to(201707));
        assert!(always.applies_to(203001));
    }

    #[test]
    fn period_strings_order_as_yyyymm() {
        assert_eq!(period_as_yyyymm("082024"), Some(202408));
        assert_eq!(period_as_yyyymm("122017"), Some(201712));
        assert!(period_as_yyyymm("072017").unwrap() < period_as_yyyymm("082024").unwrap());
        assert_eq!(period_as_yyyymm("132024"), None);
        assert_eq!(period_as_yyyymm("82024"), None);
        assert_eq!(period_as_yyyymm("abcdef"), None);
    }

    #[test]
    fn section_lookup_rejects_unknown_codes() {
        assert!(section("b2b").is_some());
        assert!(section("b2ba").is_some());
        assert!(section("cdnr").is_some());
        assert!(section("exp").is_some());
        assert!(section("at").is_some());
        assert!(section("supeco").is_some());
        assert!(section("nonsense").is_none());
        assert_eq!(
            section_codes(),
            [
                "b2b",
                "b2ba",
                "b2cl",
                "b2cla",
                "b2cs",
                "b2csa",
                "cdnr",
                "cdnra",
                "cdnur",
                "cdnura",
                "exp",
                "expa",
                "at",
                "ata",
                "atadj",
                "atadja",
                "nil",
                "supeco",
                "supecoa",
                "ecomb2b",
                "ecomb2c",
                "ecomurp2b",
                "ecomurp2c",
                "ecomab2b",
                "ecomab2c",
                "ecomaurp2b",
                "ecomaurp2c",
                "doc_issue",
                "hsn",
                "hsn(b2b)",
                "hsn(b2c)"
            ]
        );
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
