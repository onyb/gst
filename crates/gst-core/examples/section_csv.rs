//! Run a GSTR-1 section CSV through validation and JSON generation.
//!
//! A demonstration of the engine end-to-end while the real importer is still
//! to come: it reads a section CSV by header name, so it depends on the spec's
//! column definitions but not yet on the Excel reader.
//!
//! ```text
//! cargo run -p gst-core --example section_csv -- <csv> <supplier-gstin> <MMYYYY>
//! ```
//!
//! Two inputs ship alongside it, both self-contained:
//!
//! - `b2b-sample.csv` — authored here; seven rows covering the intra/inter
//!   split, a multi-rate invoice, deemed export with cess, SEZ without payment
//!   of tax, and the 65% applicable rate. Validates clean.
//! - `b2b-gstn-sample.csv` — the sample data GSTN distributes with the Returns
//!   Offline Tool as `Section_wise_CSV_files/GSTR1/b2b,sez,de.csv`, kept
//!   verbatim as a real-world input. It no longer passes the tool's own
//!   current validation (see the `quirk` note on this in the B2B spec), which
//!   makes it a useful exercise of the error reporting.

use std::process::ExitCode;

use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::record::Row;
use gst_core::spec::{GSTR1_B2B, Severity};
use gst_core::validate::{FilingContext, validate};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, supplier_gstin, period] = args.as_slice() else {
        eprintln!(
            "usage: section_csv <csv> <supplier-gstin> <MMYYYY>\n\
             \n\
             examples (both files ship with this crate):\n\
             \x20 section_csv crates/gst-core/examples/b2b-sample.csv 27AAPFU0939F1ZV 072017\n\
             \x20 section_csv crates/gst-core/examples/b2b-gstn-sample.csv 27AAPFU0939F1ZV 072017"
        );
        return ExitCode::from(2);
    };

    let Some(period) = parse_period(period) else {
        eprintln!("period must be MMYYYY, e.g. 072017");
        return ExitCode::from(2);
    };
    let ctx = FilingContext {
        supplier_gstin: supplier_gstin.clone(),
        period,
        is_sez: false,
    };

    let spec = &*GSTR1_B2B;
    let mut reader = match csv::Reader::from_path(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let headers = match reader.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("cannot read headers: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut rows = Vec::new();
    for (i, record) in reader.records().enumerate() {
        let Ok(record) = record else { continue };
        // CSV line number: the header is line 1, so data starts at line 2.
        let mut row = Row::new(i + 2);
        for (header, value) in headers.iter().zip(record.iter()) {
            row.cells.insert(header.to_owned(), value.to_owned());
        }
        rows.push(row);
    }

    println!("section : {} ({})", spec.section, spec.title);
    println!(
        "supplier: {supplier_gstin}  period: {:02}-{}",
        period.month, period.year
    );
    println!("rows    : {}\n", rows.len());

    let report = validate(spec, &rows, &ctx);
    let out = generate(spec, &report.records, &ctx);

    let mut findings: Vec<_> = report.findings.iter().chain(out.findings.iter()).collect();
    findings.sort_by_key(|f| (f.sheet_row, f.column.clone()));

    if findings.is_empty() {
        println!("no problems found");
    } else {
        println!("{} problem(s):", findings.len());
        for f in &findings {
            let tag = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warn ",
            };
            let where_ = match (&f.column, &f.rule) {
                (Some(c), _) => format!("line {} · {c}", f.sheet_row),
                (None, Some(r)) => format!("line {} · [{r}]", f.sheet_row),
                _ => format!("line {}", f.sheet_row),
            };
            println!("  {tag} {where_}\n        {}", f.message);
        }
    }

    println!(
        "\naccepted {} of {} rows into {} recipient envelope(s)",
        report.records.len(),
        rows.len(),
        out.envelopes.len()
    );

    if !out.envelopes.is_empty() {
        println!("\npayload:\n{}", out.to_json());
    }

    // Non-zero when anything was rejected, so the example behaves like the CLI
    // will: usable as a pre-upload gate in a shell pipeline.
    if report.records.len() == rows.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_period(text: &str) -> Option<ReturnPeriod> {
    if text.len() != 6 {
        return None;
    }
    let month = text[..2].parse().ok()?;
    let year = text[2..].parse().ok()?;
    ReturnPeriod::new(month, year)
}
