//! Spec-driven validation.
//!
//! Two passes per row. First every field is checked against its own
//! declaration — presence, pattern, enum, type, named constraints — and
//! normalized into a [`Record`]. Then the section's cross-field rules run as
//! predicates over that record. Rules only see rows whose fields all passed,
//! so a rule never has to defend against a malformed value.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use rust_decimal::Decimal;

use crate::date::{self, DateError, ReturnPeriod};
use crate::gstin;
use crate::masters;
use crate::record::{Cell, Record, Row};
use crate::spec::{Field, FieldType, Predicate, SectionSpec, Severity, SpecValue};

/// Everything about the filer that validation and generation depend on.
///
/// The intra/inter-state split turns on the supplier's own state, so the same
/// workbook row legitimately produces different output for different filers.
#[derive(Debug, Clone)]
pub struct FilingContext {
    pub supplier_gstin: String,
    pub period: ReturnPeriod,
    /// Whether the filer is an SEZ unit. Forces every line inter-state.
    pub is_sez: bool,
}

impl FilingContext {
    pub fn supplier_state(&self) -> Option<&str> {
        gstin::state_code(&self.supplier_gstin)
    }
}

/// One problem found in one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// 1-based sheet row, so an operator can go straight to the cell.
    pub sheet_row: usize,
    /// Column header, where the problem belongs to a single column.
    pub column: Option<String>,
    pub field: Option<String>,
    /// Rule id for cross-field findings; `None` for field-level ones.
    pub rule: Option<String>,
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    fn field_level(row: usize, field: &Field, message: impl Into<String>) -> Self {
        Self {
            sheet_row: row,
            column: Some(field.column.clone()),
            field: Some(field.id.clone()),
            rule: None,
            severity: Severity::Error,
            message: message.into(),
        }
    }
}

/// Outcome of validating a set of rows.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Rows that passed every check, ready for grouping and generation.
    pub records: Vec<Record>,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        !self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
    }
}

/// Compiled patterns, keyed by the spec's regex source. Sections reuse the
/// same handful of patterns, so compiling once per distinct source is enough.
static PATTERNS: LazyLock<std::sync::Mutex<HashMap<String, Regex>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn pattern_matches(source: &str, text: &str) -> Result<bool, String> {
    let mut cache = PATTERNS.lock().expect("pattern cache is not poisoned");
    let re = match cache.get(source) {
        Some(re) => re,
        None => {
            let re = Regex::new(source).map_err(|e| format!("invalid pattern in spec: {e}"))?;
            cache.entry(source.to_owned()).or_insert(re)
        }
    };
    Ok(re.is_match(text))
}

/// Validate rows against a section spec.
pub fn validate(spec: &SectionSpec, rows: &[Row], ctx: &FilingContext) -> Report {
    let mut report = Report::default();

    for row in rows {
        match validate_row(spec, row, ctx) {
            Ok(record) => {
                let rule_findings = apply_rules(spec, &record);
                if rule_findings.is_empty() {
                    report.records.push(record);
                } else {
                    // A row failing a cross-field rule is not generated, but
                    // warnings alone must not drop it.
                    let fatal = rule_findings.iter().any(|f| f.severity == Severity::Error);
                    if !fatal {
                        report.records.push(record);
                    }
                    report.findings.extend(rule_findings);
                }
            }
            Err(findings) => report.findings.extend(findings),
        }
    }

    report
}

/// Field-level pass. Returns the normalized record, or every field problem
/// found — reporting all of them at once beats making an operator re-run per
/// cell.
fn validate_row(
    spec: &SectionSpec,
    row: &Row,
    ctx: &FilingContext,
) -> Result<Record, Vec<Finding>> {
    let mut values = HashMap::new();
    let mut findings = Vec::new();

    for field in &spec.fields {
        if !row.has_column(&field.column) {
            findings.push(Finding::field_level(
                row.sheet_row,
                field,
                format!("column '{}' is missing from the sheet", field.column),
            ));
            continue;
        }
        match validate_field(field, row.get(&field.column).unwrap_or(""), ctx) {
            Ok(cell) => {
                values.insert(field.id.clone(), cell);
            }
            Err(message) => findings.push(Finding::field_level(row.sheet_row, field, message)),
        }
    }

    if findings.is_empty() {
        Ok(Record {
            sheet_row: row.sheet_row,
            values,
        })
    } else {
        Err(findings)
    }
}

