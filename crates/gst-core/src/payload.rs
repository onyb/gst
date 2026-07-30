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

    /// Emptiness that recurses into objects: an object whose every member is
    /// itself empty counts as empty. This is what the upload envelope's
    /// omit-empty rule means by empty; a numeric 0 is NOT empty.
    pub fn is_empty_recursive(&self) -> bool {
        match self {
            Json::Obj(entries) => entries.iter().all(|(_, v)| v.is_empty_recursive()),
            _ => self.is_empty(),
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

    pub(crate) fn write(&self, out: &mut String) {
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

/// Every node reached from `root` by following `path`: a segment names an
/// object member, an array fans out into its elements, a missing member
/// contributes nothing. The empty path is the root itself.
pub(crate) fn walk<'a>(root: &'a Json, path: &[String]) -> Vec<&'a Json> {
    let mut nodes = vec![root];
    for segment in path {
        nodes = nodes
            .iter()
            .flat_map(|node| match node.get(segment) {
                Some(Json::Arr(items)) => items.iter().collect(),
                Some(other) => vec![other],
                None => Vec::new(),
            })
            .collect();
    }
    nodes
}

/// Why a text failed to parse, with the byte offset it failed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid JSON at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse JSON text into the payload AST — the writer's inverse.
///
/// Numbers become [`Decimal`] from the raw token, so money survives exactly;
/// `serde_json`'s f64-backed numbers would turn 0.65 into 0.6500000000000000222…
/// and make every money comparison a lie. Exponent forms are accepted (a
/// foreign file may carry them; this writer never emits one); magnitudes a
/// Decimal cannot hold are an error, never a silent approximation. Duplicate
/// object keys keep the last value in the first key's position, matching
/// [`Json::insert_path`].
pub fn parse(text: &str) -> Result<Json, ParseError> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(parser.err("trailing characters after the document"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            offset: self.pos,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(format!("expected '{}'", byte as char)))
        }
    }

    fn value(&mut self) -> Result<Json, ParseError> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(other) => Err(self.err(format!("unexpected character '{}'", other as char))),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn object(&mut self) -> Result<Json, ParseError> {
        self.expect(b'{')?;
        let mut obj = Json::obj();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(obj);
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value()?;
            // Last value wins, in the first occurrence's position — the same
            // semantics as insert_path. The key may contain a dot (payload
            // keys never do, but foreign input may), so insert directly.
            if let Json::Obj(entries) = &mut obj {
                if let Some(slot) = entries.iter_mut().find(|(k, _)| *k == key) {
                    slot.1 = value;
                } else {
                    entries.push((key, value));
                }
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(obj);
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self) -> Result<Json, ParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        let mut run = self.pos;
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    out.push_str(
                        std::str::from_utf8(&self.bytes[run..self.pos])
                            .expect("slice split at ASCII boundaries"),
                    );
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    out.push_str(
                        std::str::from_utf8(&self.bytes[run..self.pos])
                            .expect("slice split at ASCII boundaries"),
                    );
                    self.pos += 1;
                    let escaped = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.pos += 1;
                            let unit = self.hex4()?;
                            let c = match unit {
                                0xD800..=0xDBFF => {
                                    // A high surrogate must pair with a low one.
                                    if self.peek() == Some(b'\\') {
                                        self.pos += 1;
                                        self.expect(b'u')
                                            .map_err(|_| self.err("lone surrogate"))?;
                                        let low = self.hex4()?;
                                        if !(0xDC00..=0xDFFF).contains(&low) {
                                            return Err(self.err("lone surrogate"));
                                        }
                                        let combined =
                                            0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                                        char::from_u32(combined)
                                            .ok_or_else(|| self.err("invalid surrogate pair"))?
                                    } else {
                                        return Err(self.err("lone surrogate"));
                                    }
                                }
                                0xDC00..=0xDFFF => return Err(self.err("lone surrogate")),
                                unit => char::from_u32(unit)
                                    .ok_or_else(|| self.err("invalid \\u escape"))?,
                            };
                            out.push(c);
                            run = self.pos;
                            continue;
                        }
                        _ => return Err(self.err("invalid escape")),
                    };
                    out.push(escaped);
                    self.pos += 1;
                    run = self.pos;
                }
                Some(byte) if byte < 0x20 => {
                    return Err(self.err("unescaped control character in string"));
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Four hex digits after `\u`, as a code unit.
    fn hex4(&mut self) -> Result<u32, ParseError> {
        let end = self.pos + 4;
        let Some(hex) = self.bytes.get(self.pos..end) else {
            return Err(self.err("truncated \\u escape"));
        };
        let hex = std::str::from_utf8(hex).map_err(|_| self.err("invalid \\u escape"))?;
        let unit = u32::from_str_radix(hex, 16).map_err(|_| self.err("invalid \\u escape"))?;
        self.pos = end;
        Ok(unit)
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        use std::str::FromStr;
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let mut exponent = false;
        while let Some(byte) = self.peek() {
            match byte {
                b'0'..=b'9' | b'.' => self.pos += 1,
                b'e' | b'E' | b'+' | b'-' if self.pos > start => {
                    exponent |= matches!(byte, b'e' | b'E');
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let token = std::str::from_utf8(&self.bytes[start..self.pos]).expect("ASCII token");
        let parsed = if exponent {
            Decimal::from_scientific(token).map_err(|e| e.to_string())
        } else {
            Decimal::from_str(token).map_err(|e| e.to_string())
        };
        match parsed {
            Ok(value) => Ok(Json::Num(value)),
            Err(e) => Err(ParseError {
                offset: start,
                message: format!("number '{token}' is not representable: {e}"),
            }),
        }
    }

    fn literal(&mut self, text: &str, value: Json) -> Result<Json, ParseError> {
        if self.bytes[self.pos..].starts_with(text.as_bytes()) {
            self.pos += text.len();
            Ok(value)
        } else {
            Err(self.err(format!("expected '{text}'")))
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

    // ---- parse: the writer's inverse ----

    #[test]
    fn every_golden_file_roundtrips_byte_for_byte() {
        // Decimal fidelity, key order and escaping in one assertion per file.
        for golden in [
            "gstr1-062025-reference.json",
            "gstr1-eco-062025-reference.json",
            "gstr1-062025-meta.json",
            "gstr1-eco-062025-meta.json",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/golden")
                .join(golden);
            let text = std::fs::read_to_string(path).expect("golden present");
            assert_eq!(parse(&text).expect(golden).to_json(), text, "{golden}");
        }
    }

    #[test]
    fn numbers_parse_into_exact_decimals() {
        let exact = |s: &str| match parse(s) {
            Ok(Json::Num(d)) => d,
            other => panic!("{s}: {other:?}"),
        };
        assert_eq!(exact("0.65"), Decimal::new(65, 2));
        assert_eq!(exact("8100.55"), Decimal::new(810055, 2));
        assert_eq!(exact("-0.5"), Decimal::new(-5, 1));
        assert_eq!(exact("45000"), Decimal::from(45000));
        // Exponent forms: a foreign file may carry them; ours never does.
        assert_eq!(exact("1e5"), Decimal::from(100000));
        assert_eq!(exact("1.2E-3"), Decimal::new(12, 4));
        // Magnitudes past Decimal's mantissa fail loudly, never approximate.
        let err = parse("1e100").unwrap_err();
        assert!(err.message.contains("not representable"), "{err}");
    }

    #[test]
    fn strings_unescape_including_surrogate_pairs() {
        let s = |text: &str| match parse(text) {
            Ok(Json::Str(s)) => s,
            other => panic!("{text}: {other:?}"),
        };
        assert_eq!(s(r#""a\"b\\c\/d""#), "a\"b\\c/d");
        assert_eq!(s(r#""\b\f\n\r\t""#), "\u{8}\u{c}\n\r\t");
        assert_eq!(s(r#""Aé""#), "Aé");
        assert_eq!(s(r#""😀""#), "😀");
        assert_eq!(s(r#""₹ नमस्ते""#), "₹ नमस्ते");
        assert!(parse(r#""\ud83d""#).is_err(), "lone high surrogate");
        assert!(parse(r#""\ude00""#).is_err(), "lone low surrogate");
    }

    #[test]
    fn malformed_documents_fail_with_an_offset() {
        for bad in [
            r#"{"a":1} trailing"#,
            r#""unterminated"#,
            "NaN",
            "{,}",
            "[1,]",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(parse(r#"{"a":1} x"#).unwrap_err().offset, 8);
    }

    #[test]
    fn duplicate_keys_keep_the_last_value_in_the_first_position() {
        assert_eq!(
            parse(r#"{"a":1,"b":2,"a":3}"#).unwrap().to_json(),
            r#"{"a":3,"b":2}"#
        );
    }

    #[test]
    fn whitespace_and_literals_parse() {
        assert_eq!(
            parse(" {\n\t\"a\" : [ true , false , null ] }\r\n")
                .unwrap()
                .to_json(),
            r#"{"a":[true,false,null]}"#
        );
    }
}
