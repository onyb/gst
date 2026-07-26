use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};

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
    Validate { workbook: PathBuf },
    /// Print section totals for a workbook (the pre-upload summary)
    Summary { workbook: PathBuf },
    /// Generate portal upload JSON from a workbook
    Generate {
        workbook: PathBuf,
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

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Validate { workbook } => bail!("not implemented: validate {}", workbook.display()),
        Command::Summary { workbook } => bail!("not implemented: summary {}", workbook.display()),
        Command::Generate { workbook, output } => bail!(
            "not implemented: generate {} into {}",
            workbook.display(),
            output.display()
        ),
        Command::Errors {
            error_file,
            workbook,
        } => bail!(
            "not implemented: errors {} against {}",
            error_file.display(),
            workbook.display()
        ),
        Command::Diff { left, right } => bail!(
            "not implemented: diff {} {}",
            left.display(),
            right.display()
        ),
    }
}
