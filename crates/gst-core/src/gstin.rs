//! GST registration numbers.
//!
//! Four 15-character forms appear as a counterparty id — a regular GSTIN, a
//! UN body's UIN, a TDS deductor id, and a non-resident taxable person id.
//! All four share the same check digit, so validation is a shape test
//! followed by the checksum.

use std::sync::LazyLock;

use regex::Regex;

use crate::spec::GstinForm;

const CODE_POINTS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Patterns are anchored here. The reference implementation leaves them
/// unanchored and relies on the checksum step to reject anything longer,
/// which is the same outcome by a less obvious route.
macro_rules! pattern {
    ($name:ident, $re:literal) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($re).expect("valid pattern"));
    };
}

pattern!(
    GSTIN,
    r"^[0-9]{2}[a-zA-Z]{5}[0-9]{4}[a-zA-Z][1-9A-Za-z][Zz1-9A-Ja-j][0-9a-zA-Z]$"
);
pattern!(UIN, r"^[0-9]{4}[A-Z]{3}[0-9]{5}[UO]N[A-Z0-9]$");
pattern!(
    TDS,
    r"^[0-9]{2}[a-zA-Z]{4}[a-zA-Z0-9][0-9]{4}[a-zA-Z][1-9A-Za-z]D[0-9a-zA-Z]$"
);
pattern!(NRTP, r"^[0-9]{4}[a-zA-Z]{3}[0-9]{5}NR[0-9a-zA-Z]$");
pattern!(
    ECO,
    r"^[0-9]{2}[a-zA-Z]{5}[0-9]{4}[a-zA-Z][1-9A-Za-z]C[0-9a-zA-Z]$"
);

/// The check digit for a registration number's first 14 characters.
///
/// Weighted mod-36 over the alphabet `0-9A-Z`, alternating factors of 2 and 1
/// from the right, summing each product's quotient and remainder.
pub fn check_digit(first_14: &str) -> Option<char> {
    let modulus = CODE_POINTS.len();
    let mut factor = 2usize;
    let mut sum = 0usize;
    for ch in first_14.trim().to_ascii_uppercase().bytes().rev() {
        let point = CODE_POINTS.iter().position(|&c| c == ch)?;
        let product = factor * point;
        factor = if factor == 2 { 1 } else { 2 };
        sum += product / modulus + product % modulus;
    }
    Some(CODE_POINTS[(modulus - sum % modulus) % modulus] as char)
}

/// Whether a registration number carries a correct check digit.
pub fn checksum_valid(id: &str) -> bool {
    if id.len() != 15 || !id.is_ascii() {
        return false;
    }
    let (body, given) = id.split_at(14);
    // Compared as written, NOT case-folded. The reference tests
    // `gst === checkGstn(gst.substr(0, 14))`, and `checkGstn` appends a digit
    // taken from an uppercase alphabet, so a lowercase check character can
    // never match however correct its value. The shape patterns admit one, so
    // this is the only thing standing between `12geops0823bbzh` and acceptance.
    check_digit(body).is_some_and(|expected| given.starts_with(expected))
}

/// Whether `id` matches the shape of any accepted form. Shape only — callers
/// still need [`checksum_valid`].
pub fn matches_any_form(id: &str, accepts: &[GstinForm]) -> bool {
    accepts.iter().any(|form| {
        let re: &Regex = match form {
            GstinForm::Gstin => &GSTIN,
            GstinForm::Uin => &UIN,
            GstinForm::Tds => &TDS,
            GstinForm::Nrtp => &NRTP,
            GstinForm::Eco => &ECO,
        };
        re.is_match(id)
    })
}

/// The state code every registration number opens with.
pub fn state_code(id: &str) -> Option<&str> {
    id.get(..2)
        .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_good_numbers() {
        // The first is GSTN's own sample from the official section CSV.
        for id in ["12GEOPS0823BBZH", "27AAPFU0939F1ZV", "29AAGCB7383J1Z4"] {
            assert!(checksum_valid(id), "{id} should pass its check digit");
            assert!(matches_any_form(id, &[GstinForm::Gstin]), "{id} shape");
        }
    }

    #[test]
    fn rejects_a_corrupted_check_digit() {
        assert!(!checksum_valid("12GEOPS0823BBZA"));
        // Right shape, wrong length.
        assert!(!checksum_valid("12GEOPS0823BBZHX"));
        assert!(!checksum_valid(""));
    }

    #[test]
    fn rejects_a_valid_number_embedded_in_a_longer_string() {
        // Anchoring matters: the reference patterns would match the substring.
        assert!(!matches_any_form("XX12GEOPS0823BBZH", &[GstinForm::Gstin]));
        assert!(!matches_any_form("12GEOPS0823BBZHYY", &[GstinForm::Gstin]));
    }

    #[test]
    fn eco_form_matches_the_c_marker() {
        // An operator GSTIN carries 'C' in the 13th position where a regular
        // one carries 'Z'.
        assert!(matches_any_form("12AJIPA1572E1C7", &[GstinForm::Eco]));
    }

    #[test]
    fn the_general_gstin_shape_does_not_exclude_operator_numbers() {
        // The 13th character class is broad — `[Zz1-9A-Ja-j]` covers 'C' — so
        // the general pattern admits an operator GSTIN too. The forms overlap
        // by design and are not a partition; anything that needs to tell an
        // operator number apart must test the ECO form specifically.
        assert!(matches_any_form("12AJIPA1572E1C7", &[GstinForm::Gstin]));
        assert!(!matches_any_form("12AAPFU0939F1ZV", &[GstinForm::Eco]));
    }

    #[test]
    fn state_code_is_the_leading_pair() {
        assert_eq!(state_code("27AAPFU0939F1ZV"), Some("27"));
        assert_eq!(state_code("XXAAPFU0939F1ZV"), None);
    }
}
