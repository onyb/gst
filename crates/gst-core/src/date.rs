//! Invoice date parsing and the return-period window.
//!
//! Workbook cells reach us either as text in one of four accepted layouts or
//! as an Excel date serial. Both normalize to `DD-MM-YYYY`, which is the form
//! the upload payload carries.

use chrono::{Datelike, NaiveDate};

/// Excel's day 1 is 1900-01-01, but its calendar wrongly contains 1900-02-29,
/// so serials line up with the real calendar only when counted from
/// 1899-12-30.
const EXCEL_EPOCH: (i32, u32, u32) = (1899, 12, 30);

/// GST came into force on 1 July 2017; nothing earlier can be reported.
const GST_START: (i32, u32, u32) = (2017, 7, 1);

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateError {
    /// Not one of the accepted layouts.
    Malformed,
    /// Well-formed but not a real calendar date, e.g. 31 February.
    NotACalendarDate,
    /// Before 1 July 2017.
    BeforeGst,
    /// After the last day of the return period being filed.
    AfterReturnPeriod,
}

/// The period a return is being filed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnPeriod {
    pub month: u32,
    pub year: i32,
}

impl ReturnPeriod {
    pub fn new(month: u32, year: i32) -> Option<Self> {
        (1..=12).contains(&month).then_some(Self { month, year })
    }

    /// Parse the `MMYYYY` form the portal and the official tool both use for a
    /// return period, e.g. `072017`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != 6 || !text.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Self::new(text[..2].parse().ok()?, text[2..].parse().ok()?)
    }

    /// The period as comparable `YYYYMM`, which is how period-scoped spec
    /// constraints are ordered.
    pub fn as_yyyymm(&self) -> u32 {
        self.year as u32 * 100 + self.month
    }

    /// The period as `MMYYYY`, the form the portal and the upload payload use.
    pub fn as_mmyyyy(&self) -> String {
        format!("{:02}{:04}", self.month, self.year)
    }

    /// Last day of the period — the latest date any document may carry.
    pub fn last_day(&self) -> NaiveDate {
        let (next_month, next_year) = if self.month == 12 {
            (1, self.year + 1)
        } else {
            (self.month + 1, self.year)
        };
        NaiveDate::from_ymd_opt(next_year, next_month, 1)
            .expect("first of month is always valid")
            .pred_opt()
            .expect("a first-of-month always has a predecessor")
    }
}

/// Parse a date cell held as text.
///
/// Accepts `D-MMM-YY`, `D-MMM-YYYY`, `DD-MMM-YY` and `DD-MMM-YYYY`, with the
/// month abbreviated in any case. Two-digit years are read as 20YY.
///
/// Unlike the reference implementation, an unrecognized month is an error
/// rather than silently becoming December — see the `quirk` note on `idt` in
/// the B2B spec.
pub fn parse_text(cell: &str) -> Result<NaiveDate, DateError> {
    let cell = cell.trim();
    let mut parts = cell.split('-');
    let (Some(day), Some(month), Some(year), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(DateError::Malformed);
    };

    if day.is_empty() || day.len() > 2 || !day.chars().all(|c| c.is_ascii_digit()) {
        return Err(DateError::Malformed);
    }
    let day: u32 = day.parse().map_err(|_| DateError::Malformed)?;

    let month_lower = month.to_ascii_lowercase();
    let month = MONTHS
        .iter()
        .position(|m| *m == month_lower)
        .ok_or(DateError::Malformed)? as u32
        + 1;

    let year: i32 = match year.len() {
        2 if year.chars().all(|c| c.is_ascii_digit()) => {
            2000 + year.parse::<i32>().map_err(|_| DateError::Malformed)?
        }
        4 if year.chars().all(|c| c.is_ascii_digit()) => {
            year.parse().map_err(|_| DateError::Malformed)?
        }
        _ => return Err(DateError::Malformed),
    };

    NaiveDate::from_ymd_opt(year, month, day).ok_or(DateError::NotACalendarDate)
}

