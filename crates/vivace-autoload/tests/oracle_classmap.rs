//! Différentiel de la détection de classes contre l'oracle réel :
//! `Composer\ClassMapGenerator\PhpFileParser::findClasses` (via le phar), sur
//! TOUS les fichiers .php/.inc des vendors des fixtures laravel et sylius
//! (≈ 50k fichiers, un seul process PHP). Toute divergence est listée.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fixture_vendor(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/work")
        .join(name)
        .join("vendor");
    assert!(
        dir.is_dir(),
        "fixture {name} absente — lancer fixtures/make.sh"
    );
    dir
}

fn php_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("php") | Some("inc")
            )
        })
        .collect()
}

fn oracle(files: &[PathBuf]) -> std::collections::BTreeMap<String, Option<Vec<String>>> {
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
           $out = [];
           foreach (json_decode(stream_get_contents(STDIN), true) as $f) {{
               try {{ $out[$f] = array_map('base64_encode', \Composer\ClassMapGenerator\PhpFileParser::findClasses($f)); }}
               catch (\Throwable $e) {{ $out[$f] = null; }}
           }}
           echo json_encode($out, JSON_INVALID_UTF8_SUBSTITUTE);"#,
        phar.display()
    );
    let mut child = Command::new("php")
        .args([
            "-d",
            "error_reporting=0",
            "-d",
            "memory_limit=1G",
            "-r",
            &script,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("php requis");
    let list: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(serde_json::to_string(&list).expect("json").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php");
    assert!(
        out.status.success(),
        "oracle en échec: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.stdout.is_empty(),
        "oracle muet, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("json oracle")
}

#[test]
fn find_classes_matches_oracle_on_fixtures() {
    let mut files = php_files(&fixture_vendor("laravel"));
    files.extend(php_files(&fixture_vendor("sylius")));
    files.sort();
    files.dedup();
    assert!(
        files.len() > 20_000,
        "trop peu de fichiers: {}",
        files.len()
    );

    let expected = oracle(&files);
    let finder = vivace_autoload::classmap::ClassFinder::new().expect("regex");
    let mut diverging = Vec::new();
    let mut compared = 0usize;
    for f in &files {
        let key = f.to_string_lossy().into_owned();
        let Some(Some(exp)) = expected.get(&key) else {
            continue; // l'oracle a levé (fichier illisible/binaire) : non comparé
        };
        let contents = std::fs::read(f).expect("read");
        let ours: Vec<String> = finder
            .find_classes(&contents)
            .expect("find")
            .iter()
            .map(|c| {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(c)
            })
            .collect();
        compared += 1;
        if &ours != exp {
            diverging.push(format!("{key}\n   nous:   {ours:?}\n   oracle: {exp:?}"));
        }
    }
    assert!(compared > 20_000, "trop peu comparés: {compared}");
    assert!(
        diverging.is_empty(),
        "{} divergences sur {compared} fichiers:\n{}",
        diverging.len(),
        diverging
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
