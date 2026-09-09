//! Différentiel du sous-ensemble de contraintes contre l'oracle
//! `Composer\Semver\Semver::satisfies` du phar réel : toute paire
//! (version, contrainte) que vivace accepte de parser doit donner le même
//! verdict que Composer. Les paires que vivace refuse (hors sous-ensemble)
//! sont comptées mais pas comparées — le refus explicite est un comportement
//! prévu, le mensonge ne l'est pas.

use std::io::Write as _;
use std::process::{Command, Stdio};

const VERSIONS: &[&str] = &[
    "0.0.1",
    "0.3.0",
    "0.3.7",
    "0.4.0",
    "1.0.0",
    "1.0.1",
    "1.2.0",
    "1.2.3",
    "1.2.9",
    "1.3.0",
    "1.9.9",
    "2.0.0",
    "2.0.1",
    "2.1.0",
    "3.0.0",
    "7.4.33",
    "8.0.0",
    "8.0.2",
    "8.1.0",
    "8.1.1",
    "8.2.29",
    "8.5.10",
    "9.0.0",
    "10.1.2",
    "1.0.0-dev",
    "1.0.0-alpha1",
    "1.0.0-beta2",
    "1.0.0-RC1",
    "2.0.0-beta1",
    "8.1.0-RC3",
    "9.0.0-alpha1",
    "1.2.3.4",
    "0.0.0",
];

const CONSTRAINTS: &[&str] = &[
    "*",
    "8.1",
    "=8.1",
    "==8.1.0",
    "!=1.2.3",
    ">=8.1",
    ">8.1",
    "<2.0",
    "<=2.0",
    ">=1.0 <2.0",
    ">=1.0,<2.0",
    ">= 1.0 , < 2.0",
    "^8.1",
    "^0.3",
    "^0.0.3",
    "^1.2.3",
    "^7.4|^8.0",
    "^7.4||^8.0",
    "~1.2",
    "~1.2.3",
    "~0.3",
    "1.2.*",
    "8.*",
    "0.*",
    "1.0 - 2.0",
    "1.0.0 - 2.0.0",
    "1.0 - 2",
    ">=8.1.0-beta1",
    "<2.0.0-beta1",
    "^8.1.0-RC1",
    ">=0.3 <0.4 || ^1.0",
];

fn oracle_matrix() -> Vec<(String, String, bool)> {
    let phar = std::env::temp_dir().join("vivace-oracle-composer.phar");
    if !phar.exists() {
        let src = String::from_utf8(
            Command::new("which")
                .arg("composer")
                .output()
                .expect("which composer")
                .stdout,
        )
        .expect("utf8");
        assert!(
            !src.trim().is_empty(),
            "composer requis (brew install composer)"
        );
        std::fs::copy(src.trim(), &phar).expect("copie du phar");
    }
    let script = format!(
        r#"require "phar://{}/vendor/autoload.php";
           $in = json_decode(stream_get_contents(STDIN), true);
           $out = [];
           foreach ($in["versions"] as $v) foreach ($in["constraints"] as $c) {{
               try {{ $r = \Composer\Semver\Semver::satisfies($v, $c) ? 1 : 0; }}
               catch (\Throwable $e) {{ $r = -1; }}
               $out[] = [$v, $c, $r];
           }}
           echo json_encode($out);"#,
        phar.display()
    );
    let mut child = Command::new("php")
        .args(["-d", "error_reporting=0", "-r", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("php requis (brew install php)");
    let payload = serde_json::json!({ "versions": VERSIONS, "constraints": CONSTRAINTS });
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php exit");
    assert!(out.status.success(), "oracle en échec: {out:?}");
    let rows: Vec<(String, String, i32)> =
        serde_json::from_slice(&out.stdout).expect("json oracle");
    rows.into_iter()
        .filter(|(_, _, r)| *r >= 0)
        .map(|(v, c, r)| (v, c, r == 1))
        .collect()
}

#[test]
fn matches_semver_satisfies() {
    let matrix = oracle_matrix();
    assert!(
        matrix.len() > 900,
        "matrice oracle trop petite: {}",
        matrix.len()
    );
    let mut compared = 0usize;
    let mut refused = 0usize;
    let mut diverging = Vec::new();
    for (version, constraint, expected) in &matrix {
        match vivace_core::constraint::satisfies(version, constraint) {
            Ok(actual) => {
                compared += 1;
                if actual != *expected {
                    diverging.push(format!(
                        "satisfies({version:?}, {constraint:?}) = {actual}, oracle dit {expected}"
                    ));
                }
            }
            Err(_) => refused += 1,
        }
    }
    assert!(
        diverging.is_empty(),
        "{} divergences (sur {compared} comparées):\n{}",
        diverging.len(),
        diverging.join("\n")
    );
    // Le refus doit rester l'exception dans ce sous-ensemble choisi pour être couvert.
    assert!(
        refused * 20 <= compared,
        "trop de refus: {refused} refusées vs {compared} comparées"
    );
}
