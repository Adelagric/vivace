//! JSON re-encoding reproducing PHP's `json_encode($data, 0)` applied to data
//! coming from `json_decode($json, true)` (the JsonFile::parseJson ->
//! JsonFile::encode pipeline, Composer 2.10.3, see docs/reference/JsonFile.php).
//!
//! Byte-exact semantics:
//! - compact output (`{"k":v}`), insertion order preserved;
//! - `/` escaped as `\/`, non-ASCII as lowercase-hex `\uXXXX` (surrogate
//!   pairs beyond the BMP), controls `\b \f \n \r \t` then `\u00XX`;
//! - assoc quirk: an empty object becomes `[]`, an object whose keys are
//!   exactly "0".."n-1" in order becomes an array (PHP lost the
//!   object/array distinction at decode time);
//! - shortest round-trip floats (serialize_precision=-1): fixed notation
//!   without a trailing `.0` for integral values (`1.0` -> `1`), exponential
//!   `d[.ddd|.0]e±X` outside (-4, 17]; see `encode_double`.

use crate::error::{Error, Result};
use serde_json::Value;

/// Options mirroring the json_encode flags used by Composer.
#[derive(Debug, Clone, Copy)]
pub struct EncodeOptions {
    pub pretty: bool,
    pub escape_slashes: bool,
    pub escape_unicode: bool,
}

/// Flags 0 (content-hash): compact, slashes and unicode escaped.
pub const FLAGS_ZERO: EncodeOptions = EncodeOptions {
    pretty: false,
    escape_slashes: true,
    escape_unicode: true,
};

/// JsonFile default (files written by Composer):
/// JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE.
pub const FLAGS_JSONFILE: EncodeOptions = EncodeOptions {
    pretty: true,
    escape_slashes: false,
    escape_unicode: false,
};

pub fn php_json_encode(value: &Value) -> Result<String> {
    php_json_encode_with(value, FLAGS_ZERO)
}

/// Sentinel key: an object reduced to this key encodes as `{}` (an empty
/// `stdClass` on the PHP side, which the array semantics cannot express).
pub const STDCLASS_MARKER: &str = "\u{0}stdClass";

/// An empty object that will encode as `{}`.
pub fn empty_stdclass() -> Value {
    let mut m = serde_json::Map::new();
    m.insert(STDCLASS_MARKER.to_owned(), Value::Null);
    Value::Object(m)
}

pub fn php_json_encode_with(value: &Value, opts: EncodeOptions) -> Result<String> {
    let mut out = String::new();
    encode_into(value, &mut out, opts, 0)?;
    Ok(out)
}

fn newline_indent(out: &mut String, level: usize) {
    out.push('\n');
    for _ in 0..level {
        out.push_str("    ");
    }
}

fn encode_into(value: &Value, out: &mut String, opts: EncodeOptions, level: usize) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => encode_number(n, out)?,
        Value::String(s) => encode_string_with(s, out, opts),
        Value::Array(items) => encode_list(items.iter(), out, opts, level)?,
        Value::Object(map) => {
            if map.len() == 1 && map.contains_key(STDCLASS_MARKER) {
                // Empty `new \stdClass` (Locker::fixupJsonDataType): `{}` where
                // an empty array would give `[]`.
                out.push_str("{}");
            } else if is_php_list(map) {
                encode_list(map.values(), out, opts, level)?;
            } else {
                out.push('{');
                for (i, (key, item)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if opts.pretty {
                        newline_indent(out, level + 1);
                    }
                    encode_string_with(key, out, opts);
                    out.push(':');
                    if opts.pretty {
                        out.push(' ');
                    }
                    encode_into(item, out, opts, level + 1)?;
                }
                if opts.pretty && !map.is_empty() {
                    newline_indent(out, level);
                }
                out.push('}');
            }
        }
    }
    Ok(())
}

fn encode_list<'a>(
    items: impl ExactSizeIterator<Item = &'a Value>,
    out: &mut String,
    opts: EncodeOptions,
    level: usize,
) -> Result<()> {
    let len = items.len();
    out.push('[');
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        if opts.pretty {
            newline_indent(out, level + 1);
        }
        encode_into(item, out, opts, level + 1)?;
    }
    if opts.pretty && len > 0 {
        newline_indent(out, level);
    }
    out.push(']');
    Ok(())
}

