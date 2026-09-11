//! Oracle composer/semver sur des données réelles : toutes les contraintes et
//! versions présentes dans les instantanés Packagist des fixtures
//! (fixtures/registry/*.tar.gz) passent par le phar — `parseConstraints`
//! (forme chaîne), `normalize`, `parseStability`, `version_compare`, et
//! `matches` sur des paires contrainte/version — et par le port.

use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use vivace_resolver::constraint::parse_constraints;
use vivace_resolver::phpver::version_compare;
use vivace_resolver::version::{normalize, parse_stability};

fn phar() -> std::path::PathBuf {
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

/// Contraintes et versions distinctes des instantanés (ordre trié).
fn corpus() -> (Vec<String>, Vec<String>) {
    let registry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/registry");
    let tmp = tempfile::tempdir().expect("tmp");
    let mut constraints: BTreeSet<String> = BTreeSet::new();
    let mut versions: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(&registry).expect("registry") {
        let p = entry.expect("entry").path();
        if p.extension().is_some_and(|e| e == "gz") {
            let dir = tmp.path().join(p.file_stem().expect("stem"));
            std::fs::create_dir_all(&dir).expect("mkdir");
            assert!(Command::new("tar")
                .args(["-C", &dir.to_string_lossy(), "-xzf", &p.to_string_lossy()])
                .status()
                .expect("tar")
                .success());
            for f in walk(&dir.join("p2")) {
                let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).expect("read"))
                    .expect("json");
                for (_, list) in v["packages"].as_object().into_iter().flatten() {
                    for pkg in list.as_array().into_iter().flatten() {
                        if let Some(s) = pkg["version"].as_str() {
                            versions.insert(s.to_owned());
                        }
                        for key in ["require", "require-dev", "conflict", "replace", "provide"] {
                            if let Some(m) = pkg[key].as_object() {
                                for (_, c) in m {
                                    if let Some(s) = c.as_str() {
                                        constraints.insert(s.to_owned());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (
        constraints.into_iter().collect(),
        versions.into_iter().collect(),
    )
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else if p.extension().is_some_and(|x| x == "json")
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("._"))
            {
                out.push(p);
            }
        }
    }
    out
}

fn php(script: &str, input: &Value) -> Value {
    let mut child = Command::new("php")
        .args([
            "-d",
            "memory_limit=-1",
            "-d",
            "error_reporting=E_ALL & ~E_DEPRECATED",
            "-r",
            script,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("php requis");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(serde_json::to_string(input).expect("json").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php");
    assert!(
        out.status.success(),
        "php: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("json: {e}\n{}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn semver_matches_composer_on_the_snapshot_corpus() {
    let phar = phar();
    let (constraints, versions) = corpus();
    assert!(
        constraints.len() > 500,
        "corpus contraintes: {}",
        constraints.len()
    );
    assert!(versions.len() > 500, "corpus versions: {}", versions.len());
    eprintln!(
        "corpus : {} contraintes, {} versions",
        constraints.len(),
        versions.len()
    );

    // Contraintes : forme chaîne (ou erreur).
    let script = format!(
        r#"require "phar://{}/vendor/autoload.php";
           $p = new \Composer\Semver\VersionParser();
           $out = [];
           foreach (json_decode(stream_get_contents(STDIN), true) as $c) {{
               try {{ $out[] = (string) $p->parseConstraints($c); }} catch (\Throwable $e) {{ $out[] = null; }}
           }}
           echo json_encode($out);"#,
        phar.display()
    );
    let expected = php(
        &script,
        &Value::Array(
            constraints
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        ),
    );
    let mut bad = 0;
    for (c, exp) in constraints.iter().zip(expected.as_array().expect("array")) {
        let ours = parse_constraints(c).ok().map(|p| p.constraint.to_string());
        let exp = exp.as_str().map(str::to_owned);
        if ours != exp {
            bad += 1;
            if bad <= 15 {
                eprintln!("contrainte {c:?}: composer={exp:?} vivace={ours:?}");
            }
        }
    }
    assert_eq!(bad, 0, "{bad} contraintes divergentes");

    // Versions : normalize + parseStability.
    let script = format!(
        r#"require "phar://{}/vendor/autoload.php";
           $p = new \Composer\Semver\VersionParser();
           $out = [];
           foreach (json_decode(stream_get_contents(STDIN), true) as $v) {{
               try {{ $n = $p->normalize($v); }} catch (\Throwable $e) {{ $n = null; }}
               $out[] = [$n, \Composer\Semver\VersionParser::parseStability($v)];
           }}
           echo json_encode($out);"#,
        phar.display()
    );
    let expected = php(
        &script,
        &Value::Array(versions.iter().map(|s| Value::String(s.clone())).collect()),
    );
    let mut bad = 0;
    let mut normalized: Vec<String> = Vec::new();
    for (v, exp) in versions.iter().zip(expected.as_array().expect("array")) {
        let ours = normalize(v, None).ok();
        let exp_n = exp[0].as_str().map(str::to_owned);
        let stab = parse_stability(v);
        if ours != exp_n || Some(stab) != exp[1].as_str() {
            bad += 1;
            if bad <= 15 {
                eprintln!(
                    "version {v:?}: composer={exp_n:?}/{} vivace={ours:?}/{stab}",
                    exp[1]
                );
            }
        }
        if let Some(n) = ours {
            normalized.push(n);
        }
    }
    assert_eq!(bad, 0, "{bad} versions divergentes");

    // version_compare sur des paires de versions normalisées (échantillon
    // déterministe) et matches(contrainte, version).
    normalized.sort();
    normalized.dedup();
    let step = std::cmp::max(1, normalized.len() / 400);
    let sample: Vec<&String> = normalized.iter().step_by(step).collect();
    let cstep = std::cmp::max(1, constraints.len() / 300);
    let csample: Vec<&String> = constraints.iter().step_by(cstep).collect();
    let mut pairs: Vec<Value> = Vec::new();
    for (i, a) in sample.iter().enumerate() {
        let b = sample[(i * 7 + 3) % sample.len()];
        pairs.push(serde_json::json!([a, b]));
    }
    let mut matches_in: Vec<Value> = Vec::new();
    for (i, c) in csample.iter().enumerate() {
        for k in 0..5 {
            let v = sample[(i * 11 + k * 37) % sample.len()];
            matches_in.push(serde_json::json!([c, v]));
        }
    }
    let script = format!(
        r#"require "phar://{}/vendor/autoload.php";
           $p = new \Composer\Semver\VersionParser();
           $in = json_decode(stream_get_contents(STDIN), true);
           $cmp = []; foreach ($in['pairs'] as [$a, $b]) $cmp[] = version_compare($a, $b);
           $m = []; foreach ($in['matches'] as [$c, $v]) {{
               try {{ $m[] = \Composer\Semver\CompilingMatcher::match($p->parseConstraints($c), \Composer\Semver\Constraint\Constraint::OP_EQ, $v); }}
               catch (\Throwable $e) {{ $m[] = null; }}
           }}
           echo json_encode(['cmp' => $cmp, 'matches' => $m]);"#,
        phar.display()
    );
    let expected = php(
        &script,
        &serde_json::json!({"pairs": pairs, "matches": matches_in}),
    );
    let mut bad = 0;
    for ((i, a), exp) in sample
        .iter()
        .enumerate()
        .zip(expected["cmp"].as_array().expect("cmp"))
    {
        let b = sample[(i * 7 + 3) % sample.len()];
        let ours = match version_compare(a, b) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        if Some(ours) != exp.as_i64() {
            bad += 1;
            if bad <= 10 {
                eprintln!("version_compare({a}, {b}): composer={exp} vivace={ours}");
            }
        }
    }
    assert_eq!(bad, 0, "{bad} comparaisons divergentes");
    let mut bad = 0;
    let mut idx = 0;
    for (i, c) in csample.iter().enumerate() {
        let parsed = parse_constraints(c).ok();
        for k in 0..5 {
            let v = sample[(i * 11 + k * 37) % sample.len()];
            let exp = expected["matches"][idx].as_bool();
            idx += 1;
            let ours = parsed.as_ref().map(|p| p.constraint.matches_version(v));
            if ours != exp {
                bad += 1;
                if bad <= 10 {
                    eprintln!("matches({c:?}, {v}): composer={exp:?} vivace={ours:?}");
                }
            }
        }
    }
    assert_eq!(bad, 0, "{bad} matches divergents");
    eprintln!(
        "oracle semver: {} contraintes, {} versions, {} comparaisons, {} matches — 0 divergence",
        constraints.len(),
        versions.len(),
        sample.len(),
        idx
    );
}

/// Intervals : `isSubsetOf` et `compactConstraint` sur des paires de
/// contraintes du corpus (échantillon déterministe), contre le phar.
#[test]
fn intervals_match_composer() {
    use vivace_resolver::intervals::{compact_constraint, is_subset_of};
    let phar = phar();
    let (constraints, _) = corpus();
    let step = std::cmp::max(1, constraints.len() / 250);
    let sample: Vec<&String> = constraints.iter().step_by(step).collect();
    let mut pairs: Vec<Value> = Vec::new();
    for (i, a) in sample.iter().enumerate() {
        let b = sample[(i * 13 + 5) % sample.len()];
        pairs.push(serde_json::json!([a, b]));
        pairs.push(serde_json::json!([a, a]));
    }
    let script = format!(
        r#"require "phar://{}/vendor/autoload.php";
           use Composer\Semver\Constraint\MultiConstraint;
           use Composer\Semver\Intervals;
           $p = new \Composer\Semver\VersionParser();
           $out = [];
           foreach (json_decode(stream_get_contents(STDIN), true) as [$a, $b]) {{
               try {{
                   $ca = $p->parseConstraints($a); $cb = $p->parseConstraints($b);
                   $out[] = [
                       Intervals::isSubsetOf($ca, $cb),
                       (string) Intervals::compactConstraint(new MultiConstraint([$ca, $cb], false)),
                       (string) Intervals::compactConstraint(new MultiConstraint([$ca, $cb], true)),
                   ];
               }} catch (\Throwable $e) {{ $out[] = null; }}
           }}
           echo json_encode($out);"#,
        phar.display()
    );
    let expected = php(&script, &Value::Array(pairs.clone()));
    let mut bad = 0;
    for (pair, exp) in pairs.iter().zip(expected.as_array().expect("array")) {
        let a = pair[0].as_str().expect("a");
        let b = pair[1].as_str().expect("b");
        let (Ok(ca), Ok(cb)) = (parse_constraints(a), parse_constraints(b)) else {
            assert!(exp.is_null(), "{a:?}/{b:?}: composer a parsé, pas nous");
            continue;
        };
        let (ca, cb) = (ca.constraint, cb.constraint);
        let ours = serde_json::json!([
            is_subset_of(&ca, &cb),
            compact_constraint(&vivace_resolver::constraint::Constraint::Multi {
                constraints: vec![ca.clone(), cb.clone()],
                conjunctive: false
            })
            .to_string(),
            compact_constraint(&vivace_resolver::constraint::Constraint::Multi {
                constraints: vec![ca.clone(), cb.clone()],
                conjunctive: true
            })
            .to_string(),
        ]);
        if &ours != exp {
            bad += 1;
            if bad <= 12 {
                eprintln!("intervals({a:?}, {b:?}): composer={exp} vivace={ours}");
            }
        }
    }
    assert_eq!(bad, 0, "{bad} paires divergentes sur {}", pairs.len());
    eprintln!("oracle intervals: {} paires — 0 divergence", pairs.len());
}
