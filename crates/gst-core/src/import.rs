//! Reading workbook rows, driven by the spec's `source` block.
//!
//! Both inputs land on the same [`Row`] shape — cells keyed by the column
//! header the spec declares — so validation and generation never learn which
//! format a row came from. Columns are matched by header text rather than
//! position, because operators reorder and hide columns in practice; a missing
//! column is an import failure, an unrecognized extra one is ignored.

use std::path::Path;

use calamine::{Data, Reader};

use crate::record::Row;
use crate::spec::{Field, SectionSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    Io(String),
    /// The spec's sheet is not in the workbook. Carries what is, since the
    /// usual cause is a renamed or wrong-template file.
    SheetMissing {
        sheet: String,
        available: Vec<String>,
    },
    /// The header row exists but the sheet is shorter than the spec expects.
    HeaderRowMissing {
        sheet: String,
        header_row: usize,
    },
    /// Columns the spec requires that the sheet does not have.
    MissingColumns(Vec<String>),
    /// The spec does not describe this input format for this section.
    UnsupportedSource(&'static str),
    /// Extension is neither a spreadsheet nor a CSV.
    UnrecognizedFormat(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io(e) => write!(f, "{e}"),
            ImportError::SheetMissing { sheet, available } => write!(
                f,
                "the workbook has no sheet named '{sheet}'. It contains: {}",
                available.join(", ")
            ),
            ImportError::HeaderRowMissing { sheet, header_row } => write!(
                f,
                "sheet '{sheet}' has no row {header_row}, where the column headers should be"
            ),
            ImportError::MissingColumns(columns) => write!(
                f,
                "these columns are missing: {}",
                columns
                    .iter()
                    .map(|c| format!("'{c}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ImportError::UnsupportedSource(what) => {
                write!(f, "this section cannot be imported from {what}")
            }
            ImportError::UnrecognizedFormat(ext) => write!(
                f,
                "unrecognized file type '{ext}' — expected .xlsx, .xls, .xlsm or .csv"
            ),
        }
    }
}

impl std::error::Error for ImportError {}

/// Read a section from a file, choosing the reader by extension.
pub fn read(path: &Path, spec: &SectionSpec) -> Result<Vec<Row>, ImportError> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "xlsx" | "xlsm" | "xls" | "xlsb" => read_excel(path, spec),
        "csv" => read_csv(path, spec),
        other => Err(ImportError::UnrecognizedFormat(other.to_owned())),
    }
}

/// An open spreadsheet. Opening parses the file's shared structures once, so
/// reading many sections out of one workbook — as `gst upload` does — costs
/// one parse, not one per section.
pub struct Workbook {
    sheets: calamine::Sheets<std::io::BufReader<std::fs::File>>,
}

impl Workbook {
    pub fn open(path: &Path) -> Result<Self, ImportError> {
        let sheets = calamine::open_workbook_auto(path)
            .map_err(|e| ImportError::Io(format!("cannot open {}: {e}", path.display())))?;
        Ok(Self { sheets })
    }

    /// Read one section's sheet, using the sheet name and row offsets the spec
    /// declares.
    pub fn read(&mut self, spec: &SectionSpec) -> Result<Vec<Row>, ImportError> {
        let source = spec
            .source
            .excel
            .as_ref()
            .ok_or(ImportError::UnsupportedSource("a spreadsheet"))?;

        let available: Vec<String> = self.sheets.sheet_names().to_vec();
        let range =
            self.sheets
                .worksheet_range(&source.sheet)
                .map_err(|_| ImportError::SheetMissing {
                    sheet: source.sheet.clone(),
                    available,
                })?;

        // A sheet's used range need not start at A1, so absolute row numbers
        // come from the range's own origin. 1-based, matching how the spec and
        // every error message count rows.
        let first_absolute = range.start().map(|(row, _)| row as usize).unwrap_or(0);
        collect_rows(
            spec,
            &source.sheet,
            source.header_row,
            source.first_data_row,
            range
                .rows()
                .enumerate()
                .map(|(offset, cells)| Ok((first_absolute + offset + 1, cells))),
        )
    }
}

/// Read a section from a spreadsheet, using the sheet name and row offsets the
/// spec declares.
pub fn read_excel(path: &Path, spec: &SectionSpec) -> Result<Vec<Row>, ImportError> {
    Workbook::open(path)?.read(spec)
}