/// After `json_decode(..., true)`, PHP encodes as an array any assoc array
/// whose keys are exactly 0..n-1 in order (an empty object included).
fn is_php_list(map: &serde_json::Map<String, Value>) -> bool {
    map.keys()
        .enumerate()
        .all(|(i, k)| k.as_str() == i.to_string())
}

fn encode_number(n: &serde_json::Number, out: &mut String) -> Result<()> {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        // PHP_INT_MAX == i64::MAX: beyond it, json_decode produces a float.
        encode_double(u as f64, out)?;
    } else if let Some(f) = n.as_f64() {
        encode_double(f, out)?;
    }
    Ok(())
}

/// Double formatting of `json_encode` with serialize_precision=-1:
/// shortest-round-trip digits, fixed notation iff the decimal point lies in
/// (-4, 17] (bounds measured empirically on PHP 8.5, cf. differential tests),
/// else exponential `d[.ddd|.0]e±X`. No `.0` on integral values in fixed.
/// Shortest significant digits that round-trip, and decimal exponent, like
/// `zend_gcvt` mode 0 (dtoa). Rust's `{:e}` gives the same string except on
/// an exact tie between two candidates (the value sits halfway, e.g.
/// 2124202659384827.25 -> "...27.2" or "...27.3"): dtoa rounds to the even
/// digit, Rust rounds up. Rust's fixed-precision formatting (exact,
/// half-to-even) decides like dtoa; we keep it if it still round-trips
/// (always true away from an asymmetric interval edge).
fn shortest_digits(a: f64) -> (String, String) {
    let sci = format!("{:e}", a);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exact = format!("{:.*e}", digits.len().saturating_sub(1), a);
    if exact != sci && exact.parse::<f64>() == Ok(a) {
        let (m, e) = exact.split_once('e').unwrap_or((exact.as_str(), "0"));
        let mut d: String = m.chars().filter(|c| *c != '.').collect();
        while d.len() > 1 && d.ends_with('0') {
            d.pop();
        }
        return (d, e.to_owned());
    }
    (digits, exp.to_owned())
}

/// `smart_str_append_double` with `serialize_precision = -1`: the shortest
/// round-trip form PHP prints for a float (`json_encode` and `serialize`
/// share it; the latter spells the exponent `E`).
pub fn php_double(f: f64) -> Result<String> {
    let mut out = String::new();
    encode_double(f, &mut out)?;
    Ok(out)
}

fn encode_double(f: f64, out: &mut String) -> Result<()> {
    if !f.is_finite() {
        return Err(Error::NonFiniteFloat(f));
    }
    if f == 0.0 {
        out.push_str(if f.is_sign_negative() { "-0" } else { "0" });
        return Ok(());
    }
    if f.is_sign_negative() {
        out.push('-');
    }
    let (digits, exp) = shortest_digits(f.abs());
    let exp: i32 = exp.parse().map_err(|_| Error::NonFiniteFloat(f))?;
    let dec_point = exp + 1; // value = 0.digits x 10^dec_point

    if dec_point > -4 && dec_point <= 17 {
        let n = digits.len() as i32;
        if dec_point <= 0 {
            out.push_str("0.");
            for _ in 0..-dec_point {
                out.push('0');
            }
            out.push_str(&digits);
        } else if dec_point >= n {
            out.push_str(&digits);
            for _ in 0..(dec_point - n) {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..dec_point as usize]);
            out.push('.');
            out.push_str(&digits[dec_point as usize..]);
        }
    } else {
        out.push_str(&digits[..1]);
        out.push('.');
        if digits.len() > 1 {
            out.push_str(&digits[1..]);
        } else {
            out.push('0');
        }
        let e = dec_point - 1;
        if e >= 0 {
            out.push_str("e+");
        } else {
            out.push_str("e-");
        }
        out.push_str(&e.abs().to_string());
    }
    Ok(())
}

