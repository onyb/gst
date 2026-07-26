use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use gst_core::date::ReturnPeriod;
use gst_core::generate::generate;
use gst_core::import;
use gst_core::spec::{self, SectionSpec, Severity};
use gst_core::validate::{FilingContext, Finding, validate};

/// Prepare Indian GST returns offline: validate Excel/CSV workbooks and
/// generate portal-ready upload JSON. Runs fully offline.
#[derive(Parser)]
#[command(name = "gst", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate a workbook and report errors with sheet/row/column references
    Validate {
        workbook: PathBuf,
        #[command(flatten)]
        filing: Filing,
        /// Output format for the report
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Print section totals for a workbook (the pre-upload summary)
    Summary {
        workbook: PathBuf,
        #[command(flatten)]
        filing: Filing,
    },
    /// Generate portal upload JSON from a workbook
    Generate {
        workbook: PathBuf,
        #[command(flatten)]
        filing: Filing,
        /// Directory for the generated JSON file(s)
        #[arg(short, long, default_value = "out")]
        output: PathBuf,
    },
    /// Map a portal error file back to workbook rows
    Errors {
        error_file: PathBuf,
        workbook: PathBuf,
    },
    /// Semantically diff two portal JSON files
    Diff { left: PathBuf, right: PathBuf },
}

/// Details the workbook cannot supply: the official templates carry no
/// supplier GSTIN or return period, since the tool takes both from its UI.
#[derive(Args)]
struct Filing {
    /// Your own GSTIN, as the filer. Determines the intra/inter-state split.
    #[arg(long)]
    gstin: String,
    /// Return period as MMYYYY, e.g. 072017
    #[arg(long)]
    period: String,
    /// Treat the filer as an SEZ unit, which makes every supply inter-state
    #[arg(long)]
    sez: bool,
    /// Section to read. Auto-detection from workbook shape is not built yet.
    #[arg(long, default_value = "b2b")]
    section: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

/// 0 clean, 1 problems found, 2 the command could not run — so `gst` drops
/// into a shell pipeline as a pre-upload gate.
const EXIT_PROBLEMS: u8 = 1;
const EXIT_UNUSABLE: u8 = 2;

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate {
            workbook,
            filing,
            format,
        } => run_validate(&workbook, &filing, format),
        Command::Generate {
            workbook,
            filing,
            output,
        } => run_generate(&workbook, &filing, &output),
        Command::Summary { .. } => {
            unimplemented("summary", "the section total calculator is not built yet")
        }
        Command::Errors { .. } => unimplemented(
            "errors",
            "the portal error-file format is not specified yet",
        ),
        Command::Diff { .. } => unimplemented("diff", "the canonicalizer is not built yet"),
    }
}

fn unimplemented(command: &str, why: &str) -> ExitCode {
    eprintln!("gst {command}: not implemented — {why}");
    ExitCode::from(EXIT_UNUSABLE)
}

/// Resolve the filing details and section spec, or explain what is wrong.
fn prepare(filing: &Filing) -> Result<(FilingContext, &'static SectionSpec), String> {
    let period = ReturnPeriod::parse(&filing.period)
        .ok_or_else(|| format!("--period '{}' is not MMYYYY, e.g. 072017", filing.period))?;

    let spec = spec::section(&filing.section).ok_or_else(|| {
        format!(
            "--section '{}' is not available yet; specified so far: {}",
            filing.section,
            spec::section_codes().join(", ")
        )
    })?;

    if !gst_core::gstin::checksum_valid(&filing.gstin) {
        return Err(format!(
            "--gstin '{}' is not a valid registration number (check digit failed)",
            filing.gstin
        ));
    }

    Ok((
        FilingContext {
            supplier_gstin: filing.gstin.clone(),
            period,
            is_sez: filing.sez,
        },
        spec,
    ))
}

