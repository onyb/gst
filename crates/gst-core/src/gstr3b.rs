//! FORM GSTR-3B — the second return, and the first form-shaped one.
//!
//! Where GSTR-1 is thirty row tables, GSTR-3B is one sheet of named cells:
//! the whole form reads into a single [`Record`] (plus 37 place-of-supply
//! mini-records for table 3.2), validation walks that record, and generation
//! emits one flat payload object. Everything — cell addresses, gates,
//! negativity windows, rules, emission order — comes from
//! `spec/gstr3b/form.json`, reverse-engineered from the official V5.8 Excel
//! VBA utility. The form does not enter the GSTR-1 section registry: like the
//! upload envelope and summary specs, it is a whole-return document with its
//! own static.
//!
//! Output is clean compact JSON, semantically identical to the utility's file
//! (which carries whitespace noise this writer deliberately does not
//! reproduce — see `emission.clean_json_divergence` in the spec).

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::date::{self, ReturnPeriod};
use crate::generate::round2;
use crate::import::{self, ImportError, Workbook};
use crate::payload::Json;
use crate::record::{Cell, Record};
use crate::spec::Severity;
use crate::validate::Finding;

// ---------------------------------------------------------------------------
// Spec types (mirroring spec/gstr3b/form.json and pos.json)

