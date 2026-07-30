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
        filing: SectionFiling,
        /// Output format for the report
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Print section counts and tax totals for a workbook (the pre-upload summary)
    Summary {
        workbook: PathBuf,
        #[command(flatten)]
        filing: Filing,
        /// Output format: a table, or the reference tool's meta JSON shape
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Generate the complete portal upload file from a whole workbook
    Upload {
        workbook: PathBuf,
        #[command(flatten)]
        filing: Filing,
        /// Aggregate turnover, if the period requires it
        #[arg(long)]
        gt: Option<String>,
        /// Current-period aggregate turnover
        #[arg(long)]
        cur_gt: Option<String>,
        /// Directory for the generated file
        #[arg(short, long, default_value = "out")]
        output: PathBuf,
    },
    /// Generate one section's payload from a workbook
    Generate {
        workbook: PathBuf,
        #[command(flatten)]
        filing: SectionFiling,
        /// Directory for the generated JSON file(s)
        #[arg(short, long, default_value = "out")]
        output: PathBuf,
    },
    /// Map a portal error file back to workbook rows
    Errors {
        error_file: PathBuf,
        workbook: PathBuf,
    },
    /// Semantically compare two portal upload files, or one file against the
    /// part set of a split upload
    Diff {
        left: PathBuf,
        /// One whole file, or every part of a split upload
        #[arg(required = true, num_args = 1..)]
        right: Vec<PathBuf>,
        /// Output format for the report
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
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
    /// Aggregate annual turnover exceeds 5 crore, which requires 6-digit HSN
    #[arg(long = "aato-over-5cr")]
    aato_over_5cr: bool,
    /// File quarterly (QRMP). In months 1 and 2 of a quarter this produces an
    /// IFF, which carries only B2B, B2BA, CDNR, CDNRA and the e-commerce tables
    #[arg(long)]
    quarterly: bool,
}

/// Filing details plus the section, for the single-section commands.
#[derive(Args)]
struct SectionFiling {
    #[command(flatten)]
    filing: Filing,
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
        Command::Upload {
            workbook,
            filing,
            gt,
            cur_gt,
            output,
        } => run_upload(&workbook, &filing, gt, cur_gt, &output),
        Command::Summary {
            workbook,
            filing,
            format,
        } => run_summary(&workbook, &filing, format),
        Command::Errors { .. } => unimplemented(
            "errors",
            "the portal error-file format is not specified yet",
        ),
        Command::Diff {
            left,
            right,
            format,
        } => run_diff(&left, &right, format),
    }
}

fn unimplemented(command: &str, why: &str) -> ExitCode {
    eprintln!("gst {command}: not implemented — {why}");
    ExitCode::from(EXIT_UNUSABLE)
}

/// Resolve the filing details, or explain what is wrong.
fn context(filing: &Filing) -> Result<FilingContext, String> {
    let period = ReturnPeriod::parse(&filing.period)
        .ok_or_else(|| format!("--period '{}' is not MMYYYY, e.g. 072017", filing.period))?;

    if !gst_core::gstin::checksum_valid(&filing.gstin) {
        return Err(format!(
            "--gstin '{}' is not a valid registration number (check digit failed)",
            filing.gstin
        ));
    }

    Ok(FilingContext {
        supplier_gstin: filing.gstin.clone(),
        period,
        is_sez: filing.sez,
        aato_over_5cr: filing.aato_over_5cr,
        is_quarterly: filing.quarterly,
    })
}

/// Prologue the single-section commands share: resolve the filing details and
/// section spec, then read that section's rows.
fn load(
    workbook: &Path,
    filing: &SectionFiling,
) -> Result<
    (
        FilingContext,
        &'static SectionSpec,
        Vec<gst_core::record::Row>,
    ),
    ExitCode,
> {
    let prepared = context(&filing.filing).and_then(|ctx| {
        let spec = spec::section(&filing.section).ok_or_else(|| {
            format!(
                "--section '{}' is not available yet; specified so far: {}",
                filing.section,
                spec::section_codes().join(", ")
            )
        })?;
        Ok((ctx, spec))
    });
    let (ctx, spec) = match prepared {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return Err(ExitCode::from(EXIT_UNUSABLE));
        }
    };

    match import::read(workbook, spec) {
        Ok(rows) => Ok((ctx, spec, rows)),
        Err(e) => {
            eprintln!("cannot read {}: {e}", workbook.display());
            Err(ExitCode::from(EXIT_UNUSABLE))
        }
    }
}

