//! Shared fact tables from `spec/masters/`: state codes, tax rate slabs,
//! invoice types, and credit/debit note reasons.

use std::sync::LazyLock;

use rust_decimal::Decimal;
use serde::Deserialize;

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

/// Look up a state by its two-digit code.
pub fn state_by_code(code: &str) -> Option<&'static State> {
    STATES.iter().find(|s| s.code == code)
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
