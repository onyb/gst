//! The upload payload, and how it is written out.
//!
//! Money is carried as [`Decimal`] all the way to the serialized bytes.
//! Routing it through `serde_json`'s f64-backed numbers would make output
//! formatting depend on floating-point printing (`45000` becoming `45000.0`),
//! which matters because generated JSON is compared against reference output.
//! A decimal renders here exactly as the reference does: normalized, with
//! trailing zeros dropped.

use std::fmt::Write as _;

use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(Decimal),
    Str(String),
    Arr(Vec<Json>),
    /// Key order is preserved as inserted, which is spec order.
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Self {
        Json::Obj(Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Json::Null => true,
            Json::Str(s) => s.is_empty(),
            Json::Arr(a) => a.is_empty(),
            Json::Obj(o) => o.is_empty(),
            _ => false,
        }
    }

    /// Insert at a dotted path, creating intermediate objects as needed, so a
    /// spec key like `itm_det.txval` nests without the caller doing the work.
    pub fn insert_path(&mut self, path: &str, value: Json) {
        let Json::Obj(entries) = self else { return };
        match path.split_once('.') {
            None => {
                if let Some(slot) = entries.iter_mut().find(|(k, _)| k == path) {
                    slot.1 = value;
                } else {
                    entries.push((path.to_owned(), value));
                }
            }
            Some((head, rest)) => {
                if !entries.iter().any(|(k, _)| k == head) {
                    entries.push((head.to_owned(), Json::obj()));
                }
                let slot = entries
                    .iter_mut()
                    .find(|(k, _)| k == head)
                    .expect("just inserted");
                slot.1.insert_path(rest, value);
            }
        }
    }

    pub fn get(&self, path: &str) -> Option<&Json> {
        let Json::Obj(entries) = self else {
            return None;
        };
        match path.split_once('.') {
            None => entries.iter().find(|(k, _)| k == path).map(|(_, v)| v),
            Some((head, rest)) => entries
                .iter()
                .find(|(k, _)| k == head)
                .and_then(|(_, v)| v.get(rest)),
        }
    }

    /// Compact JSON, as the portal expects.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            // normalize() drops trailing zeros, matching how the reference
            // implementation's numbers serialize.
            Json::Num(d) => {
                let _ = write!(out, "{}", d.normalize());
            }
            Json::Str(s) => write_string(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(entries) => {
                out.push('{');
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_render_without_trailing_zeros() {
        assert_eq!(Json::Num(Decimal::new(4500000, 2)).to_json(), "45000");
        assert_eq!(Json::Num(Decimal::new(810050, 2)).to_json(), "8100.5");
        assert_eq!(Json::Num(Decimal::new(810055, 2)).to_json(), "8100.55");
        assert_eq!(Json::Num(Decimal::ZERO).to_json(), "0");
    }

    #[test]
    fn dotted_paths_nest() {
        let mut o = Json::obj();
        o.insert_path("num", Json::Num(Decimal::from(1801)));
        o.insert_path("itm_det.txval", Json::Num(Decimal::from(45000)));
        o.insert_path("itm_det.rt", Json::Num(Decimal::from(18)));
        assert_eq!(
            o.to_json(),
            r#"{"num":1801,"itm_det":{"txval":45000,"rt":18}}"#
        );
        assert_eq!(o.get("itm_det.rt"), Some(&Json::Num(Decimal::from(18))));
    }

    #[test]
    fn insert_order_is_preserved_and_reinsertion_replaces() {
        let mut o = Json::obj();
        o.insert_path("b", Json::Str("1".into()));
        o.insert_path("a", Json::Str("2".into()));
        o.insert_path("b", Json::Str("3".into()));
        assert_eq!(o.to_json(), r#"{"b":"3","a":"2"}"#);
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(Json::Str("a\"b\\c".into()).to_json(), r#""a\"b\\c""#);
        assert_eq!(Json::Str("line\n".into()).to_json(), r#""line\n""#);
        // Control characters escape as \uXXXX rather than passing through.
        assert_eq!(Json::Str("\u{1}".into()).to_json(), r#""\u0001""#);
    }

    #[test]
    fn emptiness_covers_the_omit_when_empty_cases() {
        assert!(Json::Str(String::new()).is_empty());
        assert!(Json::Null.is_empty());
        assert!(Json::Arr(vec![]).is_empty());
        assert!(!Json::Num(Decimal::ZERO).is_empty());
        assert!(!Json::Str("x".into()).is_empty());
    }
}
