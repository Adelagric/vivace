//! Tests différentiels du content-hash contre l'oracle Composer réel.
//!
//! Deux niveaux :
//! 1. Golden : le hash calculé sur le composer.json de chaque fixture doit être
//!    exactement le `content-hash` de son composer.lock (écrit par Composer).
//! 2. Oracle vivant : pour une batterie de manifestes retors, comparer au
//!    résultat de `Locker::getContentHash` exécuté via le phar Composer.
//!
//! Prérequis (environnement de dev/CI, cf. fixtures/make.sh) : fixtures créées,
//! `php` + `composer` installés. Si absents, le test ÉCHOUE avec un message
//! explicite — pas de skip silencieux.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures_dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/work");
    assert!(
        root.is_dir(),
        "fixtures absentes ({}) — lancer fixtures/make.sh d'abord",
        root.display()
    );
    root
}

#[test]
fn golden_fixture_locks() {
    for fx in ["laravel", "symfony", "sylius"] {
        let dir = fixtures_dir().join(fx);
        let json = std::fs::read_to_string(dir.join("composer.json")).expect("composer.json");
        let lock: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("composer.lock")).expect("composer.lock"),
        )
        .expect("lock JSON");
        let expected = lock["content-hash"].as_str().expect("content-hash");
        let actual = vivace_core::content_hash::content_hash(&json).expect("hash");
        assert_eq!(actual, expected, "content-hash divergent pour {fx}");
    }
}

/// Invoque Locker::getContentHash du phar Composer sur un manifeste donné.
fn oracle_hash(manifest: &str) -> String {
    let phar = which_composer_phar();
    let script = format!(
        r#"require "phar://{}/vendor/autoload.php";
           echo \Composer\Package\Locker::getContentHash(stream_get_contents(STDIN));"#,
        phar.display()
    );
    let mut child = Command::new("php")
        .args(["-d", "error_reporting=0", "-r", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("php doit être installé (brew install php)");
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(manifest.as_bytes())
        .expect("write manifest");
    let out = child.wait_with_output().expect("php exit");
    assert!(out.status.success(), "oracle PHP en échec: {out:?}");
    String::from_utf8(out.stdout).expect("utf8")
}

/// Le phar est le binaire `composer` du PATH, copié sous extension .phar
/// (le stream phar:// exige l'extension).
fn which_composer_phar() -> PathBuf {
    let target = std::env::temp_dir().join("vivace-oracle-composer.phar");
    if !target.exists() {
        let src = String::from_utf8(
            Command::new("which")
                .arg("composer")
                .output()
                .expect("which composer")
                .stdout,
        )
        .expect("utf8");
        let src = src.trim();
        assert!(
            !src.is_empty(),
            "composer doit être installé (brew install composer)"
        );
        std::fs::copy(src, &target).expect("copie du phar");
    }
    target
}

#[test]
fn differential_against_php_oracle() {
    let manifests = [
        // Vide et minimal.
        "{}",
        r#"{"require":{}}"#,
        // Clés pertinentes vs ignorées, ordre non trié.
        r#"{"extra":{"a":1},"name":"v/x","description":"ignorée","require":{"php":"^8.2"}}"#,
        // Unicode, slashes, caractères spéciaux.
        r#"{"name":"vendé/tôt","require":{"a/b":"^1.0"},"extra":{"url":"https://ex.com/p?q=1&r=2","emoji":"🎼","quote":"a\"b\\c"}}"#,
        // Objets vides et pseudo-listes (quirk assoc PHP).
        r#"{"require":{},"extra":{"empty":{},"list":{"0":"a","1":"b"},"gap":{"0":"a","2":"b"},"rev":{"1":"a","0":"b"}}}"#,
        // Nombres : entiers, flottants, flottant entier, négatifs, grands.
        r#"{"extra":{"i":42,"f":1.5,"fi":1.0,"neg":-3,"big":9007199254740993,"tiny":1.0e-7}}"#,
        // Frontières du formatage double de PHP (fixe vs exponentiel) et
        // entier > PHP_INT_MAX (devient float au decode).
        r#"{"extra":{"a":0.0001,"b":1.0e-5,"c":9.9e16,"d":1.0e17,"e":1.23e17,"f":-0.0,"g":5.0e-324,"h":1.7976931348623157e308,"j":12345678901234567890,"k":1.0e21,"l":123456.789}}"#,
        // config.platform re-nesté + repositories.
        r#"{"config":{"platform":{"php":"8.2.1"},"sort-packages":true},"repositories":[{"type":"vcs","url":"https://github.com/a/b"}]}"#,
        // prefer-stable / minimum-stability / version.
        r#"{"version":"1.2.3","minimum-stability":"dev","prefer-stable":true,"provide":{"x/y":"*"},"replace":{"z/w":"self.version"},"conflict":{"c/d":"<2.0"}}"#,
        // Clés imbriquées non triées (seul le premier niveau est ksorté).
        r#"{"require":{"zzz/a":"1","aaa/b":"2"},"extra":{"z":1,"a":2}}"#,
        // Booleans et null dans extra.
        r#"{"extra":{"t":true,"f":false,"n":null,"nested":[1,[2,3],{"k":"v"}]}}"#,
        // Régression proptest 2026-09-09 : divergence d'1 ULP au parsing des
        // floats sans la feature serde_json `float_roundtrip`.
        r#"{"extra":{"ulp":-1.0287745609898322e+201}}"#,
    ];
    for m in manifests {
        let ours = vivace_core::content_hash::content_hash(m).expect("hash");
        let theirs = oracle_hash(m);
        assert_eq!(ours, theirs, "divergence oracle sur: {m}");
    }
}
