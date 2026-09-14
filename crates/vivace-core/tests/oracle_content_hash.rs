//! Differential tests of the content-hash against the real Composer oracle.
//!
//! Two levels:
//! 1. Golden: the hash computed over each fixture's composer.json must be
//!    exactly the `content-hash` of its composer.lock (written by Composer).
//! 2. Live oracle: for a battery of tricky manifests, compare with the result
//!    of `Locker::getContentHash` run through the Composer phar.
//!
//! Prerequisites (dev/CI environment, cf. fixtures/make.sh): fixtures created,
//! `php` + `composer` installed. If missing, the test FAILS with an explicit
//! message; no silent skip.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures_dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/work");
    assert!(
        root.is_dir(),
        "fixtures missing ({}) — run fixtures/make.sh first",
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
        assert_eq!(actual, expected, "diverging content-hash for {fx}");
    }
}

/// Invokes the Composer phar's Locker::getContentHash on a given manifest.
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
        .expect("php must be installed (brew install php)");
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(manifest.as_bytes())
        .expect("write manifest");
    let out = child.wait_with_output().expect("php exit");
    assert!(out.status.success(), "PHP oracle failed: {out:?}");
    String::from_utf8(out.stdout).expect("utf8")
}

/// The phar is the `composer` binary from the PATH, copied under a .phar
/// extension (the phar:// stream requires the extension).
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
            "composer must be installed (brew install composer)"
        );
        std::fs::copy(src, &target).expect("phar copy");
    }
    target
}

#[test]
fn differential_against_php_oracle() {
    let manifests = [
        // Empty and minimal.
        "{}",
        r#"{"require":{}}"#,
        // Relevant vs ignored keys, unsorted order.
        r#"{"extra":{"a":1},"name":"v/x","description":"ignorée","require":{"php":"^8.2"}}"#,
        // Unicode, slashes, special characters.
        r#"{"name":"vendé/tôt","require":{"a/b":"^1.0"},"extra":{"url":"https://ex.com/p?q=1&r=2","emoji":"🎼","quote":"a\"b\\c"}}"#,
        // Empty objects and pseudo-lists (PHP assoc quirk).
        r#"{"require":{},"extra":{"empty":{},"list":{"0":"a","1":"b"},"gap":{"0":"a","2":"b"},"rev":{"1":"a","0":"b"}}}"#,
        // Numbers: integers, floats, integral float, negatives, large ones.
        r#"{"extra":{"i":42,"f":1.5,"fi":1.0,"neg":-3,"big":9007199254740993,"tiny":1.0e-7}}"#,
        // Boundaries of PHP's double formatting (fixed vs exponential) and
        // integer > PHP_INT_MAX (becomes a float at decode time).
        r#"{"extra":{"a":0.0001,"b":1.0e-5,"c":9.9e16,"d":1.0e17,"e":1.23e17,"f":-0.0,"g":5.0e-324,"h":1.7976931348623157e308,"j":12345678901234567890,"k":1.0e21,"l":123456.789}}"#,
        // Re-nested config.platform + repositories.
        r#"{"config":{"platform":{"php":"8.2.1"},"sort-packages":true},"repositories":[{"type":"vcs","url":"https://github.com/a/b"}]}"#,
        // prefer-stable / minimum-stability / version.
        r#"{"version":"1.2.3","minimum-stability":"dev","prefer-stable":true,"provide":{"x/y":"*"},"replace":{"z/w":"self.version"},"conflict":{"c/d":"<2.0"}}"#,
        // Unsorted nested keys (only the top level is ksorted).
        r#"{"require":{"zzz/a":"1","aaa/b":"2"},"extra":{"z":1,"a":2}}"#,
        // Booleans and null in extra.
        r#"{"extra":{"t":true,"f":false,"n":null,"nested":[1,[2,3],{"k":"v"}]}}"#,
        // proptest regression 2026-09-09: 1 ULP divergence when parsing
        // floats without the serde_json `float_roundtrip` feature.
        r#"{"extra":{"ulp":-1.0287745609898322e+201}}"#,
    ];
    for m in manifests {
        let ours = vivace_core::content_hash::content_hash(m).expect("hash");
        let theirs = oracle_hash(m);
        assert_eq!(ours, theirs, "oracle divergence on: {m}");
    }
}
