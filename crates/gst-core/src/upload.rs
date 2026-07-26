//! Assembling a complete GSTR-1 upload file.
//!
//! Individual sections generate their own payloads; this wraps them in the
//! envelope the portal expects. The key order, the empty-value shapes, and the
//! literal `version`/`hash` values all come from
//! `spec/gstr1/upload-envelope.json`, so the wrapper is spec-driven like
//! everything else.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::payload::Json;
use crate::spec::period_as_yyyymm;
use crate::validate::FilingContext;

#[derive(Debug, Clone, Deserialize)]
struct EnvelopeKey {
    key: String,
    from: String,
    #[serde(default)]
    wrapper: Option<String>,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Wrapped {
    wrapper: Option<String>,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Chunking {
    max_bytes: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct Filename {
    pattern: String,
}

#[derive(Debug, Clone, Deserialize)]
struct EnvelopeSpec {
    version: String,
    hash: String,
    hsn_bifurcation_start_period: String,
    filename: Filename,
    chunking: Chunking,
    keys: Vec<EnvelopeKey>,
    hsn_before_bifurcation: Wrapped,
    hsn_from_bifurcation: Wrapped,
}

static ENVELOPE: LazyLock<EnvelopeSpec> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../spec/gstr1/upload-envelope.json"))
        .expect("embedded spec gstr1/upload-envelope.json is invalid")
});

/// Turnover figures, which only some periods carry.
#[derive(Debug, Clone, Copy, Default)]
pub struct Turnover {
    pub gross: Option<Decimal>,
    pub current: Option<Decimal>,
}

/// The size beyond which the reference splits an upload into chunks.
pub fn max_chunk_bytes() -> usize {
    ENVELOPE.chunking.max_bytes
}

/// The filename the reference writes, e.g. `returns_072017_R1_27AAA…_offline.json`.
pub fn filename(ctx: &FilingContext) -> String {
    ENVELOPE
        .filename
        .pattern
        .replace("{fp}", &period_string(ctx))
        .replace("{gstin}", &ctx.supplier_gstin)
}

fn period_string(ctx: &FilingContext) -> String {
    format!("{:02}{:04}", ctx.period.month, ctx.period.year)
}

/// Build the complete upload file.
///
/// `sections` maps a section code to the envelopes that section generated.
/// Sections the caller has nothing for are still emitted, empty, because the
/// reference always writes the full key set.
pub fn build(
    sections: &HashMap<String, Vec<Json>>,
    ctx: &FilingContext,
    turnover: Turnover,
) -> Json {
    let spec = &*ENVELOPE;
    let bifurcated = period_as_yyyymm(&spec.hsn_bifurcation_start_period)
        .is_some_and(|start| ctx.period.as_yyyymm() >= start);

    let take = |code: &str| -> Vec<Json> { sections.get(code).cloned().unwrap_or_default() };

    let mut out = Json::obj();
    for entry in &spec.keys {
        let value = match entry.from.as_str() {
            "context:gstin" => Json::Str(ctx.supplier_gstin.clone()),
            "context:period" => Json::Str(period_string(ctx)),
            "context:gross_turnover" => match turnover.gross {
                Some(v) => Json::Num(v),
                None => continue,
            },
            "context:current_gross_turnover" => match turnover.current {
                Some(v) => Json::Num(v),
                None => continue,
            },
            "literal:version" => Json::Str(spec.version.clone()),
            "literal:hash" => Json::Str(spec.hash.clone()),
            "empty:array" => Json::Arr(Vec::new()),
            "hsn" => {
                let mut hsn = Json::obj();
                if bifurcated {
                    for member in &spec.hsn_from_bifurcation.members {
                        // 'hsn_b2b' -> section 'hsn(b2b)'
                        let code = member.replace("hsn_", "hsn(") + ")";
                        hsn.insert_path(member, Json::Arr(take(&code)));
                    }
                } else if let Some(wrapper) = &spec.hsn_before_bifurcation.wrapper {
                    hsn.insert_path(wrapper, Json::Arr(take("hsn")));
                }
                hsn
            }
            "object" => {
                let mut obj = Json::obj();
                for member in &entry.members {
                    obj.insert_path(member, Json::Arr(Vec::new()));
                }
                obj
            }
            other => {
                if let Some(code) = other.strip_prefix("section:") {
                    Json::Arr(take(code))
                } else if let Some(code) = other.strip_prefix("wrapped:") {
                    let wrapper = entry
                        .wrapper
                        .as_deref()
                        .expect("wrapped envelope key declares a wrapper");
                    let mut obj = Json::obj();
                    obj.insert_path(wrapper, Json::Arr(take(code)));
                    obj
                } else {
                    panic!("envelope spec names unknown source `{other}`");
                }
            }
        };
        out.insert_path(&entry.key, value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::ReturnPeriod;

    fn ctx(month: u32, year: i32) -> FilingContext {
        FilingContext {
            supplier_gstin: "27AAPFU0939F1ZV".into(),
            period: ReturnPeriod::new(month, year).unwrap(),
            is_sez: false,
        }
    }

    #[test]
    fn an_empty_return_still_carries_every_section_key() {
        let json = build(&HashMap::new(), &ctx(7, 2017), Turnover::default()).to_json();
        for key in [
            "gstin",
            "fp",
            "version",
            "hash",
            "b2b",
            "b2ba",
            "b2cl",
            "b2cla",
            "b2cs",
            "b2csa",
            "nil",
            "exp",
            "expa",
            "hsnSac",
            "cdnra",
            "at",
            "ata",
            "cdnr",
            "cdnur",
            "cdnura",
            "atadj",
            "atadja",
            "doc_issue",
            "hsn",
            "supeco",
            "supecoa",
            "ecom",
            "ecoma",
        ] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "missing {key}: {json}"
            );
        }
        // The literals are fixed by the tool release, not by the return.
        assert!(json.contains(r#""version":"GST3.2.4""#), "{json}");
        assert!(json.contains(r#""hash":"hash""#), "{json}");
        // Turnover is omitted entirely when not supplied.
        assert!(!json.contains("\"gt\""), "{json}");
        assert!(!json.contains("\"cur_gt\""), "{json}");
    }

    #[test]
    fn the_key_order_matches_the_reference() {
        let json = build(&HashMap::new(), &ctx(7, 2017), Turnover::default()).to_json();
        let order = [
            "\"gstin\"",
            "\"fp\"",
            "\"version\"",
            "\"hash\"",
            "\"b2b\"",
            "\"b2ba\"",
            "\"b2cl\"",
            "\"b2cla\"",
            "\"b2cs\"",
            "\"b2csa\"",
            "\"nil\"",
            "\"exp\"",
            "\"expa\"",
            "\"hsnSac\"",
            "\"cdnra\"",
            "\"at\"",
            "\"ata\"",
            "\"cdnr\"",
            "\"cdnur\"",
            "\"cdnura\"",
            "\"atadj\"",
            "\"atadja\"",
            "\"doc_issue\"",
            "\"hsn\"",
            "\"supeco\"",
            "\"supecoa\"",
            "\"ecom\"",
            "\"ecoma\"",
        ];
        let mut last = 0usize;
        for key in order {
            let at = json.find(key).unwrap_or_else(|| panic!("missing {key}"));
            assert!(at > last, "{key} is out of order in {json}");
            last = at;
        }
    }

    #[test]
    fn turnover_appears_right_after_the_period_when_supplied() {
        let json = build(
            &HashMap::new(),
            &ctx(7, 2017),
            Turnover {
                gross: Some(Decimal::from(5000000)),
                current: Some(Decimal::from(1200000)),
            },
        )
        .to_json();
        assert!(
            json.starts_with(
                r#"{"gstin":"27AAPFU0939F1ZV","fp":"072017","gt":5000000,"cur_gt":1200000,"version""#
            ),
            "{json}"
        );
    }

    #[test]
    fn hsn_changes_shape_at_the_bifurcation_period() {
        // Before May 2025 a single data array.
        let before = build(&HashMap::new(), &ctx(4, 2025), Turnover::default()).to_json();
        assert!(before.contains(r#""hsn":{"data":[]}"#), "{before}");

        // From May 2025 onward, split by B2B and B2C.
        let after = build(&HashMap::new(), &ctx(5, 2025), Turnover::default()).to_json();
        assert!(
            after.contains(r#""hsn":{"hsn_b2b":[],"hsn_b2c":[]}"#),
            "{after}"
        );
    }

    #[test]
    fn nil_and_doc_issue_are_wrapped_not_bare_arrays() {
        let json = build(&HashMap::new(), &ctx(7, 2017), Turnover::default()).to_json();
        assert!(json.contains(r#""nil":{"inv":[]}"#), "{json}");
        assert!(json.contains(r#""doc_issue":{"doc_det":[]}"#), "{json}");
    }

    #[test]
    fn section_payloads_land_under_their_own_key() {
        let mut sections = HashMap::new();
        sections.insert(
            "b2b".to_string(),
            vec![Json::Obj(vec![(
                "ctin".to_string(),
                Json::Str("12GEOPS0823BBZH".into()),
            )])],
        );
        let json = build(&sections, &ctx(7, 2017), Turnover::default()).to_json();
        assert!(
            json.contains(r#""b2b":[{"ctin":"12GEOPS0823BBZH"}]"#),
            "{json}"
        );
        // Untouched sections stay empty.
        assert!(json.contains(r#""b2ba":[]"#), "{json}");
    }

    #[test]
    fn the_filename_follows_the_reference_pattern() {
        assert_eq!(
            filename(&ctx(7, 2017)),
            "returns_072017_R1_27AAPFU0939F1ZV_offline.json"
        );
    }

    #[test]
    fn the_chunk_threshold_is_the_tools_actual_limit() {
        // 4.7 MiB, not the 5 MB the portal documentation mentions.
        assert_eq!(max_chunk_bytes(), 4_928_307);
    }
}
