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

/// B2B invoice types: table 4 (B2B/SEZ/DE) and the ECO table 15 variant.
#[derive(Debug, Clone, Deserialize)]
pub struct InvoiceTypes {
    pub table4: Vec<Code>,
    pub table15: Vec<Code>,
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
    fn note_reasons_present() {
        assert_eq!(NOTE_REASONS.len(), 7);
    }
}
