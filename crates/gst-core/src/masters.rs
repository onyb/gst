//! Shared fact tables from `spec/masters/`: state codes, tax rate slabs,
//! invoice types, and credit/debit note reasons.

use std::sync::LazyLock;

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::spec::SpecValue;

/// A GST state/UT: the code is the first two digits of every GSTIN and is
/// also used for place of supply.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct State {
    pub code: String,
    pub name: String,
}

/// Allowed tax rate slabs, in percent.
#[derive(Debug, Clone, Deserialize)]
pub struct TaxRates {
    /// Rates allowed on an invoice line (IGST, or CGST+SGST combined).
    pub item: Vec<Decimal>,
    pub igst: Vec<Decimal>,
    pub cgst: Vec<Decimal>,
    pub sgst: Vec<Decimal>,
    pub cess: Vec<Decimal>,
}

/// A coded name, as used for invoice types and note reasons.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Code {
    pub code: String,
    pub name: String,
}

/// An invoice type. The import label is what an Excel or CSV workbook must
/// contain; the UI label is what the official tool displays for the same code.
/// The two differ, so matching an imported row against the display label
/// silently rejects valid workbooks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InvoiceType {
    pub code: String,
    pub import_label: String,
    pub ui_label: String,
}

/// A category in the nil-rated / exempted / non-GST table, label to code.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NilSupplyType {
    pub code: String,
    pub label: String,
}

#[derive(Deserialize)]
struct NilSupplyTypesFile {
    types: Vec<NilSupplyType>,
}

/// Resolve a workbook's supply-category label to its payload code.
pub fn nil_supply_code(label: &str) -> Option<&'static str> {
    let label = label.trim();
    NIL_SUPPLY_TYPES
        .iter()
        .find(|t| t.label == label)
        .map(|t| t.code.as_str())
}

/// B2B invoice types: table 4 (B2B/SEZ/DE) and the ECO table 15 variant.
#[derive(Debug, Clone, Deserialize)]
pub struct InvoiceTypes {
    pub table4: Vec<InvoiceType>,
    pub table15: Vec<InvoiceType>,
}

/// Resolve a workbook's invoice type label to its code, the way the import
/// path does: trim surrounding whitespace, then match the import label exactly.
pub fn invoice_type_code<'a>(types: &'a [InvoiceType], label: &str) -> Option<&'a str> {
    let label = label.trim();
    types
        .iter()
        .find(|t| t.import_label == label)
        .map(|t| t.code.as_str())
}

#[derive(Deserialize)]
struct StatesFile {
    states: Vec<State>,
}

#[derive(Deserialize)]
struct ReasonsFile {
    reasons: Vec<Code>,
}

#[derive(Deserialize)]
struct DocumentTypesFile {
    types: Vec<String>,
}

fn embedded<T: serde::de::DeserializeOwned>(name: &str, json: &str) -> T {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("embedded spec {name} is invalid: {e}"))
}

pub static STATES: LazyLock<Vec<State>> = LazyLock::new(|| {
    embedded::<StatesFile>(
        "state-codes.json",
        include_str!("../../../spec/masters/state-codes.json"),
    )
    .states
});

pub static TAX_RATES: LazyLock<TaxRates> = LazyLock::new(|| {
    embedded(
        "tax-rates.json",
        include_str!("../../../spec/masters/tax-rates.json"),
    )
});

pub static INVOICE_TYPES: LazyLock<InvoiceTypes> = LazyLock::new(|| {
    embedded(
        "invoice-types.json",
        include_str!("../../../spec/masters/invoice-types.json"),
    )
});

pub static NOTE_REASONS: LazyLock<Vec<Code>> = LazyLock::new(|| {
    embedded::<ReasonsFile>(
        "note-reasons.json",
        include_str!("../../../spec/masters/note-reasons.json"),
    )
    .reasons
});