fn run_validate(workbook: &Path, filing: &SectionFiling, format: Format) -> ExitCode {
    let (ctx, spec, rows) = match load(workbook, filing) {
        Ok(v) => v,
        Err(code) => return code,
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
        "{errors} error(s), {warnings} warning(s); {envelopes} envelope(s) would be generated"
    );
}

fn report_json(findings: &[&Finding]) {
    use gst_core::payload::Json;
    // Built as a payload::Json object so the escaping rules live in one place;
    // insertion order keeps the shape stable regardless of the internal types.
    let items: Vec<Json> = findings
        .iter()
        .map(|f| {
            let mut obj = Json::obj();
            obj.insert_path("row", Json::Num(f.sheet_row.into()));
            if let Some(c) = &f.column {
                obj.insert_path("column", Json::Str(c.clone()));
            }
            if let Some(field) = &f.field {
                obj.insert_path("field", Json::Str(field.clone()));
            }
            if let Some(r) = &f.rule {
                obj.insert_path("rule", Json::Str(r.clone()));
            }
            let severity = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            obj.insert_path("severity", Json::Str(severity.to_owned()));
            obj.insert_path("message", Json::Str(f.message.clone()));
            obj
        })
        .collect();
    println!("{}", Json::Arr(items).to_json());
}

fn run_generate(workbook: &Path, filing: &SectionFiling, output: &Path) -> ExitCode {
    let (ctx, spec, rows) = match load(workbook, filing) {
        Ok(v) => v,
        Err(code) => return code,
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
        "wrote {} — {} envelope(s) from {} of {} row(s)",
        path.display(),
        grouped.envelopes.len(),
        report.records.len(),
        rows.len()
    );

    // Being explicit rather than letting anyone assume this file is uploadable.
    let unverified = spec.unverified_keys();
    println!(
        "\nnot a complete upload file: this is the '{}' section payload only — \
         run `gst upload` for the full portal envelope.",
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

/// Read every section the engine knows from one workbook and assemble the
/// complete upload file.
///
/// A section whose sheet is absent, or which has no rows, contributes nothing —
/// the envelope still carries its key, empty, as the reference does.
/// The prologue every whole-workbook command shares: resolve the filing
/// context and read, validate and group every section. The workbook must have
/// at least one section sheet with data. The `load` counterpart covers the
/// single-section commands.
fn load_workbook(
    workbook: &Path,
    filing: &Filing,
) -> Result<(FilingContext, gst_core::upload::WorkbookRun), ExitCode> {
    let ctx = context(filing).map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(EXIT_UNUSABLE)
    })?;
    let run = gst_core::upload::read_workbook(workbook, &ctx).map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(EXIT_UNUSABLE)
    })?;
    if run.stats.is_empty() {
        eprintln!(
            "no section sheets with data found in {}",
            workbook.display()
        );
        return Err(ExitCode::from(EXIT_UNUSABLE));
    }
    Ok((ctx, run))
}

/// Read and parse one JSON file into the payload AST, or explain why not.
fn parse_json_file(path: &Path) -> Result<gst_core::payload::Json, ExitCode> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("cannot read {}: {e}", path.display());
        ExitCode::from(EXIT_UNUSABLE)
    })?;
    gst_core::payload::parse(&text).map_err(|e| {
        eprintln!("{}: {e}", path.display());
        ExitCode::from(EXIT_UNUSABLE)
    })
}