fn validate_field(field: &Field, raw: &str, ctx: &FilingContext) -> Result<Cell, String> {
    let mut text = raw.trim().to_owned();

    // A default stands in for an empty cell before anything else looks at it.
    if text.is_empty()
        && let Some(default) = &field.default
    {
        text = default.as_text();
    }

    if text.is_empty() {
        return if field.required && !field.must_be_empty {
            Err(format!("'{}' is required", field.column))
        } else {
            Ok(Cell::Empty)
        };
    }

    if field.must_be_empty {
        return Err(format!(
            "'{}' must be blank in this section, found '{text}'",
            field.column
        ));
    }

    // `state_code` cells arrive as 'NN-Name'; every later check wants the code.
    let checked = if field.ty == FieldType::StateCode {
        state_code_prefix(&text)
    } else {
        text.clone()
    };

    if let Some(source) = &field.pattern
        && !pattern_matches(source, &checked)?
    {
        return Err(format!("'{}' has an invalid value '{text}'", field.column));
    }

    if let Some(max) = field.max_length
        && checked.chars().count() > max
    {
        return Err(format!(
            "'{}' is longer than {max} characters",
            field.column
        ));
    }

    if let Some(allowed) = allowed_values(field)?
        && !allowed.iter().any(|v| v.matches_text(&checked))
    {
        let shown: Vec<String> = allowed.iter().map(SpecValue::as_text).collect();
        return Err(format!(
            "'{}' must be one of {} — found '{text}'",
            field.column,
            shown.join(", ")
        ));
    }

    for constraint in &field.constraints {
        apply_constraint(constraint, field, &checked, ctx)?;
    }

    if field.ty == FieldType::Gstin && !gstin::matches_any_form(&checked, &field.accepts) {
        return Err(format!(
            "'{}' is not a recognized registration number: '{text}'",
            field.column
        ));
    }

    to_cell(field, &checked, ctx)
}

/// Named field-level checks the spec refers to by name.
fn apply_constraint(
    name: &str,
    field: &Field,
    text: &str,
    ctx: &FilingContext,
) -> Result<(), String> {
    match name {
        // Document numbers that are numerically zero are rejected even though
        // they satisfy the character pattern. Non-numeric numbers are fine.
        "numeric_value_not_zero" => match text.parse::<Decimal>() {
            Ok(n) if n.is_zero() => Err(format!(
                "'{}' cannot be zero — found '{text}'",
                field.column
            )),
            _ => Ok(()),
        },
        // Codes 01-38 are states and UTs; 96 is Other Country, 97 is Other
        // Territory. The template's dropdown omits 96, but it is accepted.
        "pos_code_range" => {
            let code: u32 = text
                .parse()
                .map_err(|_| format!("'{}' is not a state code: '{text}'", field.column))?;
            if (1..=38).contains(&code) || code == 96 || code == 97 {
                Ok(())
            } else {
                Err(format!(
                    "'{}' is not a valid place of supply code: '{text}'",
                    field.column
                ))
            }
        }
        "gstin_checksum" => {
            if gstin::checksum_valid(text) {
                Ok(())
            } else {
                Err(format!(
                    "'{}' has an incorrect check digit: '{text}'",
                    field.column
                ))
            }
        }
        "date_within_return_window" => {
            let parsed = parse_date(text)?;
            date::check_window(parsed, ctx.period).map_err(|e| describe_date_error(field, text, e))
        }
        other => Err(format!(
            "spec names constraint '{other}', which the engine does not implement"
        )),
    }
}

fn parse_date(text: &str) -> Result<chrono::NaiveDate, String> {
    // Excel hands dates over as serials when the cell is date-formatted.
    if let Ok(serial) = text.parse::<i64>() {
        return date::from_excel_serial(serial).map_err(|_| format!("'{text}' is not a date"));
    }
    date::parse_text(text).map_err(|e| match e {
        DateError::NotACalendarDate => format!("'{text}' is not a real date"),
        _ => format!("'{text}' is not a valid date — expected a form like 14-Jul-2017"),
    })
}

fn describe_date_error(field: &Field, text: &str, e: DateError) -> String {
    match e {
        DateError::BeforeGst => format!(
            "'{}' is before 1 July 2017, when GST came into force: '{text}'",
            field.column
        ),
        DateError::AfterReturnPeriod => format!(
            "'{}' is after the end of the return period: '{text}'",
            field.column
        ),
        DateError::NotACalendarDate => format!("'{}' is not a real date: '{text}'", field.column),
        DateError::Malformed => format!("'{}' is not a valid date: '{text}'", field.column),
    }
}