/// Convert an Excel date serial to a calendar date.
pub fn from_excel_serial(serial: i64) -> Result<NaiveDate, DateError> {
    let (y, m, d) = EXCEL_EPOCH;
    NaiveDate::from_ymd_opt(y, m, d)
        .expect("epoch is valid")
        .checked_add_signed(chrono::TimeDelta::days(serial))
        .ok_or(DateError::Malformed)
}

/// Check a parsed date against the reportable window: on or after 1 July 2017
/// and no later than the end of the period being filed.
pub fn check_window(date: NaiveDate, period: ReturnPeriod) -> Result<(), DateError> {
    let (y, m, d) = GST_START;
    let start = NaiveDate::from_ymd_opt(y, m, d).expect("GST start is valid");
    if date < start {
        return Err(DateError::BeforeGst);
    }
    if date > period.last_day() {
        return Err(DateError::AfterReturnPeriod);
    }
    Ok(())
}

/// Month number from a full month name, case-insensitively.
///
/// Amendment tables identify the period being corrected by a spelled-out month
/// plus a financial year rather than a date.
pub fn month_number(name: &str) -> Option<u32> {
    const FULL: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let name = name.trim().to_ascii_lowercase();
    FULL.iter().position(|m| *m == name).map(|i| i as u32 + 1)
}

/// The `MMYYYY` period a financial year and month name identify.
///
/// Indian financial years run April to March, so JANUARY of `2017-18` is
/// January 2018, not 2017 — which is why amendment tables need both columns to
/// pin down a single period.
pub fn period_from_financial_year(fy: &str, month_name: &str) -> Option<String> {
    let month = month_number(month_name)?;
    let start_year: i32 = fy.trim().get(..4)?.parse().ok()?;
    let year = if month >= 4 {
        start_year
    } else {
        start_year + 1
    };
    Some(ReturnPeriod { month, year }.as_mmyyyy())
}