fn run_diff(left: &Path, right: &[PathBuf], format: Format) -> ExitCode {
    use gst_core::diff::DiffKind;

    let left_json = match parse_json_file(left) {
        Ok(json) => json,
        Err(code) => return code,
    };
    let mut parts = Vec::new();
    for path in right {
        match parse_json_file(path) {
            Ok(json) => parts.push(json),
            Err(code) => return code,
        }
    }
    let merged_note = (parts.len() > 1).then(|| format!("{} parts merged", parts.len()));
    let (right_json, mut notes) = if parts.len() == 1 {
        (parts.remove(0), Vec::new())
    } else {
        match gst_core::upload::merge_parts(parts) {
            Ok(merged) => (merged.whole, merged.notes),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(EXIT_UNUSABLE);
            }
        }
    };

    let mut report = match gst_core::diff::diff(&left_json, &right_json) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };
    notes.extend(std::mem::take(&mut report.notes));
    if let Some(note) = merged_note {
        notes.insert(0, note);
    }

    match format {
        Format::Json => {
            use gst_core::payload::Json;
            let mut out = Json::obj();
            out.insert_path("identical", Json::Bool(report.identical()));
            let differences = report
                .differences
                .iter()
                .map(|d| {
                    let mut entry = Json::obj();
                    entry.insert_path("section", d.section.clone().map_or(Json::Null, Json::Str));
                    entry.insert_path("path", Json::Str(d.path.clone()));
                    entry.insert_path("kind", Json::Str(d.kind.as_str().to_owned()));
                    entry.insert_path("left", d.left.clone().map_or(Json::Null, Json::Str));
                    entry.insert_path("right", d.right.clone().map_or(Json::Null, Json::Str));
                    entry.insert_path("derived", Json::Bool(d.derived));
                    entry.insert_path(
                        "cause",
                        d.cause.map_or(Json::Null, |c| Json::Str(c.to_owned())),
                    );
                    entry
                })
                .collect();
            out.insert_path("differences", Json::Arr(differences));
            out.insert_path(
                "notes",
                Json::Arr(notes.iter().cloned().map(Json::Str).collect()),
            );
            println!("{}", out.to_json());
        }
        Format::Text => {
            let trim = |value: &Option<String>| -> String {
                match value {
                    None => "absent".to_owned(),
                    Some(v) if v.len() > 60 => format!("{}…", &v[..v.floor_char_boundary(59)]),
                    Some(v) => v.clone(),
                }
            };
            for d in &report.differences {
                let line = match d.kind {
                    DiffKind::RecordRemoved => format!("{}: only in left", d.path),
                    DiffKind::RecordAdded => format!("{}: only in right", d.path),
                    DiffKind::ModeMismatch => {
                        format!("{}: {} vs {}", d.path, trim(&d.left), trim(&d.right))
                    }
                    DiffKind::CountMismatch => format!(
                        "{}: {} record(s) -> {}",
                        d.path,
                        trim(&d.left),
                        trim(&d.right)
                    ),
                    _ => format!("{}: {} -> {}", d.path, trim(&d.left), trim(&d.right)),
                };
                let mut suffix = String::new();
                if d.kind == DiffKind::AbsentVsZero {
                    suffix.push_str(" (tax-neutral)");
                }
                if d.derived {
                    suffix.push_str(" (derived)");
                }
                if let Some(cause) = d.cause {
                    suffix.push_str(&format!(" (follows {cause})"));
                }
                println!("{line}{suffix}");
            }
            for note in &notes {
                println!("note: {note}");
            }
            if report.identical() {
                println!("files are semantically identical");
            } else {
                println!(
                    "\n{} difference(s), {} note(s)",
                    report.differences.len(),
                    notes.len()
                );
            }
        }
    }

    if report.identical() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_PROBLEMS)
    }
}

fn run_summary(workbook: &Path, filing: &Filing, format: Format) -> ExitCode {
    let (ctx, run) = match load_workbook(workbook, filing) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };

    let summaries = gst_core::summary::summarize(&run, &ctx);

    match format {
        Format::Json => println!(
            "{}",
            gst_core::summary::meta_json(&summaries, &ctx).to_json()
        ),
        Format::Text => {
            println!(
                "{} — pre-upload summary for {}\n",
                workbook.display(),
                ctx.period.as_mmyyyy()
            );
            // Label rows with our own section titles; the official labels
            // stay in the meta JSON.
            let title = |s: &gst_core::summary::SectionSummary| {
                gst_core::spec::section(s.cd).map_or(s.cd, |sec| sec.title.as_str())
            };
            if summaries.is_empty() {
                println!("(nothing to summarise)");
            } else {
                let width = summaries.iter().map(|s| title(s).len()).fold(7, usize::max);
                println!(
                    "{:<width$} {:>6} {:>12} {:>12} {:>12} {:>12}",
                    "section", "count", "cgst", "sgst", "igst", "cess"
                );
                for s in &summaries {
                    println!(
                        "{:<width$} {:>6} {:>12.2} {:>12.2} {:>12.2} {:>12.2}",
                        title(s),
                        s.count,
                        s.totals.cgst,
                        s.totals.sgst,
                        s.totals.igst,
                        s.totals.cess
                    );
                }
            }
            // The reference page carries the same caveat.
            println!(
                "\nNote: nil-rated (table 8) and documents-issued (table 13) sections carry\nno tax and are not summarised; their data still reaches the upload file."
            );
        }
    }

    let errors = run.errors().count();
    if errors > 0 {
        eprintln!(
            "{errors} error(s) — rejected rows are excluded from these totals; run `gst validate --section <name>` to see them"
        );
        return ExitCode::from(EXIT_PROBLEMS);
    }
    ExitCode::SUCCESS
}