/// An HSN or SAC code and its official description.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HsnCode {
    #[serde(rename = "c")]
    pub code: String,
    /// Defaulted because exactly one entry in GSTN's own table has no
    /// description at all (code 52083110).
    #[serde(rename = "n", default)]
    pub description: String,
}

#[derive(Deserialize)]
struct HsnFile {
    codes: Vec<HsnCode>,
}

/// HSN/SAC codes indexed by code, for the exact-match lookup the reference does.
pub static HSN_CODES: LazyLock<std::collections::HashMap<String, String>> = LazyLock::new(|| {
    embedded::<HsnFile>(
        "hsn-codes.json",
        include_str!("../../../spec/masters/hsn-codes.json"),
    )
    .codes
    .into_iter()
    .map(|c| (c.code, c.description))
    .collect()
});

/// The official description for an HSN code, by exact match.
///
/// The reference derives the description this way and blanks the code when the
/// lookup fails, so an unknown code cannot be reported at all.
pub fn hsn_description(code: &str) -> Option<&'static str> {
    HSN_CODES
        .get(code.trim())
        .map(String::as_str)
        // An empty description counts as absent: the reference blanks the code
        // when its lookup yields nothing, which then fails the required-code
        // check. One code in the shipped table has no description, so it is
        // unreportable in the reference too.
        .filter(|d| !d.is_empty())
}

/// Unit quantity labels, as 'CODE-DESCRIPTION'.
pub static UQC_LABELS: LazyLock<Vec<String>> = LazyLock::new(|| {
    embedded::<UqcFile>("uqc.json", include_str!("../../../spec/masters/uqc.json")).labels
});

#[derive(Deserialize)]
struct UqcFile {
    labels: Vec<String>,
}

/// The code a unit-quantity label carries before its hyphen: 'KGS-KILOGRAMS'
/// becomes 'KGS'. A label with no hyphen, such as 'NA', passes through whole.
pub fn uqc_code(label: &str) -> String {
    let label = label.trim();
    match label.split_once('-') {
        Some((code, _)) => code.trim().to_owned(),
        None => label.to_owned(),
    }
}

/// Categories of the nil-rated / exempted / non-GST table.
pub static NIL_SUPPLY_TYPES: LazyLock<Vec<NilSupplyType>> = LazyLock::new(|| {
    embedded::<NilSupplyTypesFile>(
        "nil-supply-types.json",
        include_str!("../../../spec/masters/nil-supply-types.json"),
    )
    .types
});

/// Natures of document, in the order that defines their payload numbering.
pub static DOCUMENT_TYPES: LazyLock<Vec<String>> = LazyLock::new(|| {
    embedded::<DocumentTypesFile>(
        "document-types.json",
        include_str!("../../../spec/masters/document-types.json"),
    )
    .types
});

/// Look up a state by its two-digit code.
pub fn state_by_code(code: &str) -> Option<&'static State> {
    STATES.iter().find(|s| s.code == code)
}

/// The masters, kept as raw JSON so specs can point into them by name.
const MASTER_FILES: &[(&str, &str)] = &[
    (
        "state-codes.json",
        include_str!("../../../spec/masters/state-codes.json"),
    ),
    (
        "tax-rates.json",
        include_str!("../../../spec/masters/tax-rates.json"),
    ),
    (
        "invoice-types.json",
        include_str!("../../../spec/masters/invoice-types.json"),
    ),
    (
        "note-reasons.json",
        include_str!("../../../spec/masters/note-reasons.json"),
    ),
    ("uqc.json", include_str!("../../../spec/masters/uqc.json")),
    (
        "document-types.json",
        include_str!("../../../spec/masters/document-types.json"),
    ),
    (
        "nil-supply-types.json",
        include_str!("../../../spec/masters/nil-supply-types.json"),
    ),
    (
        "months.json",
        include_str!("../../../spec/masters/months.json"),
    ),
    (
        "financial-years.json",
        include_str!("../../../spec/masters/financial-years.json"),
    ),
];