fn encode_string_with(s: &str, out: &mut String, opts: EncodeOptions) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' if opts.escape_slashes => out.push_str("\\/"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                push_unicode_escape(c as u32, out);
            }
            c if c.is_ascii() => out.push(c),
            // Without JSON_UNESCAPED_LINE_TERMINATORS, json_encode escapes
            // U+2028/U+2029 even with JSON_UNESCAPED_UNICODE.
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if !opts.escape_unicode => out.push(c),
            c => {
                let cp = c as u32;
                if cp > 0xFFFF {
                    // UTF-16 surrogate pair, like json_encode.
                    let v = cp - 0x10000;
                    push_unicode_escape(0xD800 + (v >> 10), out);
                    push_unicode_escape(0xDC00 + (v & 0x3FF), out);
                } else {
                    push_unicode_escape(cp, out);
                }
            }
        }
    }
    out.push('"');
}

fn push_unicode_escape(cp: u32, out: &mut String) {
    use std::fmt::Write as _;
    // write! on a String is infallible.
    let _ = write!(out, "\\u{cp:04x}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn enc(v: Value) -> String {
        php_json_encode(&v).expect("encodable")
    }

    #[test]
    fn compact_object_preserves_order() {
        let v: Value = serde_json::from_str(r#"{"b":1,"a":{"z":true,"y":null}}"#).unwrap();
        assert_eq!(enc(v), r#"{"b":1,"a":{"z":true,"y":null}}"#);
    }

    #[test]
    fn slash_and_unicode_are_escaped() {
        assert_eq!(enc(json!("a/b")), r#""a\/b""#);
        assert_eq!(enc(json!("héhé")), "\"h\\u00e9h\\u00e9\"");
        assert_eq!(enc(json!("🎼")), "\"\\ud83c\\udfbc\"");
        assert_eq!(enc(json!("tab\tok")), "\"tab\\tok\"");
        assert_eq!(enc(json!("\u{1}")), "\"\\u0001\"");
    }

    #[test]
    fn empty_object_becomes_array() {
        let v: Value = serde_json::from_str(r#"{"extra":{}}"#).unwrap();
        assert_eq!(enc(v), r#"{"extra":[]}"#);
    }

    #[test]
    fn sequential_numeric_keys_become_array() {
        let v: Value = serde_json::from_str(r#"{"0":"a","1":"b"}"#).unwrap();
        assert_eq!(enc(v), r#"["a","b"]"#);
        // Non-sequential order -> stays an object.
        let v: Value = serde_json::from_str(r#"{"1":"a","0":"b"}"#).unwrap();
        assert_eq!(enc(v), r#"{"1":"a","0":"b"}"#);
        // Gap in the indices -> stays an object.
        let v: Value = serde_json::from_str(r#"{"0":"a","2":"b"}"#).unwrap();
        assert_eq!(enc(v), r#"{"0":"a","2":"b"}"#);
    }

    #[test]
    fn numbers_round_trip() {
        assert_eq!(enc(json!(42)), "42");
        assert_eq!(enc(json!(-7)), "-7");
        assert_eq!(enc(json!(1.5)), "1.5");
        // Empirical PHP boundaries (see encode_double + oracle tests).
        assert_eq!(enc(json!(1.0)), "1");
        assert_eq!(enc(json!(1.0e-7)), "1.0e-7");
        assert_eq!(enc(json!(0.0001)), "0.0001");
        assert_eq!(enc(json!(1.0e-5)), "1.0e-5");
        assert_eq!(enc(json!(9.9e16)), "99000000000000000");
        assert_eq!(enc(json!(1.0e17)), "1.0e+17");
        assert_eq!(enc(json!(1.23e17)), "1.23e+17");
        assert_eq!(enc(json!(-0.0)), "-0");
        assert_eq!(enc(json!(5e-324)), "5.0e-324");
        assert_eq!(
            enc(json!(1.7976931348623157e308)),
            "1.7976931348623157e+308"
        );
        assert_eq!(
            enc(json!(12345678901234567890_u64)),
            "1.2345678901234567e+19"
        );
    }
}