/// Render as `DD-MM-YYYY`, the form the upload payload carries.
pub fn normalize(date: NaiveDate) -> String {
    format!("{:02}-{:02}-{:04}", date.day(), date.month(), date.year())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn accepts_all_four_layouts() {
        // The official sample CSV uses the two-digit-year form.
        assert_eq!(parse_text("14-Jul-17"), Ok(ymd(2017, 7, 14)));
        assert_eq!(parse_text("14-Jul-2017"), Ok(ymd(2017, 7, 14)));
        assert_eq!(parse_text("4-Jul-17"), Ok(ymd(2017, 7, 4)));
        assert_eq!(parse_text("4-Jul-2017"), Ok(ymd(2017, 7, 4)));
        // Month case is not significant.
        assert_eq!(parse_text("14-JUL-17"), Ok(ymd(2017, 7, 14)));
        assert_eq!(parse_text("14-jul-17"), Ok(ymd(2017, 7, 14)));
    }

    #[test]
    fn rejects_an_unrecognized_month_instead_of_defaulting_to_december() {
        // A deliberate divergence: the reference implementation returns
        // December here, silently filing the invoice in the wrong month.
        assert_eq!(parse_text("14-Xyz-17"), Err(DateError::Malformed));
        assert_eq!(parse_text("14-13-17"), Err(DateError::Malformed));
    }

    #[test]
    fn rejects_malformed_and_impossible_dates() {
        assert_eq!(parse_text("2017-07-14"), Err(DateError::Malformed));
        assert_eq!(parse_text("14/Jul/17"), Err(DateError::Malformed));
        assert_eq!(parse_text("14-Jul"), Err(DateError::Malformed));
        assert_eq!(parse_text("14-Jul-17-1"), Err(DateError::Malformed));
        assert_eq!(parse_text("31-Feb-20"), Err(DateError::NotACalendarDate));
    }

    #[test]
    fn excel_serials_line_up_with_the_real_calendar() {
        // 1 January 2020 is serial 43831 in Excel's numbering.
        assert_eq!(from_excel_serial(43831), Ok(ymd(2020, 1, 1)));
        assert_eq!(from_excel_serial(42930), Ok(ymd(2017, 7, 14)));
    }

    #[test]
    fn window_spans_gst_start_to_end_of_period() {
        let period = ReturnPeriod::new(7, 2017).unwrap();
        assert_eq!(period.last_day(), ymd(2017, 7, 31));
        assert_eq!(check_window(ymd(2017, 7, 14), period), Ok(()));
        assert_eq!(check_window(ymd(2017, 7, 31), period), Ok(()));
        assert_eq!(
            check_window(ymd(2017, 6, 30), period),
            Err(DateError::BeforeGst)
        );
        assert_eq!(
            check_window(ymd(2017, 8, 1), period),
            Err(DateError::AfterReturnPeriod)
        );
    }

    #[test]
    fn december_period_rolls_the_year() {
        let period = ReturnPeriod::new(12, 2024).unwrap();
        assert_eq!(period.last_day(), ymd(2024, 12, 31));
        assert!(check_window(ymd(2025, 1, 1), period).is_err());
    }

    #[test]
    fn february_period_respects_leap_years() {
        assert_eq!(
            ReturnPeriod::new(2, 2024).unwrap().last_day(),
            ymd(2024, 2, 29)
        );
        assert_eq!(
            ReturnPeriod::new(2, 2023).unwrap().last_day(),
            ymd(2023, 2, 28)
        );
    }

    #[test]
    fn financial_year_and_month_pin_down_a_period() {
        // April to March: July belongs to the opening calendar year, January to
        // the closing one.
        assert_eq!(
            period_from_financial_year("2017-18", "JULY").as_deref(),
            Some("072017")
        );
        assert_eq!(
            period_from_financial_year("2017-18", "JANUARY").as_deref(),
            Some("012018")
        );
        assert_eq!(
            period_from_financial_year("2017-18", "MARCH").as_deref(),
            Some("032018")
        );
        assert_eq!(
            period_from_financial_year("2017-18", "APRIL").as_deref(),
            Some("042017")
        );
        // Case does not matter to the lookup itself.
        assert_eq!(
            period_from_financial_year("2024-25", "december").as_deref(),
            Some("122024")
        );
        assert_eq!(period_from_financial_year("2017-18", "SMARCH"), None);
        assert_eq!(period_from_financial_year("bad", "JULY"), None);
    }

    #[test]
    fn month_numbers_come_from_full_names() {
        assert_eq!(month_number("JANUARY"), Some(1));
        assert_eq!(month_number("December"), Some(12));
        assert_eq!(month_number("  july  "), Some(7));
        assert_eq!(month_number("Jul"), None);
        assert_eq!(month_number(""), None);
    }

    #[test]
    fn parses_the_mmyyyy_period_form() {
        assert_eq!(ReturnPeriod::parse("072017"), ReturnPeriod::new(7, 2017));
        assert_eq!(ReturnPeriod::parse("122024"), ReturnPeriod::new(12, 2024));
        assert_eq!(ReturnPeriod::parse(" 072017 "), ReturnPeriod::new(7, 2017));

        // Month must be real, and the shape is exactly six digits.
        assert_eq!(ReturnPeriod::parse("132017"), None);
        assert_eq!(ReturnPeriod::parse("002017"), None);
        assert_eq!(ReturnPeriod::parse("72017"), None);
        assert_eq!(ReturnPeriod::parse("07-2017"), None);
        assert_eq!(ReturnPeriod::parse("0720177"), None);
        assert_eq!(ReturnPeriod::parse("abcdef"), None);
    }

    #[test]
    fn normalizes_to_the_payload_form() {
        assert_eq!(normalize(ymd(2017, 7, 4)), "04-07-2017");
        assert_eq!(normalize(ymd(2024, 12, 31)), "31-12-2024");
    }
}