/// Read a section from a section-wise CSV.
pub fn read_csv(path: &Path, spec: &SectionSpec) -> Result<Vec<Row>, ImportError> {
    let header_row = spec.source.csv.as_ref().map(|c| c.header_row).unwrap_or(1);

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .map_err(|e| ImportError::Io(format!("cannot open {}: {e}", path.display())))?;

    collect_rows(
        spec,
        &path.display().to_string(),
        header_row,
        header_row + 1,
        reader.records().enumerate().map(|(offset, record)| {
            record
                .map(|r| (offset + 1, r))
                .map_err(|e| ImportError::Io(format!("malformed CSV: {e}")))
        }),
    )
}

/// One raw line of input, whatever the format stores cells as. Blankness is
/// answered without rendering cells to text, because the official template
/// pre-formats ~20 000 empty rows per sheet.
trait RawCells {
    fn texts(&self) -> Vec<String>;
    fn is_blank(&self) -> bool;
    fn text_at(&self, i: usize) -> String;
}

impl RawCells for &[Data] {
    fn texts(&self) -> Vec<String> {
        self.iter().map(cell_text).collect()
    }

    fn is_blank(&self) -> bool {
        self.iter().all(|cell| match cell {
            Data::Empty => true,
            Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.trim().is_empty(),
            _ => false,
        })
    }

    fn text_at(&self, i: usize) -> String {
        self.get(i).map(cell_text).unwrap_or_default()
    }
}

impl RawCells for csv::StringRecord {
    fn texts(&self) -> Vec<String> {
        self.iter().map(|c| c.trim().to_owned()).collect()
    }

    fn is_blank(&self) -> bool {
        self.iter().all(|c| c.trim().is_empty())
    }

    fn text_at(&self, i: usize) -> String {
        self.get(i).unwrap_or("").trim().to_owned()
    }
}

/// The spec's fields resolved to the column positions their headers occupy —
/// computed once when the header row is seen, so building each row is a
/// direct index rather than a per-field header scan.
struct HeaderMap<'s> {
    fields: Vec<(&'s Field, usize)>,
    /// Columns the spec requires that the header does not have.
    missing: Vec<String>,
}

impl<'s> HeaderMap<'s> {
    fn new(spec: &'s SectionSpec, header: &[String]) -> Self {
        let mut fields = Vec::new();
        let mut missing = Vec::new();
        for field in &spec.fields {
            match header.iter().position(|h| h == &field.column) {
                Some(index) => fields.push((field, index)),
                None => missing.push(field.column.clone()),
            }
        }
        Self { fields, missing }
    }

    /// Build a row keyed by the spec's column names, pulling each from
    /// wherever the header puts it. Columns the spec does not declare are
    /// dropped.
    fn build_row(&self, sheet_row: usize, value_at: impl Fn(usize) -> String) -> Row {
        let mut row = Row::new(sheet_row);
        for (field, index) in &self.fields {
            row.cells.insert(field.column.clone(), value_at(*index));
        }
        row
    }
}

/// The reader loop both formats share: capture the header row, skip everything
/// before the first data row, skip blank lines, and key each remaining line's
/// cells by the spec's columns.
fn collect_rows<C: RawCells>(
    spec: &SectionSpec,
    sheet: &str,
    header_row: usize,
    first_data_row: usize,
    lines: impl Iterator<Item = Result<(usize, C), ImportError>>,
) -> Result<Vec<Row>, ImportError> {
    let mut header: Option<HeaderMap> = None;
    let mut rows = Vec::new();

    for line in lines {
        let (sheet_row, cells) = line?;
        if sheet_row == header_row {
            header = Some(HeaderMap::new(spec, &cells.texts()));
            continue;
        }
        if sheet_row < first_data_row {
            continue;
        }
        let Some(header) = header.as_ref() else {
            continue;
        };
        // Blank rows are not data and must not be reported as invalid.
        if cells.is_blank() {
            continue;
        }
        rows.push(header.build_row(sheet_row, |i| cells.text_at(i)));
    }

    let Some(header) = header else {
        return Err(ImportError::HeaderRowMissing {
            sheet: sheet.to_owned(),
            header_row,
        });
    };
    if header.missing.is_empty() {
        Ok(rows)
    } else {
        Err(ImportError::MissingColumns(header.missing.clone()))
    }
}

