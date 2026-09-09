//! Différentiel de normalize_pretty contre VersionParser::normalize du phar.

use std::io::Write as _;
use std::process::{Command, Stdio};

const VERSIONS: &[&str] = &[
    "1.0",
    "v2.0.4",
    "0.18.0",
    "1.0-beta2",
    "1.0.0rc1",
    "1.0.0RC1",
    "2.0b1",
    "1.2.3.4",
    "3.0.1-patch2",
    "dev-main",
    "dev-feature/x",
    "2.x-dev",
    "1.2.x-dev",
    "1.0.0-alpha",
    "1.0.0-stable",
    "10.20.30",
    "0.0.1",
    "1.0.0-dev",
    "4.2.1-p1",
    "1.0.0-a5",
    "V3.1",
];

#[test]
fn matches_version_parser_normalize() {
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
    let script = format!(
        r#"require "phar://{}/vendor/autoload.php";
           $p = new \Composer\Semver\VersionParser();
           $out = [];
           foreach (json_decode(stream_get_contents(STDIN), true) as $v) {{
               try {{ $out[] = [$v, $p->normalize($v)]; }}
               catch (\Throwable $e) {{ $out[] = [$v, null]; }}
           }}
           echo json_encode($out);"#,
        phar.display()
    );
    let mut child = Command::new("php")
        .args(["-d", "error_reporting=0", "-r", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("php requis");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(serde_json::to_string(VERSIONS).expect("json").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php");
    assert!(out.status.success());
    let rows: Vec<(String, Option<String>)> = serde_json::from_slice(&out.stdout).expect("json");
    for (input, expected) in rows {
        let ours = vivace_core::version::normalize_pretty(&input).ok();
        match expected {
            Some(exp) => assert_eq!(
                ours.as_deref(),
                Some(exp.as_str()),
                "divergence sur {input:?}"
            ),
            None => assert_eq!(ours, None, "on accepte {input:?} que l'oracle refuse"),
        }
    }
}
