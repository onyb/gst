//! The pre-upload View Summary: per-section counts and tax-head totals.
//!
//! The reference computes this server-side from the working payload and shows
//! it before file generation; it never enters the upload file. Row order,
//! labels, count units, sign and zeroing rules all come from
//! `spec/gstr1/summary.json`, so the table is spec-driven like everything
//! else. Only the four tax heads are summed — the reference has no
//! taxable-value column; adding one later is one more alias in the spec's
//! `amount_keys` plus one accumulator here.

use rust_decimal::Decimal;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::payload::{Json, walk};
use crate::spec;
use crate::upload::{self, WorkbookRun};
use crate::validate::FilingContext;

/// The four summary columns, named as the display headers name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Column {
    Cgst,
    Sgst,
    Igst,
    Cess,
}

const COLUMNS: [Column; 4] = [Column::Cgst, Column::Sgst, Column::Igst, Column::Cess];

/// Payload key for each column: camt/samt/iamt/csamt everywhere except
/// supeco/supecoa, whose rows carry cgst/sgst/igst/cess.
#[derive(Debug, Clone, Deserialize)]
struct AmountKeys {
    cgst: String,
    sgst: String,
    igst: String,
    cess: String,
}

impl AmountKeys {
    fn key(&self, column: Column) -> &str {
        match column {
            Column::Cgst => &self.cgst,
            Column::Sgst => &self.sgst,
            Column::Igst => &self.igst,
            Column::Cess => &self.cess,
        }
    }
}

/// Subtract instead of add when the field on the counted node takes one of
/// these values — credit notes and refund vouchers reduce the totals.
#[derive(Debug, Clone, Deserialize)]
struct Negate {
    field: String,
    values: Vec<String>,
}

/// Where a rule's field is read: on the counted node or on the payload object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum At {
    Count,
    Envelope,
}

/// Blank the listed columns when the field equals the value — the
/// without-payment supplies show zero IGST and cess regardless of stored tax.
#[derive(Debug, Clone, Deserialize)]
struct Zero {
    field: String,
    at: At,
    equals: String,
    columns: Vec<Column>,
}

#[derive(Debug, Clone, Deserialize)]
struct RowSpec {
    cd: String,
    name: String,
    #[serde(default)]
    excluded: bool,
    #[serde(default)]
    count: Vec<String>,
    #[serde(default)]
    amounts: Vec<String>,
    #[serde(default)]
    amount_keys: Option<AmountKeys>,
    #[serde(default)]
    negate: Option<Negate>,
    #[serde(default)]
    zero: Option<Zero>,
    #[serde(default)]
    from_period: Option<String>,
    #[serde(default)]
    until_period: Option<String>,
}

impl RowSpec {
    fn covers(&self, period: u32) -> bool {
        spec::period_window(&self.from_period, &self.until_period, period)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SummarySpec {
    amount_keys: AmountKeys,
    rows: Vec<RowSpec>,
}

static SUMMARY: LazyLock<SummarySpec> = LazyLock::new(|| {
    crate::masters::embedded(
        "gstr1/summary.json",
        include_str!("../../../spec/gstr1/summary.json"),
    )
});

/// Running totals for the four tax heads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub cgst: Decimal,
    pub sgst: Decimal,
    pub igst: Decimal,
    pub cess: Decimal,
}

impl Totals {
    fn slot(&mut self, column: Column) -> &mut Decimal {
        match column {
            Column::Cgst => &mut self.cgst,
            Column::Sgst => &mut self.sgst,
            Column::Igst => &mut self.igst,
            Column::Cess => &mut self.cess,
        }
    }
}

/// One row of the pre-upload summary, in the official display order.
#[derive(Debug, Clone)]
pub struct SectionSummary {
    /// Section code, equal to the official meta `cd`.
    pub cd: &'static str,
    /// The official tool's label, verbatim — typos included.
    pub name: &'static str,
    pub count: usize,
    pub totals: Totals,
}

/// Every declared summary row's section code, for coverage checks.
pub fn covered_sections() -> impl Iterator<Item = &'static str> {
    SUMMARY.rows.iter().map(|row| row.cd.as_str())
}