fn run_upload(
    workbook: &Path,
    filing: &Filing,
    gt: Option<String>,
    cur_gt: Option<String>,
    output: &Path,
) -> ExitCode {
    let parse_turnover = |flag: &str, raw: Option<String>| match raw {
        None => Ok(None),
        Some(text) => gst_core::validate::parse_amount(&text)
            .map(Some)
            .ok_or_else(|| format!("--{flag} '{text}' is not a number")),
    };
    let turnover = match (parse_turnover("gt", gt), parse_turnover("cur-gt", cur_gt)) {
        (Ok(gross), Ok(current)) => {
            // The reference emits both turnover keys or neither: it branches on
            // the gross figure alone, and supplying only the current one makes
            // it write `"cur_gt":NaN` and then fail to parse its own file. So
            // one without the other is refused here rather than produced.
            if gross.is_some() != current.is_some() {
                eprintln!(
                    "--gt and --cur-gt go together: give both or neither \
                     (the reference emits both turnover figures or omits both)"
                );
                return ExitCode::from(EXIT_UNUSABLE);
            }
            gst_core::upload::Turnover { gross, current }
        }
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let (ctx, run) = match load_workbook(workbook, filing) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };

    let chunked = match run.chunks(&ctx, turnover) {
        Ok(chunked) => chunked,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_PROBLEMS);
        }
    };

    if let Err(e) = std::fs::create_dir_all(output) {
        eprintln!("cannot create {}: {e}", output.display());
        return ExitCode::from(EXIT_UNUSABLE);
    }
    let today = chrono::Local::now().date_naive();
    let parts = chunked.bodies.len();
    for (i, body) in chunked.bodies.iter().enumerate() {
        let name = if parts == 1 {
            gst_core::upload::filename(&ctx, today)
        } else {
            gst_core::upload::chunk_filename(&ctx, today, i + 1, parts)
        };
        let path = output.join(name);
        if let Err(e) = std::fs::write(&path, body) {
            eprintln!("cannot write {}: {e}", path.display());
            return ExitCode::from(EXIT_UNUSABLE);
        }
        println!("{}", path.display());
        if parts == 1 {
            println!("{} bytes\n", body.len());
        } else {
            println!("{} bytes", body.len());
        }
    }
    if parts > 1 {
        println!(
            "\nsplit into {parts} parts: the whole file measures {} bytes against the {} byte \
             chunk limit (as the reference measures it) — upload each part separately\n",
            chunked.unsplit_measure,
            gst_core::upload::max_chunk_bytes()
        );
    }
    println!(
        "{:<8} {:>6} {:>9} {:>10}",
        "section", "rows", "accepted", "records"
    );
    for stat in &run.stats {
        println!(
            "{:<8} {:>6} {:>9} {:>10}",
            stat.section, stat.rows, stat.accepted, stat.envelopes
        );
    }

    let read: usize = run.stats.iter().map(|s| s.rows).sum();
    let accepted: usize = run.stats.iter().map(|s| s.accepted).sum();
    let errors = run.errors().count();
    println!("\n{read} row(s) read, {accepted} accepted, {errors} error(s)");

    // Whole-sheet problems carry no row number and would otherwise be invisible
    // behind "run gst validate", which is a per-section command and cannot show
    // a section that was skipped outright.
    for finding in run
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Error && f.sheet_row == 0)
    {
        eprintln!("\n{}", finding.message);
    }

    if errors > 0 {
        eprintln!("run `gst validate --section <name>` to see the errors");
        return ExitCode::from(EXIT_PROBLEMS);
    }
    ExitCode::SUCCESS
}