/// Apply the field's declared transform and settle its final type.
fn to_cell(field: &Field, checked: &str, _ctx: &FilingContext) -> Result<Cell, String> {
    match field.transform.as_deref() {
        // Percent to factor: 100 becomes 1.00, 65 becomes 0.65. Every computed
        // tax amount is scaled by it.
        Some("percent_to_factor") => {
            let pct: Decimal = checked
                .parse()
                .map_err(|_| format!("'{}' is not a number: '{checked}'", field.column))?;
            Ok(Cell::Number((pct / Decimal::from(100)).round_dp(2)))
        }
        // The enum check above already confirmed the label; map it to its code.
        Some("invoice_type_code") => {
            let types = &masters::INVOICE_TYPES.table4;
            masters::invoice_type_code(types, checked)
                .map(|code| Cell::Text(code.to_owned()))
                .ok_or_else(|| {
                    format!(
                        "'{}' is not a known invoice type: '{checked}'",
                        field.column
                    )
                })
        }
        Some("date_normalize") => Ok(Cell::Date(parse_date(checked)?)),
        // Already applied before validation, since later checks need the code.
        Some("state_code_prefix") => Ok(Cell::Text(checked.to_owned())),
        Some(other) => Err(format!(
            "spec names transform '{other}', which the engine does not implement"
        )),
        None => match field.ty {
            FieldType::Decimal => checked
                .parse()
                .map(Cell::Number)
                .map_err(|_| format!("'{}' is not a number: '{checked}'", field.column)),
            FieldType::Date => Ok(Cell::Date(parse_date(checked)?)),
            _ => Ok(Cell::Text(checked.to_owned())),
        },
    }
}

/// 'NN-Name' reduces to 'NN'. A bare code passes through unchanged.
fn state_code_prefix(text: &str) -> String {
    text.split('-')
        .next()
        .unwrap_or(text)
        .trim()
        .chars()
        .take(2)
        .collect()
}

fn allowed_values(field: &Field) -> Result<Option<Vec<SpecValue>>, String> {
    if let Some(inline) = &field.allowed {
        return Ok(Some(inline.clone()));
    }
    match &field.enum_ref {
        Some(reference) => masters::resolve_enum_ref(reference).map(Some),
        None => Ok(None),
    }
}

/// Cross-field pass.
fn apply_rules(spec: &SectionSpec, record: &Record) -> Vec<Finding> {
    spec.rules
        .iter()
        .filter(|rule| rule.when.as_ref().is_none_or(|w| evaluate(w, record)))
        .filter(|rule| !evaluate(&rule.assert, record))
        .map(|rule| Finding {
            sheet_row: record.sheet_row,
            column: None,
            field: None,
            rule: Some(rule.id.clone()),
            severity: rule.severity,
            message: rule.message.clone(),
        })
        .collect()
}

