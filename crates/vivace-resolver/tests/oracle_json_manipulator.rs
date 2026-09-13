//! Oracle `JsonManipulator` : les scénarios d'édition que `require` et
//! `remove` produisent (ajout, remplacement et retrait de liens avec ou
//! sans tri, sous-clés de `config`, clés racines) sont rejoués par le phar
//! (tools/oracle-json-manipulator.php) et par le port sur un corpus de
//! manifestes réels — les composer.json des fixtures et, s'ils sont
//! présents, ceux des paquets installés dans fixtures/work — plus des cas
//! synthétiques de mise en forme. Retours, texte final et échec (exception
//! côté PHP, `Err` côté Rust) doivent coïncider.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use vivace_resolver::json_manipulator::JsonManipulator;

fn phar() -> PathBuf {
    let phar = std::env::temp_dir().join("vivace-oracle-composer.phar");
    if !phar.exists() {
        let src = String::from_utf8(
            Command::new("which")
                .arg("composer")
                .output()
                .expect("which")
                .stdout,
        )
        .expect("utf8");
        assert!(!src.trim().is_empty(), "composer requis");
        std::fs::copy(src.trim(), &phar).expect("copie");
    }
    phar
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Une opération du scénario, sous la forme que le script PHP attend.
#[derive(Clone, Debug)]
enum Op {
    AddLink(&'static str, String, &'static str, bool),
    AddSubNode(&'static str, String, Value, bool),
    RemoveSubNode(&'static str, String),
    AddMainKey(&'static str, Value),
    RemoveMainKey(&'static str),
    RemoveMainKeyIfEmpty(&'static str),
    RemoveConfigSetting(String),
}

impl Op {
    fn to_json(&self) -> Value {
        match self {
            Op::AddLink(t, p, c, s) => json!(["addLink", t, p, c, s]),
            Op::AddSubNode(m, n, v, a) => json!(["addSubNode", m, n, v, a]),
            Op::RemoveSubNode(m, n) => json!(["removeSubNode", m, n]),
            Op::AddMainKey(k, v) => json!(["addMainKey", k, v]),
            Op::RemoveMainKey(k) => json!(["removeMainKey", k]),
            Op::RemoveMainKeyIfEmpty(k) => json!(["removeMainKeyIfEmpty", k]),
            Op::RemoveConfigSetting(n) => json!(["removeConfigSetting", n]),
        }
    }

    fn apply(&self, m: &mut JsonManipulator) -> Result<bool, String> {
        let r = match self {
            Op::AddLink(t, p, c, s) => m.add_link(t, p, c, *s),
            Op::AddSubNode(mn, n, v, a) => m.add_sub_node(mn, n, v, *a),
            Op::RemoveSubNode(mn, n) => m.remove_sub_node(mn, n),
            Op::AddMainKey(k, v) => m.add_main_key(k, v),
            Op::RemoveMainKey(k) => m.remove_main_key(k),
            Op::RemoveMainKeyIfEmpty(k) => m.remove_main_key_if_empty(k),
            Op::RemoveConfigSetting(n) => m.remove_config_setting(n),
        };
        r.map_err(|e| e.to_string())
    }
}

/// Résultat d'un scénario, des deux côtés.
#[derive(Debug, PartialEq)]
struct Outcome {
    results: Vec<bool>,
    contents: Option<String>,
    failed: bool,
}

fn run_rust(contents: &str, ops: &[Op]) -> Outcome {
    let mut results = Vec::new();
    let mut m = match JsonManipulator::new(contents) {
        Ok(m) => m,
        Err(_) => {
            return Outcome {
                results,
                contents: None,
                failed: true,
            }
        }
    };
    for op in ops {
        match op.apply(&mut m) {
            Ok(b) => results.push(b),
            Err(_) => {
                return Outcome {
                    results,
                    contents: None,
                    failed: true,
                }
            }
        }
    }
    Outcome {
        results,
        contents: Some(m.contents()),
        failed: false,
    }
}

fn run_php(phar: &Path, files: &[(String, Vec<Vec<Op>>)]) -> Vec<Vec<Outcome>> {
    let input = json!({
        "phar": phar.to_string_lossy(),
        "files": files.iter().map(|(contents, scenarios)| json!({
            "contents": contents,
            "scenarios": scenarios.iter().map(|ops| ops.iter().map(Op::to_json).collect::<Vec<_>>()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    let mut child = Command::new("php")
        .args([
            "-d",
            "error_reporting=E_ALL & ~E_DEPRECATED",
            root()
                .join("tools/oracle-json-manipulator.php")
                .to_str()
                .expect("path"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("php requis");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(serde_json::to_string(&input).expect("json").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php");
    assert!(
        out.status.success(),
        "php: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("json: {e}\n{}", String::from_utf8_lossy(&out.stdout)));
    v.as_array()
        .expect("array")
        .iter()
        .map(|f| {
            f["scenarios"]
                .as_array()
                .expect("scenarios")
                .iter()
                .map(|s| Outcome {
                    results: s["results"]
                        .as_array()
                        .expect("results")
                        .iter()
                        .map(|b| b.as_bool().expect("bool"))
                        .collect(),
                    contents: s["contents"].as_str().map(str::to_owned),
                    failed: !s["error"].is_null(),
                })
                .collect()
        })
        .collect()
}

/// Les scénarios joués sur chaque manifeste : ceux de `require`/`remove`,
/// paramétrés par les clés que le fichier contient déjà.
fn scenarios(contents: &str) -> Vec<Vec<Op>> {
    let decoded: Value = serde_json::from_str(contents).unwrap_or(Value::Null);
    let first_key = |section: &str| -> Option<String> {
        decoded[section]
            .as_object()
            .and_then(|m| m.keys().next().cloned())
    };
    let first_plugin = decoded["config"]["allow-plugins"]
        .as_object()
        .and_then(|m| m.keys().next().cloned());
    let mut out = vec![
        vec![Op::AddLink(
            "require",
            "vivace/new-link".into(),
            "^1.0",
            false,
        )],
        vec![Op::AddLink(
            "require",
            "vivace/new-link".into(),
            "^1.0",
            true,
        )],
        vec![Op::AddLink(
            "require-dev",
            "vivace/new-dev".into(),
            "dev-main",
            true,
        )],
        vec![
            Op::AddLink("require", "ext-json".into(), "*", true),
            Op::AddLink("require", "php".into(), ">=8.1", true),
            Op::AddLink("require", "lib-icu".into(), "*", true),
            Op::AddLink("require", "composer-plugin-api".into(), "^2", true),
        ],
        vec![
            Op::AddSubNode(
                "config",
                "allow-plugins.vivace/plugin".into(),
                json!(true),
                true,
            ),
            Op::RemoveConfigSetting("allow-plugins.vivace/plugin".into()),
        ],
        vec![
            Op::AddSubNode("config", "sort-packages".into(), json!(true), true),
            Op::AddSubNode("config", "sort-packages".into(), json!(false), false),
        ],
        vec![
            Op::AddMainKey("extra", json!({"branch-alias": {"dev-main": "1.x-dev"}})),
            Op::AddMainKey("name", json!("vivace/renamed")),
        ],
        vec![
            Op::RemoveMainKey("require"),
            Op::RemoveMainKey("require-dev"),
        ],
        vec![
            Op::AddSubNode(
                "extra",
                "vivace.nested".into(),
                json!([1, "a", {"k": null}]),
                true,
            ),
            Op::AddSubNode("extra", "vivace.nested".into(), json!({}), true),
        ],
        vec![
            Op::AddSubNode(
                "scripts",
                "post-install-cmd".into(),
                json!(["echo ok"]),
                false,
            ),
            Op::RemoveSubNode("scripts", "post-install-cmd".into()),
        ],
        vec![Op::RemoveSubNode("config", "foo.1".into())],
        vec![Op::RemoveSubNode("config", "bar.0".into())],
        vec![Op::RemoveSubNode("config", "baz.0".into())],
        vec![Op::RemoveSubNode("config", "qux.0".into())],
        vec![Op::RemoveSubNode("config", "list.1".into())],
        vec![Op::RemoveSubNode("scripts", "test.0".into())],
        vec![Op::AddSubNode("extra", "x.y".into(), json!(1), true)],
        vec![
            Op::AddMainKey("description", json!("a\u{2028}b")),
            Op::AddLink("require", "e/f".into(), "^1\u{2029}", false),
        ],
        vec![
            Op::AddMainKey("\u{0}k", json!(1)),
            Op::AddMainKey("\u{0}k", json!(2)),
        ],
        vec![
            Op::AddMainKey("config", json!({"z": 1})),
            Op::RemoveMainKey("config"),
        ],
    ];
    if let Some(k) = first_key("require") {
        out.push(vec![
            Op::AddLink("require", k.to_uppercase(), "^9.9", false),
            Op::AddLink("require", k.clone(), "^9.8", true),
        ]);
        out.push(vec![
            Op::RemoveSubNode("require", k.clone()),
            Op::RemoveMainKeyIfEmpty("require"),
        ]);
        out.push(vec![
            Op::RemoveSubNode("require", k.to_uppercase()),
            Op::AddLink("require-dev", k, "*", true),
        ]);
    }
    if let Some(keys) = decoded["require"].as_object().filter(|m| m.len() > 1) {
        let last = keys.keys().next_back().cloned().unwrap_or_default();
        let middle = keys.keys().nth(keys.len() / 2).cloned().unwrap_or_default();
        out.push(vec![
            Op::RemoveSubNode("require", last.clone()),
            Op::RemoveSubNode("require", middle),
            Op::AddLink("require", last, "^0.1", false),
        ]);
        let mut all: Vec<Op> = keys
            .keys()
            .map(|k| Op::RemoveSubNode("require", k.clone()))
            .collect();
        all.push(Op::RemoveMainKeyIfEmpty("require"));
        all.push(Op::AddLink("require", "vivace/after".into(), "^1", true));
        out.push(all);
    }
    if let Some(k) = first_key("require-dev") {
        out.push(vec![
            Op::RemoveSubNode("require-dev", k.clone()),
            Op::RemoveMainKeyIfEmpty("require-dev"),
            Op::AddLink("require", k, "^1", true),
        ]);
    }
    if let Some(p) = first_plugin {
        out.push(vec![
            Op::RemoveConfigSetting(format!("allow-plugins.{p}")),
            Op::RemoveConfigSetting(format!("allow-plugins.{p}")),
        ]);
    }
    out
}

fn synthetic() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("braces", "{}".into()),
        ("braces-space", "{ }".into()),
        ("no-newline", "{\n    \"name\": \"a/b\"\n}".into()),
        ("crlf", "{\r\n    \"name\": \"a/b\",\r\n    \"require\": {\r\n        \"c/d\": \"^1\"\r\n    }\r\n}\r\n".into()),
        ("tabs", "{\n\t\"require\": {\n\t\t\"c/d\": \"^1\"\n\t}\n}\n".into()),
        ("two-spaces", "{\n  \"require\": {\n    \"c/d\": \"^1\",\n    \"e/f\": \"^2\"\n  },\n  \"require-dev\": {\n    \"g/h\": \"*\"\n  }\n}\n".into()),
        ("one-line", "{\"name\": \"a/b\", \"require\": {\"c/d\": \"^1\"}, \"require-dev\": {}}".into()),
        ("empty-sections", "{\n    \"require\": {},\n    \"require-dev\": [],\n    \"config\": {}\n}\n".into()),
        ("null-require", "{\n    \"require\": null\n}\n".into()),
        ("string-require", "{\n    \"require\": \"nope\"\n}\n".into()),
        ("escaped-slash", "{\n    \"require\": {\n        \"c\\/d\": \"^1\",\n        \"E\\/F\": \"^2\"\n    }\n}\n".into()),
        ("unicode", "{\n    \"description\": \"héhé ✓ \\u00e9\",\n    \"require\": {\n        \"c/d\": \"^1\"\n    }\n}\n".into()),
        ("hash-key", "{\n    \"require\": {\n        \"c/d#x\": \"^1\"\n    },\n    \"extra\": {\n        \"a b\": 1\n    }\n}\n".into()),
        ("duplicate", "{\n    \"require\": {\n        \"c/d\": \"^1\",\n        \"c/d\": \"^2\"\n    }\n}\n".into()),
        ("trailing-ws", "{\n    \"require\": {\n        \"c/d\": \"^1\"   \n    }   \n}   \n\n".into()),
        ("numbers", "{\n    \"extra\": {\n        \"f\": 1.0,\n        \"i\": 10,\n        \"e\": 1e3,\n        \"l\": [1, 2],\n        \"o\": {\"0\": \"a\", \"1\": \"b\"}\n    },\n    \"require\": {\n        \"c/d\": \"^1\"\n    }\n}\n".into()),
        ("nested-config", "{\n    \"config\": {\n        \"allow-plugins\": {\n            \"a/b\": true,\n            \"c/d\": false\n        },\n        \"sort-packages\": true\n    }\n}\n".into()),
        ("unsorted", "{\n    \"require\": {\n        \"zeta/z\": \"^1\",\n        \"ext-Json\": \"*\",\n        \"php\": \">=8\",\n        \"alpha/a10\": \"^1\",\n        \"alpha/a9\": \"^1\",\n        \"Alpha/b\": \"^1\",\n        \"hhvm\": \"*\",\n        \"lib-icu\": \"*\",\n        \"ext-mbstring\": \"*\",\n        \"composer-runtime-api\": \"^2\"\n    }\n}\n".into()),
        ("not-object", "[]".into()),
        ("invalid", "{\n    \"require\": {\n        \"c/d\": \"^1\",\n    }\n}\n".into()),
        ("bom", "\u{feff}{\n    \"require\": {\n        \"c/d\": \"^1\"\n    }\n}\n".into()),
        ("leading-blank", "\n\n  {\n    \"require\": {\n        \"c/d\": \"^1\"\n    }\n}".into()),
        ("mixed-indent", "{\n  \"name\": \"a/b\",\n    \"require\": {\n            \"c/d\": \"^1\"\n    }\n}\n".into()),
        ("scripts-list", "{\n    \"scripts\": {\n        \"post-install-cmd\": [\"a\", \"b\"],\n        \"test\": \"phpunit\"\n    }\n}\n".into()),
        // Cas relevés en revue : section en liste, clés entières, sous-clé
        // sur une chaîne / un booléen / une liste, `-0`, U+2028, clé NUL,
        // imbrication au-delà de 128 niveaux.
        ("list-section", "{\n    \"require\": [{\"c/d\": \"^1\"}],\n    \"require-dev\": [\"x\", \"y\"]\n}\n".into()),
        ("int-keys", "{\n    \"require\": {\n        \"0\": \"^1\",\n        \"c/d\": \"^1\"\n    },\n    \"require-dev\": {\n        \"-3\": \"*\"\n    }\n}\n".into()),
        ("int-key-alone", "{\n    \"require\": {\n        \"0\": \"^1\"\n    }\n}\n".into()),
        ("scalar-subkeys", "{\n    \"config\": {\n        \"foo\": \"abc\",\n        \"bar\": false,\n        \"baz\": null,\n        \"qux\": true,\n        \"list\": [\"a\", \"b\"]\n    },\n    \"scripts\": {\n        \"test\": [\"a\", \"b\"]\n    }\n}\n".into()),
        ("minus-zero", "{\n    \"extra\": {\n        \"x\": {\n            \"n\": -0,\n            \"m\": -0.0,\n            \"p\": [-0, 1]\n        }\n    },\n    \"require\": {\n        \"c/d\": \"^1\"\n    }\n}\n".into()),
        ("line-separators", "{\n    \"description\": \"a\u{2028}b\u{2029}c\",\n    \"require\": {\n        \"c/d\": \"^1\"\n    }\n}\n".into()),
        ("nul-key", "{\n    \"config\": {\n        \"x\": {\n            \"\\u0000a\": 1\n        }\n    },\n    \"require\": {\n        \"c/d\": \"^1\"\n    }\n}\n".into()),
        ("deep", format!("{{\n    \"require\": {{\n        \"c/d\": \"^1\"\n    }},\n    \"extra\": {}1{}\n}}\n", "{\"a\": ".repeat(200), "}".repeat(200))),
    ]
}

/// Les composer.json des fixtures, puis ceux des paquets installés dans
/// fixtures/work (dédoublonnés par contenu).
fn corpus() -> Vec<(String, String)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let projects = root().join("fixtures/projects");
    for entry in std::fs::read_dir(&projects).expect("fixtures/projects") {
        let p = entry.expect("entry").path().join("composer.json");
        if let Ok(s) = std::fs::read_to_string(&p) {
            if seen.insert(s.clone()) {
                out.push((p.to_string_lossy().into_owned(), s));
            }
        }
    }
    assert!(
        out.len() >= 8,
        "fixtures/projects: {} manifestes",
        out.len()
    );
    let work = root().join("fixtures/work");
    if work.is_dir() {
        for f in walk(&work) {
            if let Ok(s) = std::fs::read_to_string(&f) {
                if s.len() < 64 * 1024 && seen.insert(s.clone()) {
                    out.push((f.to_string_lossy().into_owned(), s));
                }
            }
        }
    }
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else if p.file_name().is_some_and(|n| n == "composer.json") {
                out.push(p);
            }
        }
    }
    out
}

fn check(phar: &Path, files: Vec<(String, String)>) -> usize {
    let mut failures = Vec::new();
    let mut scenarios_run = 0;
    let mut failed_both = 0;
    for chunk in files.chunks(40) {
        let batch: Vec<(String, Vec<Vec<Op>>)> = chunk
            .iter()
            .map(|(_, contents)| (contents.clone(), scenarios(contents)))
            .collect();
        let expected = run_php(phar, &batch);
        for (((name, _), (contents, scenarios)), expected) in chunk.iter().zip(&batch).zip(expected)
        {
            assert_eq!(scenarios.len(), expected.len(), "{name}");
            for (ops, want) in scenarios.iter().zip(expected) {
                scenarios_run += 1;
                let got = run_rust(contents, ops);
                if want.failed {
                    failed_both += 1;
                }
                if got != want {
                    failures.push(format!(
                        "{name}\n  ops: {ops:?}\n  php : {want:?}\n  rust: {got:?}"
                    ));
                }
            }
        }
    }
    eprintln!("{scenarios_run} scénarios, dont {failed_both} en échec côté PHP");
    assert!(
        failures.is_empty(),
        "{} divergence(s) sur {scenarios_run} scénarios :\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    scenarios_run
}

#[test]
fn synthetic_cases_match_composer() {
    let phar = phar();
    let files: Vec<(String, String)> = synthetic()
        .into_iter()
        .map(|(n, s)| (n.to_owned(), s))
        .collect();
    let n = check(&phar, files);
    eprintln!("synthétiques : {n} scénarios");
}

#[test]
fn real_manifests_match_composer() {
    let phar = phar();
    let files = corpus();
    let count = files.len();
    let n = check(&phar, files);
    eprintln!("corpus : {count} manifestes, {n} scénarios");
}
