//! Réencodage JSON reproduisant `json_encode($data, 0)` de PHP appliqué à des
//! données issues de `json_decode($json, true)` (pipeline de JsonFile::parseJson
//! → JsonFile::encode, Composer 2.10.3, voir docs/reference/JsonFile.php).
//!
//! Sémantique à l'octet près :
//! - sortie compacte (`{"k":v}`), ordre d'insertion préservé ;
//! - `/` échappé en `\/`, non-ASCII en `\uXXXX` hexa minuscule (paires de
//!   substituts au-delà du BMP), contrôles `\b \f \n \r \t` puis `\u00XX` ;
//! - quirk assoc : un objet vide devient `[]`, un objet dont les clés sont
//!   exactement "0".."n-1" dans l'ordre devient un tableau (PHP a perdu la
//!   distinction objet/tableau au decode) ;
//! - floats au plus court round-trip (serialize_precision=-1) : notation fixe
//!   sans `.0` final pour les valeurs entières (`1.0` → `1`), exponentielle
//!   `d[.ddd|.0]e±X` hors de (-4, 17] — voir `encode_double`.

use crate::error::{Error, Result};
use serde_json::Value;

/// Options miroir des flags de json_encode utilisés par Composer.
#[derive(Debug, Clone, Copy)]
pub struct EncodeOptions {
    pub pretty: bool,
    pub escape_slashes: bool,
    pub escape_unicode: bool,
}

/// Flags 0 (content-hash) : compact, slashes et unicode échappés.
pub const FLAGS_ZERO: EncodeOptions = EncodeOptions {
    pretty: false,
    escape_slashes: true,
    escape_unicode: true,
};

/// Défaut JsonFile (fichiers écrits par Composer) :
/// JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE.
pub const FLAGS_JSONFILE: EncodeOptions = EncodeOptions {
    pretty: true,
    escape_slashes: false,
    escape_unicode: false,
};

pub fn php_json_encode(value: &Value) -> Result<String> {
    php_json_encode_with(value, FLAGS_ZERO)
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
            if is_php_list(map) {
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

/// Après `json_decode(..., true)`, PHP encode en tableau tout array-assoc dont
/// les clés sont exactement 0..n-1 dans l'ordre (un objet vide inclus).
fn is_php_list(map: &serde_json::Map<String, Value>) -> bool {
    map.keys()
        .enumerate()
        .all(|(i, k)| k.as_str() == i.to_string())
}

fn encode_number(n: &serde_json::Number, out: &mut String) -> Result<()> {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        // PHP_INT_MAX == i64::MAX : au-delà, json_decode produit un float.
        encode_double(u as f64, out)?;
    } else if let Some(f) = n.as_f64() {
        encode_double(f, out)?;
    }
    Ok(())
}

/// Formatage double de `json_encode` avec serialize_precision=-1 : chiffres
/// shortest-round-trip, notation fixe ssi le point décimal est dans (-4, 17]
/// (bornes relevées empiriquement sur PHP 8.5, cf. tests différentiels),
/// sinon exponentielle `d[.ddd|.0]e±X`. Pas de `.0` sur les entiers en fixe.
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
    // `{:e}` produit `d[.ddd]e±X` avec la mantisse shortest round-trip —
    // les mêmes chiffres que PHP (double-conversion).
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().map_err(|_| Error::NonFiniteFloat(f))?;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let dec_point = exp + 1; // valeur = 0.digits × 10^dec_point

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
            c if !opts.escape_unicode => out.push(c),
            c => {
                let cp = c as u32;
                if cp > 0xFFFF {
                    // Paire de substituts UTF-16, comme json_encode.
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
    // write! sur String est infaillible.
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
        // Ordre non séquentiel → reste un objet.
        let v: Value = serde_json::from_str(r#"{"1":"a","0":"b"}"#).unwrap();
        assert_eq!(enc(v), r#"{"1":"a","0":"b"}"#);
        // Trou dans les indices → reste un objet.
        let v: Value = serde_json::from_str(r#"{"0":"a","2":"b"}"#).unwrap();
        assert_eq!(enc(v), r#"{"0":"a","2":"b"}"#);
    }

    #[test]
    fn numbers_round_trip() {
        assert_eq!(enc(json!(42)), "42");
        assert_eq!(enc(json!(-7)), "-7");
        assert_eq!(enc(json!(1.5)), "1.5");
        // Frontières empiriques PHP (voir encode_double + tests oracle).
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
