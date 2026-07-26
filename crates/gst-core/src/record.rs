//! Rows as they arrive from a workbook, and records as they leave validation.

use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

/// One raw row, keyed by column header exactly as the template spells it.
///
/// `sheet_row` is the 1-based row the values came from, carried so that every
/// finding can point an operator at the cell to fix.
#[derive(Debug, Clone)]
pub struct Row {
    pub sheet_row: usize,
    pub cells: HashMap<String, String>,
}

impl Row {
    pub fn new(sheet_row: usize) -> Self {
        Self {
            sheet_row,
            cells: HashMap::new(),
        }
    }

    /// Build a row from `(column, value)` pairs.
    pub fn from_pairs<I, K, V>(sheet_row: usize, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            sheet_row,
            cells: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// This row with one cell replaced — the builder tests use to derive a
    /// variant from a known-good base.
    pub fn with_cell(mut self, column: impl Into<String>, value: impl Into<String>) -> Self {
        self.cells.insert(column.into(), value.into());
        self
    }

    pub fn get(&self, column: &str) -> Option<&str> {
        self.cells.get(column).map(String::as_str)
    }

    pub fn has_column(&self, column: &str) -> bool {
        self.cells.contains_key(column)
    }
}

/// A validated, normalized cell value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Text(String),
    Number(Decimal),
    Date(NaiveDate),
}

impl Cell {
    pub fn is_empty(&self) -> bool {
        matches!(self, Cell::Empty)
    }

    /// Canonical text form, used for predicate comparison and grouping keys.
    pub fn as_text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(s) => s.clone(),
            Cell::Number(d) => d.normalize().to_string(),
            Cell::Date(d) => crate::date::normalize(*d),
        }
    }

    pub fn as_number(&self) -> Option<Decimal> {
        match self {
            Cell::Number(d) => Some(*d),
            Cell::Text(s) => s.parse().ok(),
            _ => None,
        }
    }
}

/// One row after field-level validation: field ids mapped to normalized cells.
#[derive(Debug, Clone)]
pub struct Record {
    pub sheet_row: usize,
    pub values: HashMap<String, Cell>,
}

impl Record {
    pub fn get(&self, field: &str) -> &Cell {
        self.values.get(field).unwrap_or(&Cell::Empty)
    }

    pub fn text(&self, field: &str) -> String {
        self.get(field).as_text()
    }

    pub fn number(&self, field: &str) -> Option<Decimal> {
        self.get(field).as_number()
    }
}