/// Compute the summary rows for one workbook run.
///
/// Rows with a count of 0 are omitted, as the reference omits them from the
/// counts array itself — which is also what keeps `nil` and `doc_issue` out.
/// Order is the official server order, not [`spec::sections()`] order.
pub fn summarize(run: &WorkbookRun, ctx: &FilingContext) -> Vec<SectionSummary> {
    let period = ctx.period.as_yyyymm();
    let mut out = Vec::new();
    for row in &SUMMARY.rows {
        if row.excluded || !row.covers(period) {
            continue;
        }
        let Some(generated) = run.sections.get(&row.cd) else {
            continue;
        };
        let keys = row.amount_keys.as_ref().unwrap_or(&SUMMARY.amount_keys);
        let hits = |z: &&Zero, holder: &Json| {
            field_matches(holder, &z.field, std::slice::from_ref(&z.equals))
        };
        let mut count = 0;
        let mut totals = Totals::default();
        for envelope in &generated.envelopes {
            // An envelope-level zero rule is invariant across the nodes below.
            let envelope_zero = row
                .zero
                .as_ref()
                .filter(|z| z.at == At::Envelope && hits(z, envelope));
            for node in walk(envelope, &row.count) {
                count += 1;
                let negate = row
                    .negate
                    .as_ref()
                    .is_some_and(|n| field_matches(node, &n.field, &n.values));
                let zeroed: &[Column] = envelope_zero
                    .or_else(|| {
                        row.zero
                            .as_ref()
                            .filter(|z| z.at == At::Count && hits(z, node))
                    })
                    .map_or(&[], |z| &z.columns);
                for amounts in walk(node, &row.amounts) {
                    accumulate(&mut totals, amounts, keys, negate, zeroed);
                }
            }
        }
        if count > 0 {
            out.push(SectionSummary {
                cd: &row.cd,
                name: &row.name,
                count,
                totals,
            });
        }
    }
    out
}

/// The `_meta.json` sidecar shape the reference caches the summary in:
/// `{gstin, fp, version, hash, counts: [{cd, result, count, name}]}` with
/// `result` as `{cgTl, sgTl, igTl, csTl}`, key orders matching the reference.
/// Turnover (`gt`/`cur_gt`) appears in the reference's meta only when the
/// working file carries it; this engine summarises without turnover.
pub fn meta_json(summaries: &[SectionSummary], ctx: &FilingContext) -> Json {
    // The reference copies the working file's header keys and appends counts;
    // the envelope spec already owns that header.
    let mut meta = upload::header(ctx, upload::Turnover::default());
    let counts = summaries
        .iter()
        .map(|s| {
            let mut result = Json::obj();
            result.insert_path("cgTl", Json::Num(s.totals.cgst));
            result.insert_path("sgTl", Json::Num(s.totals.sgst));
            result.insert_path("igTl", Json::Num(s.totals.igst));
            result.insert_path("csTl", Json::Num(s.totals.cess));
            let mut entry = Json::obj();
            entry.insert_path("cd", Json::Str(s.cd.to_owned()));
            entry.insert_path("result", result);
            entry.insert_path("count", Json::Num(Decimal::from(s.count as u64)));
            entry.insert_path("name", Json::Str(s.name.to_owned()));
            entry
        })
        .collect();
    meta.insert_path("counts", Json::Arr(counts));
    meta
}

fn field_matches(node: &Json, field: &str, values: &[String]) -> bool {
    match node.get(field) {
        Some(Json::Str(v)) => values.iter().any(|x| x == v),
        _ => false,
    }
}