#[derive(Debug, Clone, Deserialize)]
struct CellRef {
    cell: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MonthRef {
    cell: String,
    fiscal_order: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Header {
    gstin: CellRef,
    legal_name: CellRef,
    fy: CellRef,
    month: MonthRef,
}

#[derive(Debug, Clone, Deserialize)]
struct Gate {
    from_period: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Gates {
    table_311: Gate,
    inter_sup_dropped: Gate,
}

#[derive(Debug, Clone, Deserialize)]
struct OutwardNegatives {
    from_period: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ItcNegatives {
    from_period: String,
    cells: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Negatives {
    outward: OutwardNegatives,
    itc: ItcNegatives,
}

/// A payload object drawn from one sheet row: `cells` maps payload leaf keys
/// to A1 addresses; leaves without a cell are structurally blocked and emit 0.
#[derive(Debug, Clone, Deserialize)]
struct KeyedRow {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    ty: Option<String>,
    cells: HashMap<String, String>,
    #[serde(default)]
    formulas: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SupDetails {
    rows: Vec<KeyedRow>,
}

#[derive(Debug, Clone, Deserialize)]
struct EcoDtls {
    eco_sup: KeyedRow,
    eco_reg_sup: KeyedRow,
}

#[derive(Debug, Clone, Deserialize)]
struct ItcNet {
    cached_cells: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ItcElg {
    itc_avl: Vec<KeyedRow>,
    itc_rev: Vec<KeyedRow>,
    itc_net: ItcNet,
    itc_inelg: Vec<KeyedRow>,
}

#[derive(Debug, Clone, Deserialize)]
struct InwardSup {
    rows: Vec<KeyedRow>,
}

#[derive(Debug, Clone, Deserialize)]
struct IntrLtfee {
    intr_details: KeyedRow,
    ltfee_details: KeyedRow,
}

#[derive(Debug, Clone, Deserialize)]
struct PairCols {
    txval: String,
    iamt: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Pairs {
    unreg_details: PairCols,
    comp_details: PairCols,
    uin_details: PairCols,
}

#[derive(Debug, Clone, Deserialize)]
struct PosRegion {
    first_row: u32,
    last_row: u32,
    pos_column: String,
    pairs: Pairs,
}

#[derive(Debug, Clone, Deserialize)]
struct FormSpec {
    sheet: String,
    header: Header,
    gates: Gates,
    negatives: Negatives,
    sup_details: SupDetails,
    eco_dtls: EcoDtls,
    itc_elg: ItcElg,
    inward_sup: InwardSup,
    intr_ltfee: IntrLtfee,
    pos_region: PosRegion,
}

#[derive(Debug, Clone, Deserialize)]
struct PosList {
    entries: Vec<String>,
    financial_years: Vec<String>,
}

static FORM: LazyLock<FormSpec> = LazyLock::new(|| {
    crate::masters::embedded(
        "gstr3b/form.json",
        include_str!("../../../spec/gstr3b/form.json"),
    )
});

static POS: LazyLock<PosList> = LazyLock::new(|| {
    crate::masters::embedded(
        "gstr3b/pos.json",
        include_str!("../../../spec/gstr3b/pos.json"),
    )
});

// ---------------------------------------------------------------------------
// A1 addressing

/// A1 → 0-based absolute `(row, col)`, as calamine's `Range::get_value` wants.
fn a1(cell: &str) -> (u32, u32) {
    let letters: String = cell
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let digits = &cell[letters.len()..];
    let col = letters.chars().fold(0u32, |acc, c| {
        acc * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1)
    });
    let row: u32 = digits
        .parse()
        .unwrap_or_else(|_| panic!("bad A1 address '{cell}'"));
    (row - 1, col - 1)
}

/// The 1-based sheet row of an A1 address, for finding locators.
fn a1_row(cell: &str) -> usize {
    (a1(cell).0 + 1) as usize
}

// ---------------------------------------------------------------------------
// Reading

/// One 3.2 row: the place-of-supply text plus its six amount cells, raw.
#[derive(Debug, Clone)]
pub struct PosRow {
    pub sheet_row: usize,
    /// The dropdown text verbatim ("06-Haryana"), empty when unselected.
    pub pos: String,
    /// `unreg_txval`, `unreg_iamt`, `comp_txval`, `comp_iamt`, `uin_txval`,
    /// `uin_iamt` — as validated cells (Empty / Number / Text-if-broken).
    pub record: Record,
}

/// The whole form, read but not yet judged.
#[derive(Debug, Clone)]
pub struct FormData {
    pub gstin: String,
    /// Entered on the form, never validated, never exported — carried only so
    /// nothing silently reads it back from the payload later.
    pub legal_name: String,
    pub fy: String,
    pub month: String,
    /// From FY + month; None when either is unselected or unrecognized.
    pub period: Option<ReturnPeriod>,
    /// Every fixed cell, keyed by field id (`osup_det_txval`, `itc_avl_impg_iamt`…).
    pub record: Record,
    pub pos_rows: Vec<PosRow>,
}

/// Field id → A1 cell for every fixed amount cell, plus whether the cell is a
/// mirror formula in the utility (SGST = CGST) and which field it mirrors.
struct FieldMap {
    entries: Vec<(String, String)>,
    mirrors: Vec<(String, String)>, // (formula field id, mirrored field id)
}

fn field_id(prefix: &str, row: &KeyedRow, leaf: &str) -> String {
    match (&row.key, &row.ty) {
        (Some(key), _) => format!("{key}_{leaf}"),
        (_, Some(ty)) => format!("{prefix}_{}_{leaf}", ty.to_lowercase()),
        _ => format!("{prefix}_{leaf}"),
    }
}

fn field_map() -> FieldMap {
    let spec = &*FORM;
    let mut entries = Vec::new();
    let mut mirrors = Vec::new();
    let mut add = |prefix: &str, row: &KeyedRow| {
        for (leaf, cell) in &row.cells {
            let id = field_id(prefix, row, leaf);
            entries.push((id.clone(), cell.clone()));
            if let Some(mirrored_cell) = row.formulas.get(cell) {
                // The formula target is another of this row's cells; find its leaf.
                if let Some((m_leaf, _)) = row.cells.iter().find(|(_, c)| *c == mirrored_cell) {
                    mirrors.push((id, field_id(prefix, row, m_leaf)));
                }
            }
        }
    };
    for row in &spec.sup_details.rows {
        add("sup", row);
    }
    add("eco_sup", &spec.eco_dtls.eco_sup);
    add("eco_reg_sup", &spec.eco_dtls.eco_reg_sup);
    for row in &spec.itc_elg.itc_avl {
        add("itc_avl", row);
    }
    for row in &spec.itc_elg.itc_rev {
        add("itc_rev", row);
    }
    for row in &spec.itc_elg.itc_inelg {
        add("itc_inelg", row);
    }
    for row in &spec.inward_sup.rows {
        add("isup", row);
    }
    add("intr", &spec.intr_ltfee.intr_details);
    add("ltfee", &spec.intr_ltfee.ltfee_details);
    for (leaf, cell) in &spec.itc_elg.itc_net.cached_cells {
        entries.push((format!("itc_net_{leaf}"), cell.clone()));
    }
    FieldMap { entries, mirrors }
}

/// Whether this workbook is a GSTR-3B form (carries the form sheet).
pub fn is_gstr3b(workbook: &Workbook) -> bool {
    workbook.sheet_names().iter().any(|s| s == &FORM.sheet)
}

fn classify(text: &str) -> Cell {
    if text.trim().is_empty() {
        Cell::Empty
    } else if let Ok(number) = text.trim().parse::<Decimal>() {
        Cell::Number(number)
    } else {
        Cell::Text(text.trim().to_owned())
    }
}

/// Read the form. Every fixed cell lands in one [`Record`]; formula cells
/// arrive as their cached values, exactly as the utility's exporter reads them.
pub fn read(path: &Path) -> Result<FormData, ImportError> {
    let mut workbook = Workbook::open(path)?;
    let range = workbook.range(&FORM.sheet)?;
    let text_at = |cell: &str| -> String {
        range
            .get_value(a1(cell))
            .map(import::cell_text)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };

    let gstin = text_at(&FORM.header.gstin.cell);
    let legal_name = text_at(&FORM.header.legal_name.cell);
    let fy = text_at(&FORM.header.fy.cell);
    let month = text_at(&FORM.header.month.cell);
    let period = date::period_from_financial_year(&fy, &month)
        .and_then(|mmyyyy| ReturnPeriod::parse(&mmyyyy));

    let map = field_map();
    let mut values = HashMap::new();
    for (id, cell) in &map.entries {
        values.insert(id.clone(), classify(&text_at(cell)));
    }
    let record = Record {
        sheet_row: a1_row(&FORM.header.gstin.cell),
        values,
    };

    let region = &FORM.pos_region;
    let mut pos_rows = Vec::new();
    for row in region.first_row..=region.last_row {
        let at = |col: &str| text_at(&format!("{col}{row}"));
        let pos = at(&region.pos_column);
        let mut values = HashMap::new();
        for (name, col) in [
            ("unreg_txval", &region.pairs.unreg_details.txval),
            ("unreg_iamt", &region.pairs.unreg_details.iamt),
            ("comp_txval", &region.pairs.comp_details.txval),
            ("comp_iamt", &region.pairs.comp_details.iamt),
            ("uin_txval", &region.pairs.uin_details.txval),
            ("uin_iamt", &region.pairs.uin_details.iamt),
        ] {
            values.insert(name.to_owned(), classify(&at(col)));
        }
        let record = Record {
            sheet_row: row as usize,
            values,
        };
        if !pos.is_empty() || record.values.values().any(|c| !c.is_empty()) {
            pos_rows.push(PosRow {
                sheet_row: row as usize,
                pos,
                record,
            });
        }
    }

    Ok(FormData {
        gstin,
        legal_name,
        fy,
        month,
        period,
        record,
        pos_rows,
    })
}

// ---------------------------------------------------------------------------
// Validation

const MAX_AMOUNT_TEXT: &str = "9999999999999.99";

fn finding(
    cell: &str,
    field: Option<&str>,
    rule: &str,
    severity: Severity,
    message: String,
) -> Finding {
    Finding {
        sheet_row: a1_row(cell),
        column: Some(cell.to_owned()),
        field: field.map(str::to_owned),
        rule: Some(rule.to_owned()),
        severity,
        message,
    }
}

fn period_at_least(period: Option<ReturnPeriod>, mmyyyy: &str) -> bool {
    match (period, crate::spec::period_as_yyyymm(mmyyyy)) {
        (Some(period), Some(gate)) => period.as_yyyymm() >= gate,
        _ => false,
    }
}

/// Judge the whole form. Field checks, then the structural rules; every
/// finding carries the A1 cell as its locator.
pub fn validate(form: &FormData) -> Vec<Finding> {
    let spec = &*FORM;
    let mut findings = Vec::new();
    let max: Decimal = MAX_AMOUNT_TEXT.parse().expect("constant parses");
    let min_neg: Decimal = -max;

    // Header: the utility's exact GSTIN rules, plus our checksum warning.
    let gstin_cell = &spec.header.gstin.cell;
    let gstin_ok = form.gstin.len() == 15
        && form.gstin[..2].chars().all(|c| c.is_ascii_digit())
        && form.gstin[..2]
            .parse::<u32>()
            .is_ok_and(|p| p < 39 && p != 28)
        && gstin_pattern_matches(&form.gstin);
    if !gstin_ok {
        findings.push(finding(
            gstin_cell,
            Some("gstin"),
            "gstr3b.gstin_valid",
            Severity::Error,
            "Please enter valid GSTIN".to_owned(),
        ));
    } else if !crate::gstin::checksum_valid(&form.gstin) {
        findings.push(finding(
            gstin_cell,
            Some("gstin"),
            "gstr3b.gstin_checksum",
            Severity::Warning,
            "the GSTIN's check digit fails — the utility never verifies it, but the portal will"
                .to_owned(),
        ));
    }
    if form.fy.is_empty() || !POS.financial_years.contains(&form.fy) {
        findings.push(finding(
            &spec.header.fy.cell,
            Some("fy"),
            "gstr3b.fy_selected",
            Severity::Error,
            "Please select return file year".to_owned(),
        ));
    }
    if form.month.is_empty() || !spec.header.month.fiscal_order.contains(&form.month) {
        findings.push(finding(
            &spec.header.month.cell,
            Some("month"),
            "gstr3b.month_selected",
            Severity::Error,
            "Please select return file month".to_owned(),
        ));
    }

    // Amounts: numeric, range, negativity per the windows.
    let outward_negatives = period_at_least(form.period, &spec.negatives.outward.from_period);
    let itc_negatives = period_at_least(form.period, &spec.negatives.itc.from_period);
    let map = field_map();
    for (id, cell) in &map.entries {
        let is_outward = id.starts_with("sup_")
            || id.starts_with("osup_")
            || id.starts_with("isup_rev_")
            || id.starts_with("eco_");
        let negatives_allowed = if spec.negatives.itc.cells.contains(cell) {
            itc_negatives
        } else if is_outward {
            outward_negatives
        } else {
            false
        };
        check_amount(
            cell,
            id,
            form.record.get(id),
            negatives_allowed,
            min_neg,
            max,
            &mut findings,
        );
    }
    for row in &form.pos_rows {
        for name in [
            "unreg_txval",
            "unreg_iamt",
            "comp_txval",
            "comp_iamt",
            "uin_txval",
            "uin_iamt",
        ] {
            let col = pos_col(name);
            let cell = format!("{col}{}", row.sheet_row);
            check_amount(
                &cell,
                name,
                row.record.get(name),
                outward_negatives,
                min_neg,
                max,
                &mut findings,
            );
        }
    }

    // SGST mirrors CGST — sheet formulas the workbook may have lost.
    for (formula_id, mirrored_id) in &map.mirrors {
        let formula = form.record.number(formula_id).unwrap_or_default();
        let mirrored = form.record.number(mirrored_id).unwrap_or_default();
        if formula != mirrored {
            let cell = map
                .entries
                .iter()
                .find(|(id, _)| id == formula_id)
                .map(|(_, c)| c.as_str())
                .unwrap_or_default();
            findings.push(finding(
                cell,
                Some(formula_id),
                "gstr3b.sgst_mirrors_cgst",
                Severity::Warning,
                format!(
                    "this cell is a formula in the utility (state tax = central tax) but \
                     disagrees with {mirrored_id} — the value is exported as-is"
                ),
            ));
        }
    }

    // itc_net drift: cached row 39 vs the recomputed formula.
    let computed = itc_net(&form.record);
    for (leaf, cell) in &spec.itc_elg.itc_net.cached_cells {
        let cached = form.record.get(&format!("itc_net_{leaf}"));
        if let Cell::Number(cached) = cached
            && *cached != computed[leaf.as_str()]
        {
            findings.push(finding(
                cell,
                Some(&format!("itc_net_{leaf}")),
                "gstr3b.itc_net_drift",
                Severity::Warning,
                format!(
                    "cached net ITC {cached} disagrees with the recomputed 4(C) formula \
                     value {} — the computed value is exported",
                    computed[leaf.as_str()]
                ),
            ));
        }
    }

    // Table 3.2 structure.
    let mut seen_pos: HashMap<String, usize> = HashMap::new();
    for row in &form.pos_rows {
        let pos_cell = format!("{}{}", spec.pos_region.pos_column, row.sheet_row);
        let any_amount = row.record.values.values().any(|c| !c.is_empty());
        if !row.pos.is_empty() {
            if !any_amount {
                findings.push(finding(
                    &pos_cell,
                    None,
                    "gstr3b.pos_without_amounts",
                    Severity::Error,
                    "Please enter Taxable value or amount of integrated tax in Table 3.2 \
                     for the selected Place of Supply"
                        .to_owned(),
                ));
            }
            if !POS.entries.contains(&row.pos) {
                findings.push(finding(
                    &pos_cell,
                    None,
                    "gstr3b.pos_unknown",
                    Severity::Error,
                    format!(
                        "'{}' is not a Place of Supply the utility's dropdown offers",
                        row.pos
                    ),
                ));
            }
            let prefix = pos_prefix(&row.pos);
            if let Some(first) = seen_pos.insert(prefix, row.sheet_row) {
                findings.push(finding(
                    &pos_cell,
                    None,
                    "gstr3b.pos_duplicate",
                    Severity::Error,
                    format!(
                        "Duplicate entries not allowed for Place of Supply(State/UT) — \
                         already selected on row {first}"
                    ),
                ));
            }
        } else if any_amount {
            findings.push(finding(
                &pos_cell,
                None,
                "gstr3b.amounts_without_pos",
                Severity::Error,
                "Please select Place of Supply in Table 3.2 for which Taxable value or \
                 amount of integrated tax is entered"
                    .to_owned(),
            ));
        }
    }

    // The one cross-table rule: 3.2 IGST within 3.1(a) (+ 3.1.1(i)).
    let inter_sup_dropped = period_at_least(form.period, &spec.gates.inter_sup_dropped.from_period);
    if !inter_sup_dropped {
        let igst_total: Decimal = form
            .pos_rows
            .iter()
            .flat_map(|row| ["unreg_iamt", "comp_iamt", "uin_iamt"].map(|n| row.record.number(n)))
            .flatten()
            .sum();
        let eco_live = period_at_least(form.period, &spec.gates.table_311.from_period);
        let d11 = form.record.number("osup_det_iamt").unwrap_or_default();
        let d22 = form.record.number("eco_sup_iamt").unwrap_or_default();
        let violated = if eco_live {
            // Post-072022 the rule fires only when 3.2 carries any IGST.
            igst_total != Decimal::ZERO && igst_total > d11 + d22
        } else {
            igst_total > d11
        };
        if violated {
            let cap = if eco_live { d11 + d22 } else { d11 };
            findings.push(finding(
                "D11",
                Some("osup_det_iamt"),
                "gstr3b.inter_sup_igst_within_31",
                Severity::Error,
                format!(
                    "Total amount of Integrated Tax in section 3.2 ({igst_total}) should be \
                     less than or equal to the Integrated tax declared in section 3.1(a){} \
                     which is Rs. {cap}",
                    if eco_live { " and 3.1.1(i)" } else { "" }
                ),
            ));
        }
    }

    findings
}

fn gstin_pattern_matches(gstin: &str) -> bool {
    // The utility's regex, anchored (its length-15 gate makes that equivalent).
    static PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            "^[0-9]{2}[a-zA-Z]{5}[0-9]{4}[a-zA-Z][1-9A-Za-z][Zz1-9A-Ja-j][0-9a-zA-Z]$",
        )
        .expect("pattern compiles")
    });
    PATTERN.is_match(gstin)
}

fn check_amount(
    cell: &str,
    field: &str,
    value: &Cell,
    negatives_allowed: bool,
    min_neg: Decimal,
    max: Decimal,
    findings: &mut Vec<Finding>,
) {
    let invalid = match value {
        Cell::Empty => false, // blank is valid and exports 0
        Cell::Number(n) => {
            let min = if negatives_allowed {
                min_neg
            } else {
                Decimal::ZERO
            };
            *n < min || *n > max
        }
        _ => true,
    };
    if invalid {
        findings.push(finding(
            cell,
            Some(field),
            "gstr3b.amount_valid",
            Severity::Error,
            "Please enter a valid amount. Field accepts only numeric value. \
             Maximum: 13 digits with two decimal places."
                .to_owned(),
        ));
    }
}

fn pos_col(name: &str) -> &str {
    let pairs = &FORM.pos_region.pairs;
    match name {
        "unreg_txval" => &pairs.unreg_details.txval,
        "unreg_iamt" => &pairs.unreg_details.iamt,
        "comp_txval" => &pairs.comp_details.txval,
        "comp_iamt" => &pairs.comp_details.iamt,
        "uin_txval" => &pairs.uin_details.txval,
        "uin_iamt" => &pairs.uin_details.iamt,
        _ => unreachable!("unknown pos pair field"),
    }
}

fn pos_prefix(pos: &str) -> String {
    pos.split('-').next().unwrap_or_default().to_owned()
}

/// The 4(C) formula per column: sum of rounded avl minus sum of rounded rev.
fn itc_net(record: &Record) -> HashMap<&'static str, Decimal> {
    let spec = &*FORM;
    let mut out = HashMap::new();
    for leaf in ["iamt", "camt", "samt", "csamt"] {
        let sum = |rows: &[KeyedRow], prefix: &str| -> Decimal {
            rows.iter()
                .map(|row| {
                    let id = field_id(prefix, row, leaf);
                    record
                        .number(&id)
                        .map(|d| round2(d.to_string().parse::<f64>().unwrap_or_default()))
                        .unwrap_or_default()
                })
                .sum()
        };
        let avl = sum(&spec.itc_elg.itc_avl, "itc_avl");
        let rev = sum(&spec.itc_elg.itc_rev, "itc_rev");
        out.insert(leaf, avl - rev);
    }
    out
}

// ---------------------------------------------------------------------------
// Generation

/// A fixed-block amount: blank emits 0, everything else rounds exactly as the
/// utility does (Excel ROUND, half away from zero on the double).
fn amount(record: &Record, id: &str) -> Json {
    match record.number(id) {
        None => Json::Num(Decimal::ZERO),
        Some(d) => Json::Num(round2(d.to_string().parse::<f64>().unwrap_or_default())),
    }
}

fn keyed_object(record: &Record, prefix: &str, row: &KeyedRow, leaves: &[&str]) -> Json {
    let mut obj = Json::obj();
    if let Some(ty) = &row.ty {
        obj.insert_path("ty", Json::Str(ty.clone()));
    }
    for leaf in leaves {
        obj.insert_path(leaf, amount(record, &field_id(prefix, row, leaf)));
    }
    obj
}

/// The upload payload, clean JSON in the utility's emission order.
pub fn generate(form: &FormData, period: ReturnPeriod) -> Json {
    let spec = &*FORM;
    let record = &form.record;
    let mut out = Json::obj();
    out.insert_path("gstin", Json::Str(form.gstin.clone()));
    out.insert_path("ret_period", Json::Str(period.as_mmyyyy()));

    let mut sup = Json::obj();
    for row in &spec.sup_details.rows {
        let key = row.key.as_deref().expect("sup rows are keyed");
        sup.insert_path(
            key,
            keyed_object(
                record,
                "sup",
                row,
                &["txval", "iamt", "camt", "samt", "csamt"],
            ),
        );
    }
    out.insert_path("sup_details", sup);

    if period_at_least(Some(period), &spec.gates.table_311.from_period) {
        let mut eco = Json::obj();
        eco.insert_path(
            "eco_sup",
            keyed_object(
                record,
                "eco_sup",
                &spec.eco_dtls.eco_sup,
                &["txval", "iamt", "camt", "samt", "csamt"],
            ),
        );
        eco.insert_path(
            "eco_reg_sup",
            keyed_object(
                record,
                "eco_reg_sup",
                &spec.eco_dtls.eco_reg_sup,
                &["txval"],
            ),
        );
        out.insert_path("eco_dtls", eco);
    }

    let leaves = ["iamt", "camt", "samt", "csamt"];
    let mut itc = Json::obj();
    itc.insert_path(
        "itc_avl",
        Json::Arr(
            spec.itc_elg
                .itc_avl
                .iter()
                .map(|row| keyed_object(record, "itc_avl", row, &leaves))
                .collect(),
        ),
    );
    itc.insert_path(
        "itc_rev",
        Json::Arr(
            spec.itc_elg
                .itc_rev
                .iter()
                .map(|row| keyed_object(record, "itc_rev", row, &leaves))
                .collect(),
        ),
    );
    let net = itc_net(record);
    let mut net_obj = Json::obj();
    for leaf in leaves {
        net_obj.insert_path(leaf, Json::Num(net[leaf]));
    }
    itc.insert_path("itc_net", net_obj);
    itc.insert_path(
        "itc_inelg",
        Json::Arr(
            spec.itc_elg
                .itc_inelg
                .iter()
                .map(|row| keyed_object(record, "itc_inelg", row, &leaves))
                .collect(),
        ),
    );
    out.insert_path("itc_elg", itc);

    let mut inward = Json::obj();
    inward.insert_path(
        "isup_details",
        Json::Arr(
            spec.inward_sup
                .rows
                .iter()
                .map(|row| keyed_object(record, "isup", row, &["inter", "intra"]))
                .collect(),
        ),
    );
    out.insert_path("inward_sup", inward);

    let mut intr = Json::obj();
    intr.insert_path(
        "intr_details",
        keyed_object(record, "intr", &spec.intr_ltfee.intr_details, &leaves),
    );
    // Late fee: the only unrounded values, and the only omitted keys.
    let mut ltfee = Json::obj();
    for leaf in ["camt", "samt"] {
        let id = field_id("ltfee", &spec.intr_ltfee.ltfee_details, leaf);
        if let Some(raw) = record.number(&id) {
            ltfee.insert_path(leaf, Json::Num(raw));
        }
    }
    intr.insert_path("ltfee_details", ltfee);
    out.insert_path("intr_ltfee", intr);

    if !period_at_least(Some(period), &spec.gates.inter_sup_dropped.from_period) {
        let mut inter = Json::obj();
        for (key, txval_field, iamt_field) in [
            ("unreg_details", "unreg_txval", "unreg_iamt"),
            ("comp_details", "comp_txval", "comp_iamt"),
            ("uin_details", "uin_txval", "uin_iamt"),
        ] {
            let rows = form
                .pos_rows
                .iter()
                .filter(|row| {
                    !row.pos.is_empty()
                        && (!row.record.get(txval_field).is_empty()
                            || !row.record.get(iamt_field).is_empty())
                })
                .map(|row| {
                    let mut obj = Json::obj();
                    obj.insert_path("pos", Json::Str(pos_prefix(&row.pos)));
                    obj.insert_path("txval", amount(&row.record, txval_field));
                    obj.insert_path("iamt", amount(&row.record, iamt_field));
                    obj
                })
                .collect();
            inter.insert_path(key, Json::Arr(rows));
        }
        out.insert_path("inter_sup", inter);
    }

    out
}

/// The utility's output name: `{Month}_{FYstartYear}-GSTR3B{GSTIN}-Details.json`.
/// The year is the FY START year even for January–March, so the name and
/// `ret_period` disagree by one in those months — faithfully reproduced.
pub fn filename(form: &FormData) -> String {
    let fy_start = form.fy.get(..4).unwrap_or_default();
    format!(
        "{}_{}-GSTR3B{}-Details.json",
        form.month, fy_start, form.gstin
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1_addresses_parse() {
        assert_eq!(a1("A1"), (0, 0));
        assert_eq!(a1("C5"), (4, 2));
        assert_eq!(a1("H124"), (123, 7));
        assert_eq!(a1("AA10"), (9, 26));
        assert_eq!(a1_row("D66"), 66);
    }

    #[test]
    fn the_spec_loads_and_every_cell_parses_uniquely() {
        let map = field_map();
        let mut seen = std::collections::HashSet::new();
        for (id, cell) in &map.entries {
            let _ = a1(cell);
            assert!(seen.insert(id.clone()), "duplicate field id {id}");
        }
        assert!(map.entries.len() > 50, "{}", map.entries.len());
        // The one genuine SGST input has no mirror; the nine formulas do.
        assert_eq!(map.mirrors.len(), 9, "{:?}", map.mirrors);
        assert!(!map.mirrors.iter().any(|(f, _)| f == "itc_rev_oth_samt"));
    }

    #[test]
    fn the_pos_list_is_the_utilitys_verbatim() {
        assert_eq!(POS.entries.len(), 38);
        assert!(!POS.entries.iter().any(|e| e.starts_with("28-")));
        assert!(POS.entries.contains(&"31-Lakshdweep".to_owned()));
        assert!(POS.entries.contains(&"36-Telengana".to_owned()));
        assert_eq!(POS.financial_years.last().unwrap(), "2026-27");
    }

    #[test]
    fn period_gates_sit_at_the_derived_boundaries() {
        let p = |mm: u32, yyyy: i32| ReturnPeriod::new(mm, yyyy).unwrap();
        // eco/3.1.1 from July 2022
        assert!(!period_at_least(Some(p(6, 2022)), "072022"));
        assert!(period_at_least(Some(p(7, 2022)), "072022"));
        // itc negatives from January 2023 (fiscal index 10 of FY 2022-23)
        assert!(!period_at_least(Some(p(12, 2022)), "012023"));
        assert!(period_at_least(Some(p(1, 2023)), "012023"));
        // outward negatives from September 2024
        assert!(!period_at_least(Some(p(8, 2024)), "092024"));
        assert!(period_at_least(Some(p(9, 2024)), "092024"));
        // inter_sup dropped from November 2025
        assert!(!period_at_least(Some(p(10, 2025)), "112025"));
        assert!(period_at_least(Some(p(11, 2025)), "112025"));
        assert!(period_at_least(Some(p(4, 2026)), "112025"));
    }

    #[test]
    fn the_filename_uses_the_fy_start_year_even_in_january() {
        let form = FormData {
            gstin: "27AAPFU0939F1ZV".into(),
            legal_name: String::new(),
            fy: "2024-25".into(),
            month: "January".into(),
            period: ReturnPeriod::parse("012025"),
            record: Record {
                sheet_row: 5,
                values: HashMap::new(),
            },
            pos_rows: vec![],
        };
        assert_eq!(
            filename(&form),
            "January_2024-GSTR3B27AAPFU0939F1ZV-Details.json"
        );
        assert_eq!(form.period.unwrap().as_mmyyyy(), "012025");
    }
}
