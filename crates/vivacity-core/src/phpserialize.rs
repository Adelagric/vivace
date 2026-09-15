//! PHP `serialize()` of a value that came out of `json_decode($json, true)`:
//! what `PathRepository::initialize` hashes (`sha1($json . serialize($this
//! ->options))`) to build the dist reference of a path package. Only the
//! shapes a JSON document can produce are covered: assoc arrays in
//! insertion order (`a:n:{…}`; a key that PHP would have turned into an
//! integer — a canonical decimal in the `int` range — is written `i:`), lists,
//! strings (`s:<bytes>:"…";`), integers, floats in PHP's shortest round-trip
//! form (`d:`), booleans and null.

use crate::error::{Error, Result};
use serde_json::Value;
use std::fmt::Write as _;

/// `serialize(json_decode($json, true))` for `v`.
pub fn serialize(v: &Value) -> Result<String> {
    let mut out = String::new();
    write_value(v, &mut out)?;
    Ok(out)
}

fn write_value(v: &Value, out: &mut String) -> Result<()> {
    match v {
        Value::Null => out.push_str("N;"),
        Value::Bool(b) => {
            let _ = write!(out, "b:{};", u8::from(*b));
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let _ = write!(out, "i:{i};");
            } else if let Some(f) = n.as_f64() {
                // Beyond the int range PHP decodes a float.
                let _ = write!(
                    out,
                    "d:{};",
                    crate::phpjson::php_double(f)?.replace('e', "E")
                );
            } else {
                return Err(Error::NonFiniteFloat(f64::NAN));
            }
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            let _ = write!(out, "a:{}:{{", items.len());
            for (i, item) in items.iter().enumerate() {
                let _ = write!(out, "i:{i};");
                write_value(item, out)?;
            }
            out.push('}');
        }
        Value::Object(map) => {
            let _ = write!(out, "a:{}:{{", map.len());
            for (k, item) in map {
                match php_int_key(k) {
                    Some(i) => {
                        let _ = write!(out, "i:{i};");
                    }
                    None => write_string(k, out),
                }
                write_value(item, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(s: &str, out: &mut String) {
    let _ = write!(out, "s:{}:\"{s}\";", s.len());
}

/// `ZEND_HANDLE_NUMERIC_STR`: a key that is a canonical decimal integer
/// (no leading zero unless `0`, optional `-`, not `-0`) within the int
/// range becomes an integer key.
fn php_int_key(k: &str) -> Option<i64> {
    let digits = k.strip_prefix('-').unwrap_or(k);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    if k == "-0" {
        return None;
    }
    k.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn path_repository_options() {
        assert_eq!(
            serialize(&json!({"relative": true})).unwrap(),
            "a:1:{s:8:\"relative\";b:1;}"
        );
        assert_eq!(
            serialize(&json!({"symlink": false, "versions": {"acme/beta": "2.0.0"}, "relative": true}))
                .unwrap(),
            "a:3:{s:7:\"symlink\";b:0;s:8:\"versions\";a:1:{s:9:\"acme/beta\";s:5:\"2.0.0\";}s:8:\"relative\";b:1;}"
        );
    }

    #[test]
    fn scalars_and_keys_like_php() {
        // `php -r 'echo serialize(json_decode(…, true));'` (PHP 8.4).
        let v: Value = serde_json::from_str(
            r#"{"a":1.5,"b":10,"c":null,"d":[1,"x"],"e":{},"f":[],"g":0.1,"h":1.0,"i":100000000000000000000,"j":1e-7,"k":-0.0,"l":"é"}"#,
        )
        .unwrap();
        assert_eq!(
            serialize(&v).unwrap(),
            "a:12:{s:1:\"a\";d:1.5;s:1:\"b\";i:10;s:1:\"c\";N;s:1:\"d\";a:2:{i:0;i:1;i:1;s:1:\"x\";}s:1:\"e\";a:0:{}s:1:\"f\";a:0:{}s:1:\"g\";d:0.1;s:1:\"h\";d:1;s:1:\"i\";d:1.0E+20;s:1:\"j\";d:1.0E-7;s:1:\"k\";d:-0;s:1:\"l\";s:2:\"é\";}"
        );
        let v: Value = serde_json::from_str(
            r#"{"1":"x","05":"y","-3":"z","1.0":"w","9223372036854775807":1,"9223372036854775808":2}"#,
        )
        .unwrap();
        assert_eq!(
            serialize(&v).unwrap(),
            "a:6:{i:1;s:1:\"x\";s:2:\"05\";s:1:\"y\";i:-3;s:1:\"z\";s:3:\"1.0\";s:1:\"w\";i:9223372036854775807;i:1;s:19:\"9223372036854775808\";i:2;}"
        );
        let v: Value = serde_json::from_str(
            "[18446744073709551615, 9223372036854775808, -9223372036854775808]",
        )
        .unwrap();
        assert_eq!(
            serialize(&v).unwrap(),
            "a:3:{i:0;d:1.8446744073709552E+19;i:1;d:9.223372036854776E+18;i:2;i:-9223372036854775808;}"
        );
    }
}