/// Resolve a field's `enum_ref` to the values a cell may hold.
///
/// A ref looks like `../masters/state-codes.json#/states`. Scalar arrays are
/// taken as-is. Arrays of objects yield the value a workbook actually
/// contains, preferring `import_label` (what the import path matches) over
/// `code`, then `name` — so referencing `invoice-types.json#/table4` gives the
/// import labels, not the codes or the labels the tool's own UI displays.
pub fn resolve_enum_ref(reference: &str) -> Result<Vec<SpecValue>, String> {
    let (path, pointer) = reference
        .split_once('#')
        .ok_or_else(|| format!("enum_ref `{reference}` has no JSON pointer"))?;
    let file = path
        .rsplit('/')
        .next()
        .ok_or_else(|| format!("enum_ref `{reference}` has no filename"))?;
    let raw = MASTER_FILES
        .iter()
        .find(|(name, _)| *name == file)
        .map(|(_, json)| *json)
        .ok_or_else(|| format!("enum_ref `{reference}` names an unknown master `{file}`"))?;

    let doc: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("master {file} is invalid: {e}"))?;
    let target = doc
        .pointer(pointer)
        .ok_or_else(|| format!("enum_ref `{reference}` does not resolve in {file}"))?;
    let items = target
        .as_array()
        .ok_or_else(|| format!("enum_ref `{reference}` does not point at an array"))?;

    items
        .iter()
        .map(|item| match item {
            serde_json::Value::Object(map) => ["import_label", "code", "name"]
                .iter()
                .find_map(|k| map.get(*k))
                .and_then(|v| v.as_str())
                .map(|s| SpecValue::Text(s.to_owned()))
                .ok_or_else(|| {
                    format!("enum_ref `{reference}`: object has no import_label, code or name")
                }),
            other => serde_json::from_value(other.clone())
                .map_err(|e| format!("enum_ref `{reference}`: {e}")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_cover_all_codes() {
        assert_eq!(STATES.len(), 38);
        assert_eq!(state_by_code("27").unwrap().name, "Maharashtra");
        assert_eq!(state_by_code("97").unwrap().name, "Other Territory");
        assert!(state_by_code("00").is_none());
    }

    #[test]
    fn item_rates_include_special_slabs() {
        let rates = &TAX_RATES.item;
        for r in ["0.1", "0.25", "1.5", "7.5", "40"] {
            let r: Decimal = r.parse().unwrap();
            assert!(rates.contains(&r), "missing item rate {r}");
        }
    }

    #[test]
    fn invoice_types_table15_excludes_cbw() {
        assert!(INVOICE_TYPES.table4.iter().any(|t| t.code == "CBW"));
        assert!(INVOICE_TYPES.table15.iter().all(|t| t.code != "CBW"));
    }

    #[test]
    fn invoice_type_resolves_import_label_not_ui_label() {
        let t4 = &INVOICE_TYPES.table4;
        assert_eq!(invoice_type_code(t4, "Deemed Exp"), Some("DE"));
        assert_eq!(invoice_type_code(t4, "  Deemed Exp  "), Some("DE"));
        assert_eq!(
            invoice_type_code(t4, "SEZ supplies with payment"),
            Some("SEWP")
        );

        // The UI spells these differently; a workbook using the displayed
        // label is not accepted by the import path.
        assert_eq!(invoice_type_code(t4, "Deemed Exports"), None);
        assert_eq!(invoice_type_code(t4, "SEZ Supplies with payment"), None);

        // 'Regular B2B' is table 4; the ECO variant says plain 'Regular'.
        assert_eq!(invoice_type_code(t4, "Regular B2B"), Some("R"));
        assert_eq!(invoice_type_code(t4, "Regular"), None);
        assert_eq!(
            invoice_type_code(&INVOICE_TYPES.table15, "Regular"),
            Some("R")
        );
    }

    #[test]
    fn note_reasons_present() {
        assert_eq!(NOTE_REASONS.len(), 7);
    }
}