/// Render a spreadsheet cell as the text validation expects.
///
/// Dates are deliberately left as their serial number: the validator already
/// accepts serials and owns the conversion, so the calendar rules live in one
/// place rather than being split across the reader.
fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.trim().to_owned(),
        Data::Int(i) => i.to_string(),
        // Rust prints f64 without a trailing '.0' and never in scientific
        // notation, so 50000.0 renders as '50000' and 0.25 as '0.25'.
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Data::DateTime(d) => d.as_f64().to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.trim().to_owned(),
        Data::Error(e) => format!("#{e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::GSTR1_B2B;

    /// Repo-root `fixtures/gstr1/`, reached from this crate's directory.
    fn fixtures() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/gstr1")
            .canonicalize()
            .expect("fixtures directory exists")
    }

    #[test]
    fn reads_the_authored_sample_csv() {
        let rows = read(&fixtures().join("b2b-sample.csv"), &GSTR1_B2B).expect("reads");
        assert_eq!(rows.len(), 7);

        // Header is line 1, so the first record is line 2.
        assert_eq!(rows[0].sheet_row, 2);
        assert_eq!(rows[0].get("Invoice Number"), Some("INV-001"));
        assert_eq!(rows[0].get("Place Of Supply"), Some("27-Maharashtra"));
        // Blank cells are present but empty, distinguishing them from a
        // missing column.
        assert_eq!(rows[0].get("Cess Amount"), Some(""));
        assert_eq!(rows[6].sheet_row, 8);
    }

    #[test]
    fn reads_gstns_sample_csv() {
        let rows = read(&fixtures().join("b2b-gstn-sample.csv"), &GSTR1_B2B).expect("reads");
        assert_eq!(rows.len(), 15);
        assert_eq!(rows[0].get("Invoice Number"), Some("1000"));
    }

    #[test]
    fn every_declared_column_is_present_in_each_row() {
        let rows = read(&fixtures().join("b2b-sample.csv"), &GSTR1_B2B).expect("reads");
        for row in &rows {
            for field in &GSTR1_B2B.fields {
                assert!(
                    row.has_column(&field.column),
                    "row {} lacks '{}'",
                    row.sheet_row,
                    field.column
                );
            }
        }
    }

    #[test]
    fn a_missing_column_fails_the_import() {
        let dir = std::env::temp_dir().join("gst-import-missing-col");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("short.csv");
        // Everything except 'Cess Amount'.
        std::fs::write(
            &path,
            "GSTIN/UIN of Recipient,Receiver Name,Invoice Number,Invoice date,Invoice Value,\
             Place Of Supply,Reverse Charge,Applicable % of Tax Rate,Invoice Type,\
             E-Commerce GSTIN,Rate,Taxable Value\n\
             12GEOPS0823BBZH,A,INV-1,14-Jul-17,118000,27-Maharashtra,N,,Regular B2B,,18,100000\n",
        )
        .unwrap();

        match read(&path, &GSTR1_B2B) {
            Err(ImportError::MissingColumns(cols)) => assert_eq!(cols, ["Cess Amount"]),
            other => panic!("expected a missing-column error, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unrecognized_extensions_are_rejected() {
        match read(std::path::Path::new("workbook.txt"), &GSTR1_B2B) {
            Err(ImportError::UnrecognizedFormat(ext)) => assert_eq!(ext, "txt"),
            other => panic!("expected a format error, got {other:?}"),
        }
    }

    /// Build a workbook laid out like the official template: a title block
    /// above the headers, headers on row 4, data from row 5.
    fn write_workbook(path: &Path, data: &[[&str; 13]]) {
        use rust_xlsxwriter::Workbook;

        let mut workbook = Workbook::new();
        let sheet = workbook.add_worksheet();
        let source = GSTR1_B2B.source.excel.as_ref().unwrap();
        sheet.set_name(&source.sheet).unwrap();

        // The template's rows 1-3 are a summary block, not data.
        sheet
            .write_string(0, 0, "Summary For B2B, SEZ, DE")
            .unwrap();
        sheet.write_string(1, 0, "No. of Recipients").unwrap();
        sheet.write_number(2, 0, 0).unwrap();

        let header_row = (source.header_row - 1) as u32;
        for (col, name) in GSTR1_B2B.columns().iter().enumerate() {
            sheet.write_string(header_row, col as u16, *name).unwrap();
        }

        let mut row = (source.first_data_row - 1) as u32;
        for cells in data {
            for (col, value) in cells.iter().enumerate() {
                if value.is_empty() {
                    continue;
                }
                // Write numbers as numbers, the way a spreadsheet really
                // stores them, so the reader's float handling is exercised.
                match value.parse::<f64>() {
                    Ok(n) => sheet.write_number(row, col as u16, n).unwrap(),
                    Err(_) => sheet.write_string(row, col as u16, *value).unwrap(),
                };
            }
            row += 1;
        }
        workbook.save(path).unwrap();
    }

    #[test]
    fn reads_a_spreadsheet_using_the_specs_sheet_and_row_offsets() {
        let dir = std::env::temp_dir().join("gst-import-xlsx");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b2b.xlsx");

        write_workbook(
            &path,
            &[
                [
                    "12GEOPS0823BBZH",
                    "Acme Traders",
                    "INV-001",
                    "14-Jul-17",
                    "118000",
                    "27-Maharashtra",
                    "N",
                    "",
                    "Regular B2B",
                    "",
                    "18",
                    "100000",
                    "",
                ],
                [
                    "29AAGCB7383J1Z4",
                    "Bharat Supplies",
                    "INV-002",
                    "20-Jul-17",
                    "23600",
                    "29-Karnataka",
                    "N",
                    "",
                    "Deemed Exp",
                    "",
                    "18",
                    "20000",
                    "500",
                ],
            ],
        );

        let rows = read(&path, &GSTR1_B2B).expect("reads");
        assert_eq!(rows.len(), 2);

        // Data starts on row 5, which is what findings must quote.
        assert_eq!(rows[0].sheet_row, 5);
        assert_eq!(rows[1].sheet_row, 6);
        assert_eq!(rows[0].get("Invoice Number"), Some("INV-001"));
        // Stored as a number, read back without a spurious decimal.
        assert_eq!(rows[0].get("Invoice Value"), Some("118000"));
        assert_eq!(rows[0].get("Rate"), Some("18"));
        assert_eq!(rows[1].get("Cess Amount"), Some("500"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_spreadsheet_row_survives_validation_and_generation() {
        use crate::date::ReturnPeriod;
        use crate::generate::generate;
        use crate::validate::{FilingContext, validate};

        let dir = std::env::temp_dir().join("gst-import-xlsx-e2e");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b2b.xlsx");
        write_workbook(
            &path,
            &[[
                "12GEOPS0823BBZH",
                "Acme Traders",
                "INV-001",
                "14-Jul-17",
                "118000",
                "27-Maharashtra",
                "N",
                "",
                "Regular B2B",
                "",
                "18",
                "100000",
                "",
            ]],
        );

        let ctx = FilingContext {
            supplier_gstin: "27AAPFU0939F1ZV".into(),
            period: ReturnPeriod::new(7, 2017).unwrap(),
            is_sez: false,
            aato_over_5cr: false,
            is_quarterly: false,
        };
        let rows = read(&path, &GSTR1_B2B).expect("reads");
        let report = validate(&GSTR1_B2B, &rows, &ctx);
        assert!(report.is_clean(), "{:?}", report.findings);

        let json = generate(&GSTR1_B2B, &report.records, &ctx).to_json();
        // Maharashtra to Maharashtra, so central and state tax.
        assert!(json.contains(r#""camt":9000,"samt":9000"#), "{json}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn blank_and_missing_sheets_are_distinguished() {
        let dir = std::env::temp_dir().join("gst-import-xlsx-blank");
        std::fs::create_dir_all(&dir).unwrap();

        // Headers but no data: the official template ships this way, with
        // thousands of pre-formatted empty rows.
        let empty = dir.join("empty.xlsx");
        write_workbook(&empty, &[]);
        assert!(read(&empty, &GSTR1_B2B).expect("reads").is_empty());

        // A workbook whose sheet is named something else.
        let wrong = dir.join("wrong.xlsx");
        let mut workbook = rust_xlsxwriter::Workbook::new();
        workbook.add_worksheet().set_name("Sheet1").unwrap();
        workbook.save(&wrong).unwrap();
        match read(&wrong, &GSTR1_B2B) {
            Err(ImportError::SheetMissing { sheet, available }) => {
                assert_eq!(sheet, "b2b,sez,de");
                assert_eq!(available, ["Sheet1"]);
            }
            other => panic!("expected a missing-sheet error, got {other:?}"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn floats_render_without_a_trailing_zero() {
        // Spreadsheets store every number as a float; '50000.0' would still
        // satisfy the value pattern, but the rate enum needs '0.25' exactly.
        assert_eq!(cell_text(&Data::Float(50000.0)), "50000");
        assert_eq!(cell_text(&Data::Float(0.25)), "0.25");
        assert_eq!(cell_text(&Data::Float(42930.0)), "42930");
        assert_eq!(cell_text(&Data::Int(18)), "18");
        assert_eq!(cell_text(&Data::Empty), "");
        assert_eq!(cell_text(&Data::String("  spaced  ".into())), "spaced");
    }
}
