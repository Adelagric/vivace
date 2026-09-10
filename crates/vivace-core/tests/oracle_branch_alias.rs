//! Différentiel de branch_alias_of contre ArrayLoader::getBranchAlias du phar :
//! pour chaque config de paquet, Composer produit (ou non) un AliasPackage ;
//! on compare (version, pretty_version) de l'alias.

use serde_json::{json, Value};
use std::io::Write as _;
use std::process::{Command, Stdio};

fn cases() -> Vec<Value> {
    vec![
        json!({"version": "dev-main", "default-branch": true}),
        json!({"version": "dev-main"}),
        json!({"version": "dev-main", "default-branch": false}),
        json!({"version": "dev-feature/x", "default-branch": true}),
        json!({"version": "1.x-dev", "default-branch": true}),
        json!({"version": "v1.x-dev", "default-branch": true}),
        json!({"version": "1.0.0-dev", "default-branch": true}),
        json!({"version": "2.0.0", "default-branch": true}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.x-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.0-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.0.x-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "0.11.x-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-master": "1.x-dev"}}}),
        json!({"version": "dev-Main", "extra": {"branch-alias": {"dev-main": "1.x-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "9999999-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "foo-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "foo-dev"}}, "default-branch": true}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.0"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.0"}}, "default-branch": true}),
        json!({"version": "1.x-dev", "extra": {"branch-alias": {"1.x-dev": "1.2.x-dev"}}}),
        json!({"version": "1.x-dev", "extra": {"branch-alias": {"1.x-dev": "2.x-dev"}}}),
        json!({"version": "1.x-dev", "extra": {"branch-alias": {"1.x-dev": "2.x-dev"}}, "default-branch": true}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-other": "2.x-dev", "dev-main": "3.x-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.x-DEV"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.X-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "v2.x-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.2.3.4-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": {"dev-main": "1.2.3.4.5-dev"}}}),
        json!({"version": "dev-main", "extra": {"branch-alias": []}, "default-branch": true}),
        json!({"version": "dev-main", "extra": {"branch-alias": "1.x-dev"}, "default-branch": true}),
        json!({"version": "1.0.0", "extra": {"branch-alias": {"1.0.0": "1.x-dev"}}}),
    ]
}

#[test]
fn matches_array_loader_branch_alias() {
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
           $l = new \Composer\Package\Loader\ArrayLoader();
           $out = [];
           foreach (json_decode(stream_get_contents(STDIN), true) as $cfg) {{
               $cfg += ['name' => 'a/b'];
               try {{
                   $p = $l->load($cfg);
                   $out[] = $p instanceof \Composer\Package\AliasPackage
                       ? [$p->getVersion(), $p->getPrettyVersion()] : null;
               }} catch (\Throwable $e) {{ $out[] = ['error', $e->getMessage()]; }}
           }}
           echo json_encode($out);"#,
        phar.display()
    );
    let cases = cases();
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
        .write_all(serde_json::to_string(&cases).expect("json").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php");
    assert!(out.status.success());
    let rows: Vec<Option<(String, String)>> = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(rows.len(), cases.len());
    for (cfg, expected) in cases.iter().zip(rows) {
        let version = cfg["version"].as_str().expect("version");
        let default_branch = cfg
            .get("default-branch")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ours =
            vivace_core::root_version::branch_alias_of(version, cfg.get("extra"), default_branch);
        assert_eq!(ours, expected, "divergence sur {cfg}");
    }
}