/// The reference re-rounds its running totals with `parseFloat(toFixed(2))`
/// on doubles. Every stored amount is already a two-place decimal, and the
/// sum of two-place doubles at money magnitudes never lands on a false
/// midpoint, so exact decimal rounding reproduces the double path — unlike
/// generation, where `generate::round2` must model the doubles themselves.
fn round2(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

/// Add one amount object's contribution. A missing or non-numeric key
/// contributes nothing (the reference guards each read with JS truthiness, so
/// 0 and absent are equivalent); a zeroed column contributes nothing either.
/// The running total re-rounds after every addition, as the reference does —
/// a no-op over two-place inputs, but it keeps the fidelity argument exact.
fn accumulate(
    totals: &mut Totals,
    amounts: &Json,
    keys: &AmountKeys,
    negate: bool,
    zeroed: &[Column],
) {
    for column in COLUMNS {
        if zeroed.contains(&column) {
            continue;
        }
        let Some(Json::Num(value)) = amounts.get(keys.key(column)) else {
            continue;
        };
        let contribution = if negate { -*value } else { *value };
        let slot = totals.slot(column);
        *slot = round2(*slot + contribution);
        // A credit cancelling a debit leaves Decimal's negative zero, which
        // Display renders as "-0.00" in the CLI table. (Serialization is safe
        // regardless — normalize() clears the sign of zero.) The reference's
        // parseFloat(toFixed(2)) lands on +0 the same way.
        if slot.is_zero() {
            *slot = Decimal::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(n: i64) -> Json {
        Json::Num(Decimal::from(n))
    }

    fn item(pairs: &[(&str, i64)]) -> Json {
        let mut o = Json::obj();
        for (k, v) in pairs {
            o.insert_path(k, num(*v));
        }
        o
    }

    #[test]
    fn walk_fans_out_arrays_and_skips_missing_members() {
        let mut inv = Json::obj();
        inv.insert_path(
            "itms",
            Json::Arr(vec![item(&[("rt", 18)]), item(&[("rt", 5)])]),
        );
        let mut envelope = Json::obj();
        envelope.insert_path("inv", Json::Arr(vec![inv]));

        assert_eq!(walk(&envelope, &[]).len(), 1);
        assert_eq!(walk(&envelope, &["inv".into()]).len(), 1);
        assert_eq!(walk(&envelope, &["inv".into(), "itms".into()]).len(), 2);
        assert_eq!(walk(&envelope, &["nt".into()]).len(), 0);
    }

    #[test]
    fn accumulation_rounds_negates_and_skips_missing_keys() {
        // The spec's default alias set is exactly camt/samt/iamt/csamt.
        let keys = &SUMMARY.amount_keys;
        let mut totals = Totals::default();
        accumulate(
            &mut totals,
            &item(&[("camt", 90), ("samt", 90)]),
            keys,
            false,
            &[],
        );
        accumulate(
            &mut totals,
            &item(&[("iamt", 100), ("csamt", 5)]),
            keys,
            true,
            &[],
        );
        assert_eq!(totals.cgst, Decimal::from(90));
        assert_eq!(totals.sgst, Decimal::from(90));
        assert_eq!(totals.igst, Decimal::from(-100));
        assert_eq!(totals.cess, Decimal::from(-5));

        let mut zeroed = Totals::default();
        accumulate(
            &mut zeroed,
            &item(&[("iamt", 100), ("csamt", 5), ("camt", 7)]),
            keys,
            false,
            &[Column::Igst, Column::Cess],
        );
        assert_eq!(zeroed.igst, Decimal::ZERO);
        assert_eq!(zeroed.cess, Decimal::ZERO);
        assert_eq!(zeroed.cgst, Decimal::from(7));
    }

    #[test]
    fn a_cancelled_total_is_positive_zero_like_the_reference() {
        let keys = &SUMMARY.amount_keys;
        let mut totals = Totals::default();
        accumulate(&mut totals, &item(&[("csamt", 500)]), keys, true, &[]);
        accumulate(&mut totals, &item(&[("csamt", 500)]), keys, false, &[]);
        // Without the collapse, Decimal's negative zero Displays as "-0.00"
        // in the CLI table.
        assert_eq!(format!("{:.2}", totals.cess), "0.00");
    }

    #[test]
    fn period_gates_are_from_inclusive_until_exclusive() {
        let row: RowSpec = serde_json::from_str(
            r#"{"cd":"x","name":"x","from_period":"012024","until_period":"052025"}"#,
        )
        .unwrap();
        assert!(!row.covers(202312));
        assert!(row.covers(202401));
        assert!(row.covers(202504));
        assert!(!row.covers(202505));
    }
}