fn run_validate(workbook: &Path, filing: &Filing, format: Format) -> ExitCode {
    let (ctx, spec) = match prepare(filing) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let rows = match import::read(workbook, spec) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("cannot read {}: {e}", workbook.display());
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let report = validate(spec, &rows, &ctx);
    // Grouping surfaces problems no single row shows, so a full validation has
    // to run generation too.
    let grouped = generate(spec, &report.records, &ctx);

    let mut findings: Vec<&Finding> = report
        .findings
        .iter()
        .chain(grouped.findings.iter())
        .collect();
    findings.sort_by(|a, b| {
        a.sheet_row
            .cmp(&b.sheet_row)
            .then_with(|| a.column.cmp(&b.column))
    });

    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();

    match format {
        Format::Text => report_text(
            spec,
            workbook,
            &rows,
            &findings,
            errors,
            grouped.envelopes.len(),
        ),
        Format::Json => report_json(&findings),
    }

    if errors > 0 {
        ExitCode::from(EXIT_PROBLEMS)
    } else {
        ExitCode::SUCCESS
    }
}

fn report_text(
    spec: &SectionSpec,
    workbook: &Path,
    rows: &[gst_core::record::Row],
    findings: &[&Finding],
    errors: usize,
    envelopes: usize,
) {
    println!("{} — {}", workbook.display(), spec.title);
    println!("{} row(s) read\n", rows.len());

    if findings.is_empty() {
        println!("no problems found");
    } else {
        for f in findings {
            let tag = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warn ",
            };
            let place = match (&f.column, &f.rule) {
                (Some(c), _) => format!("row {} · {c}", f.sheet_row),
                (None, Some(r)) => format!("row {} · [{r}]", f.sheet_row),
                _ => format!("row {}", f.sheet_row),
            };
            println!("{tag} {place}\n      {}", f.message);
        }
        println!();
    }

    let warnings = findings.len() - errors;
    println!(
        "{errors} error(s), {warnings} warning(s); {envelopes} recipient envelope(s) would be generated"
    );
}

fn report_json(findings: &[&Finding]) {
    // Hand-built so the shape stays stable regardless of the internal types.
    let items: Vec<String> = findings
        .iter()
        .map(|f| {
            let mut parts = vec![format!("\"row\":{}", f.sheet_row)];
            if let Some(c) = &f.column {
                parts.push(format!("\"column\":{}", quote(c)));
            }
            if let Some(field) = &f.field {
                parts.push(format!("\"field\":{}", quote(field)));
            }
            if let Some(r) = &f.rule {
                parts.push(format!("\"rule\":{}", quote(r)));
            }
            parts.push(format!(
                "\"severity\":{}",
                quote(match f.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                })
            ));
            parts.push(format!("\"message\":{}", quote(&f.message)));
            format!("{{{}}}", parts.join(","))
        })
        .collect();
    println!("[{}]", items.join(","));
}

fn quote(s: &str) -> String {
    let escaped: String = s
        .chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            '\n' => vec!['\\', 'n'],
            c => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}

fn run_generate(workbook: &Path, filing: &Filing, output: &Path) -> ExitCode {
    let (ctx, spec) = match prepare(filing) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let rows = match import::read(workbook, spec) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("cannot read {}: {e}", workbook.display());
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let report = validate(spec, &rows, &ctx);
    let grouped = generate(spec, &report.records, &ctx);
    let rejected = rows.len() - report.records.len();

    if let Err(e) = std::fs::create_dir_all(output) {
        eprintln!("cannot create {}: {e}", output.display());
        return ExitCode::from(EXIT_UNUSABLE);
    }
    let path = output.join(format!("{}-{}.json", spec.return_type, spec.section));
    if let Err(e) = std::fs::write(&path, grouped.to_json()) {
        eprintln!("cannot write {}: {e}", path.display());
        return ExitCode::from(EXIT_UNUSABLE);
    }

    println!(
        "wrote {} — {} recipient envelope(s) from {} of {} row(s)",
        path.display(),
        grouped.envelopes.len(),
        report.records.len(),
        rows.len()
    );

    // Being explicit rather than letting anyone assume this file is uploadable.
    let unverified = spec.unverified_keys();
    println!(
        "\nnot yet a complete upload file: this is the '{}' section payload only. \
         The outer envelope the portal expects (gstin, return period, version) \
         is not specified yet.",
        spec.section
    );
    if !unverified.is_empty() {
        println!(
            "unconfirmed payload key(s) pending differential capture: {}",
            unverified.join(", ")
        );
    }

    if rejected > 0 {
        eprintln!("\n{rejected} row(s) were rejected — run `gst validate` to see why");
        return ExitCode::from(EXIT_PROBLEMS);
    }
    ExitCode::SUCCESS
}