/// Walk a predicate against a record.
pub fn evaluate(predicate: &Predicate, record: &Record) -> bool {
    match predicate {
        Predicate::Eq { field, value } => value.matches_text(&record.text(field)),
        Predicate::Ne { field, value } => !value.matches_text(&record.text(field)),
        Predicate::In { field, values } => {
            let text = record.text(field);
            values.iter().any(|v| v.matches_text(&text))
        }
        Predicate::Empty { field, empty } => record.get(field).is_empty() == *empty,
        Predicate::All(preds) => preds.iter().all(|p| evaluate(p, record)),
        Predicate::Any(preds) => preds.iter().any(|p| evaluate(p, record)),
        Predicate::Not(pred) => !evaluate(pred, record),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::GSTR1_B2B;

    fn ctx() -> FilingContext {
        FilingContext {
            // A Maharashtra (27) supplier, so 27 is intra-state.
            supplier_gstin: "27AAPFU0939F1ZV".into(),
            period: ReturnPeriod::new(7, 2017).unwrap(),
            is_sez: false,
        }
    }

    /// A row that passes everything, as a base for targeted mutation.
    fn good_row() -> Row {
        Row::from_pairs(
            5,
            [
                ("GSTIN/UIN of Recipient", "12GEOPS0823BBZH"),
                ("Receiver Name", "Acme Traders"),
                ("Invoice Number", "INV-001"),
                ("Invoice date", "14-Jul-17"),
                ("Invoice Value", "50000"),
                ("Place Of Supply", "37-Andhra Pradesh"),
                ("Reverse Charge", "N"),
                ("Applicable % of Tax Rate", ""),
                ("Invoice Type", "Regular B2B"),
                ("E-Commerce GSTIN", ""),
                ("Rate", "18"),
                ("Taxable Value", "45000"),
                ("Cess Amount", ""),
            ],
        )
    }

    fn with(column: &str, value: &str) -> Row {
        let mut row = good_row();
        row.cells.insert(column.to_owned(), value.to_owned());
        row
    }

    fn only_finding(row: Row) -> Finding {
        let report = validate(&GSTR1_B2B, &[row], &ctx());
        assert_eq!(
            report.findings.len(),
            1,
            "expected exactly one finding, got {:?}",
            report.findings
        );
        report.findings.into_iter().next().unwrap()
    }

    #[test]
    fn a_good_row_validates_and_normalizes() {
        let report = validate(&GSTR1_B2B, &[good_row()], &ctx());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.records.len(), 1);

        let r = &report.records[0];
        assert_eq!(r.text("ctin"), "12GEOPS0823BBZH");
        assert_eq!(r.text("inum"), "INV-001");
        // Place of supply keeps only the numeric prefix.
        assert_eq!(r.text("pos"), "37");
        // The label becomes its code.
        assert_eq!(r.text("inv_typ"), "R");
        // Blank percent defaults to 100, then becomes a factor.
        assert_eq!(r.number("diff_percent"), Some(Decimal::new(1, 0)));
        // Blank cess defaults to zero.
        assert_eq!(r.number("csamt"), Some(Decimal::ZERO));
        // Dates normalize to the payload form.
        assert_eq!(r.text("idt"), "14-07-2017");
    }

    #[test]
    fn a_missing_required_field_is_reported_against_its_column() {
        let f = only_finding(with("Invoice Number", ""));
        assert_eq!(f.sheet_row, 5);
        assert_eq!(f.column.as_deref(), Some("Invoice Number"));
        assert_eq!(f.field.as_deref(), Some("inum"));
        assert!(f.message.contains("required"), "{}", f.message);
    }

    #[test]
    fn a_missing_column_is_distinguished_from_an_empty_cell() {
        let mut row = good_row();
        row.cells.remove("Invoice Number");
        let f = only_finding(row);
        assert!(
            f.message.contains("missing from the sheet"),
            "{}",
            f.message
        );
    }

    #[test]
    fn every_bad_field_in_a_row_is_reported_at_once() {
        let mut row = good_row();
        row.cells.insert("Invoice Number".into(), "".into());
        row.cells.insert("Rate".into(), "19".into());
        row.cells.insert("Reverse Charge".into(), "maybe".into());
        let report = validate(&GSTR1_B2B, &[row], &ctx());
        assert_eq!(report.findings.len(), 3, "{:?}", report.findings);
        assert!(report.records.is_empty());
    }

    #[test]
    fn invoice_number_that_is_numerically_zero_is_rejected() {
        for zero in ["0", "00"] {
            let f = only_finding(with("Invoice Number", zero));
            assert!(f.message.contains("cannot be zero"), "{}", f.message);
        }
        // '0.0' is also rejected, but by the character pattern — '.' is not an
        // allowed character — so it never reaches the zero check.
        let f = only_finding(with("Invoice Number", "0.0"));
        assert!(f.message.contains("invalid value"), "{}", f.message);

        // A non-numeric number is unaffected by the zero rule.
        let report = validate(&GSTR1_B2B, &[with("Invoice Number", "0A")], &ctx());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn invoice_number_pattern_rejects_disallowed_characters() {
        let f = only_finding(with("Invoice Number", "INV_001"));
        assert!(f.message.contains("invalid value"), "{}", f.message);
        // Seventeen characters is one too many.
        let f = only_finding(with("Invoice Number", "12345678901234567"));
        assert!(f.message.contains("invalid value"), "{}", f.message);
    }

    #[test]
    fn gstin_check_digit_is_enforced() {
        let f = only_finding(with("GSTIN/UIN of Recipient", "12GEOPS0823BBZA"));
        assert!(f.message.contains("check digit"), "{}", f.message);
    }

    #[test]
    fn rate_must_be_a_known_slab() {
        let f = only_finding(with("Rate", "19"));
        assert!(f.message.contains("must be one of"), "{}", f.message);
        // The unusual slabs really are allowed.
        for rate in ["0", "0.1", "0.25", "1.5", "7.5", "40"] {
            let report = validate(&GSTR1_B2B, &[with("Rate", rate)], &ctx());
            assert!(
                report.findings.is_empty(),
                "rate {rate}: {:?}",
                report.findings
            );
        }
    }

    #[test]
    fn place_of_supply_is_a_numeric_range_not_the_state_master() {
        // 96 is 'Other Country' and 28 was merged into 37, so neither is a
        // live state — but both fall in the accepted range, and the reference
        // implementation takes them. Constraining to the state master here
        // would wrongly reject 96.
        for pos in ["37", "37-Andhra Pradesh", "97-Other Territory", "96", "28"] {
            let report = validate(&GSTR1_B2B, &[with("Place Of Supply", pos)], &ctx());
            assert!(
                report.findings.is_empty(),
                "pos {pos}: {:?}",
                report.findings
            );
        }

        for pos in ["00", "39", "95", "98"] {
            let f = only_finding(with("Place Of Supply", pos));
            assert!(
                f.message.contains("not a valid place of supply"),
                "pos {pos}: {}",
                f.message
            );
        }
    }

    #[test]
    fn dates_outside_the_return_window_are_rejected() {
        let f = only_finding(with("Invoice date", "30-Jun-17"));
        assert!(f.message.contains("before 1 July 2017"), "{}", f.message);

        let f = only_finding(with("Invoice date", "01-Aug-17"));
        assert!(
            f.message.contains("after the end of the return period"),
            "{}",
            f.message
        );
    }

    #[test]
    fn excel_date_serials_are_accepted() {
        let report = validate(&GSTR1_B2B, &[with("Invoice date", "42930")], &ctx());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.records[0].text("idt"), "14-07-2017");
    }

    #[test]
    fn ui_invoice_type_labels_are_not_accepted_on_import() {
        let f = only_finding(with("Invoice Type", "Deemed Exports"));
        assert!(f.message.contains("must be one of"), "{}", f.message);

        let report = validate(&GSTR1_B2B, &[with("Invoice Type", "Deemed Exp")], &ctx());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.records[0].text("inv_typ"), "DE");
    }

    #[test]
    fn cross_field_rule_cbw_requires_reverse_charge() {
        let f = only_finding(with("Invoice Type", "Intra-State supplies attracting IGST"));
        assert_eq!(f.rule.as_deref(), Some("b2b.cbw_requires_rchrg"));
        // Rule findings name the rule rather than a single column.
        assert!(f.column.is_none());

        let mut row = with("Invoice Type", "Intra-State supplies attracting IGST");
        row.cells.insert("Reverse Charge".into(), "Y".into());
        let report = validate(&GSTR1_B2B, &[row], &ctx());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn cross_field_rule_deemed_export_forbids_reverse_charge() {
        let mut row = with("Invoice Type", "Deemed Exp");
        row.cells.insert("Reverse Charge".into(), "Y".into());
        let f = only_finding(row);
        assert_eq!(f.rule.as_deref(), Some("b2b.de_forbids_rchrg"));
    }

    #[test]
    fn ecommerce_gstin_is_rejected_even_when_otherwise_valid() {
        // Field-level `must_be_empty` fires first, so the row never reaches
        // the equivalent rule — one finding, not two.
        let f = only_finding(with("E-Commerce GSTIN", "12AJIPA1572E1C7"));
        assert_eq!(f.column.as_deref(), Some("E-Commerce GSTIN"));
        assert!(f.message.contains("must be blank"), "{}", f.message);
    }

    #[test]
    fn applicable_percent_accepts_only_the_two_permitted_values() {
        let report = validate(
            &GSTR1_B2B,
            &[with("Applicable % of Tax Rate", "65")],
            &ctx(),
        );
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(
            report.records[0].number("diff_percent"),
            Some(Decimal::new(65, 2))
        );

        let f = only_finding(with("Applicable % of Tax Rate", "50"));
        assert!(f.message.contains("must be one of"), "{}", f.message);
    }

    #[test]
    fn report_tracks_cleanliness_across_a_batch() {
        let rows = vec![good_row(), with("Rate", "19")];
        let report = validate(&GSTR1_B2B, &rows, &ctx());
        assert!(!report.is_clean());
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.errors().count(), 1);
    }
}
