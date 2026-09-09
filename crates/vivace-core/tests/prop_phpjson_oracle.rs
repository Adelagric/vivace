//! Property test différentiel : php_json_encode(v) doit être octet-identique à
//! `json_encode(json_decode($json, true), 0)` de PHP pour des valeurs JSON
//! arbitraires. L'oracle est le PHP réel — le générateur cherche les cas
//! auxquels on n'a pas pensé (le différentiel exemple-par-exemple prouve les
//! cas connus).

use proptest::prelude::*;
use serde_json::Value;
use std::io::Write as _;
use std::process::{Command, Stdio};

fn arb_json(depth: u32) -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|i| Value::Number(i.into())),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null)),
        // Strings avec du contenu hostile : unicode, contrôles, quotes, slashes.
        "[\\PC\\n\\t/\"\\\\]{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(depth, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            prop::collection::vec(("[a-z0-9/\\-]{0,6}", inner), 0..5).prop_map(|kvs| {
                let mut map = serde_json::Map::new();
                for (k, v) in kvs {
                    map.insert(k, v);
                }
                Value::Object(map)
            }),
        ]
    })
}

fn php_oracle_encode(json_text: &str) -> String {
    let mut child = Command::new("php")
        .args([
            "-d",
            "error_reporting=0",
            "-r",
            "echo json_encode(json_decode(stream_get_contents(STDIN), true), 0);",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("php doit être installé");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(json_text.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php exit");
    assert!(out.status.success(), "oracle PHP en échec");
    String::from_utf8(out.stdout).expect("utf8")
}

proptest! {
    // L'oracle forke un process PHP par cas : on borne le nombre de cas pour
    // garder le test sous quelques secondes (montez PROPTEST_CASES localement).
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]
    #[test]
    fn matches_php_json_encode(v in arb_json(3)) {
        // On passe par le texte JSON pour que les deux camps décodent la même
        // chose (serde et PHP normalisent chacun leur parsing).
        let text = serde_json::to_string(&v).expect("serde encode");
        let reparsed: Value = serde_json::from_str(&text).expect("reparse");
        let ours = vivace_core::phpjson::php_json_encode(&reparsed).expect("encode");
        let theirs = php_oracle_encode(&text);
        prop_assert_eq!(ours, theirs);
    }
}
