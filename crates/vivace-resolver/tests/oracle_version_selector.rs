//! `VersionSelector` oracle: for each package name of a snapshot (plus the
//! platform packages, present or not; a name unknown to the repository is a
//! transport error over `file://`, hence outside the corpus), what
//! `composer require <name>` without constraint would pick
//! (`findBestCandidate` (project's preferred stability, platform filter,
//! warnings) and `findRecommendedRequireVersion`) by the phar
//! (tools/oracle-version-selector.php) and by the port, on the same
//! snapshot and the same PHP platform.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use vivace_resolver::platform_filter::PlatformRequirementFilter;
use vivace_resolver::session::{UpdateOptions, UpdateSession};
use vivace_resolver::version_selector::{
    find_best_candidate, find_recommended_require_version, platform_constraints,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("root")
}

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
        assert!(!src.trim().is_empty(), "composer required");
        std::fs::copy(src.trim(), &phar).expect("copy");
    }
    phar
}

struct Setup {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
    root_version: String,
}

fn setup(fx: &str) -> Setup {
    let root = root();
    let tmp = tempfile::tempdir().expect("tmp");
    let reg = tmp.path().join("registry");
    std::fs::create_dir_all(&reg).expect("mkdir");
    let archive = root.join("fixtures/registry").join(format!("{fx}.tar.gz"));
    assert!(
        Command::new("tar")
            .args([
                "-C",
                &reg.to_string_lossy(),
                "-xzf",
                &archive.to_string_lossy()
            ])
            .status()
            .expect("tar")
            .success(),
        "{fx}: archive"
    );
    // As in harness/lib/registry.sh: the blocking policies declared as on
    // Packagist (partial advisories in the p2 files, empty malware summary
    // if the snapshot has none).
    if !reg.join("summary.json").is_file() {
        std::fs::write(
            reg.join("summary.json"),
            "{\"filter\": {\"malware\": {}}}\n",
        )
        .expect("summary.json");
    }
    std::fs::write(
        reg.join("packages.json"),
        format!(
            "{{\"packages\": [], \"notify-batch\": \"https://packagist.org/downloads/\", \"metadata-url\": \"file://{r}/p2/%package%.json\", \"available-package-patterns\": [\"*\"], \"security-advisories\": {{\"metadata\": true, \"api-url\": null}}, \"filter\": {{\"metadata\": true, \"lists\": {{\"malware\": {{\"enabled\": true}}}}, \"summary-url\": \"file://{r}/summary.json\"}}}}\n",
            r = reg.display()
        ),
    )
    .expect("packages.json");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join("config.json"),
        format!(
            "{{\"repositories\": {{\"snapshot\": {{\"type\": \"composer\", \"url\": \"file://{}\"}}, \"packagist.org\": false}}}}\n",
            reg.display()
        ),
    )
    .expect("config.json");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("project");
    let src = root.join("fixtures/projects").join(fx);
    let mut manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(src.join("composer.json")).expect("composer.json"),
    )
    .expect("json");
    // The manifest's remote repositories (packages.drupal.org) are not
    // reachable offline: the oracle and the port read only the snapshot.
    if let Some(obj) = manifest.as_object_mut() {
        obj.remove("repositories");
    }
    std::fs::write(
        project.join("composer.json"),
        serde_json::to_string_pretty(&manifest).expect("json"),
    )
    .expect("write");
    if src.join("composer.lock").is_file() {
        std::fs::copy(src.join("composer.lock"), project.join("composer.lock")).expect("lock");
    }
    Setup {
        _tmp: tmp,
        project,
        home,
        root_version: if fx == "rector" {
            "dev-main".to_owned()
        } else {
            String::new()
        },
    }
}

/// All package names of the snapshot (`p2/vendor/name.json` files).
fn snapshot_names(s: &Setup) -> Vec<String> {
    let p2 = s.project.parent().expect("tmp").join("registry/p2");
    let mut names = BTreeSet::new();
    for vendor in std::fs::read_dir(&p2).expect("p2").flatten() {
        if !vendor.path().is_dir() {
            continue;
        }
        let v = vendor.file_name().to_string_lossy().into_owned();
        for f in std::fs::read_dir(vendor.path()).expect("vendor").flatten() {
            let n = f.file_name().to_string_lossy().into_owned();
            if n.starts_with("._") {
                continue;
            }
            if let Some(stem) = n.strip_suffix(".json") {
                names.insert(format!("{v}/{}", stem.trim_end_matches("~dev")));
            }
        }
    }
    names.into_iter().collect()
}

fn oracle(s: &Setup, names: &[String], filter_args: &[&str]) -> Value {
    let mut child = Command::new("php")
        .arg(root().join("tools/oracle-version-selector.php"))
        .arg(phar())
        .args(filter_args)
        .current_dir(&s.project)
        .env("COMPOSER_HOME", &s.home)
        .env("COMPOSER_CACHE_DIR", s.home.join("cache"))
        .env("COMPOSER_ROOT_VERSION", &s.root_version)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("php");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(serde_json::to_string(names).expect("json").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php");
    assert!(
        out.status.success(),
        "oracle: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("oracle json: {e}\n{}", String::from_utf8_lossy(&out.stdout)))
}

fn port(s: &Setup, names: &[String], filter: &PlatformRequirementFilter) -> Value {
    let mut session = UpdateSession::prepare_update(
        &s.project,
        Some(&s.home),
        true,
        None,
        None,
        &UpdateOptions::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let preferred = if session.root.prefer_stable {
        "stable".to_owned()
    } else {
        session.root.minimum_stability.clone()
    };
    let platform = platform_constraints(&session.platform, &session.arena);
    let php_version = session.php_version.clone();
    let mut out = serde_json::Map::new();
    for name in names {
        let candidates = session
            .find_packages_for_require(name, false)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut warnings = Vec::new();
        let best = find_best_candidate(
            &candidates,
            &session.arena,
            &preferred,
            filter,
            &platform,
            &mut warnings,
        );
        // All stabilities: `null` when a `~dev` file is missing from the
        // snapshot (the oracle does the same).
        let any_best = match session.find_packages_for_require(name, true) {
            Ok(any) => Some(
                find_best_candidate(
                    &any,
                    &session.arena,
                    &preferred,
                    filter,
                    &platform,
                    &mut Vec::new(),
                )
                .is_some(),
            ),
            Err(_) => None,
        };
        let mut entry = json!({
            "found": best.is_some(),
            "warnings": warnings.iter().filter(|w| !w.verbose).map(|w| w.message.clone()).collect::<Vec<_>>(),
            "found_any_stability": any_best,
        });
        if let Some(idx) = best {
            let p = &session.arena[idx];
            entry["name"] = json!(p.pretty_name);
            entry["version"] = json!(p.version);
            entry["pretty_version"] = json!(p.pretty_version);
            entry["recommended"] = json!(find_recommended_require_version(
                p,
                &session.arena,
                &php_version
            ));
        }
        out.insert(name.clone(), entry);
    }
    Value::Object(out)
}

/// Project stability variant: `minimum-stability` and `prefer-stable`
/// rewritten in the manifest copy; only the names whose snapshot also has
/// the `~dev` file are played (the others are a transport error on both
/// sides).
fn check_stability(fx: &str, minimum_stability: &str, prefer_stable: bool) -> usize {
    let s = setup(fx);
    let path = s.project.join("composer.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("manifest")).expect("json");
    manifest["minimum-stability"] = json!(minimum_stability);
    manifest["prefer-stable"] = json!(prefer_stable);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&manifest).expect("json"),
    )
    .expect("write");
    let p2 = s.project.parent().expect("tmp").join("registry/p2");
    let names: Vec<String> = snapshot_names(&s)
        .into_iter()
        .filter(|n| p2.join(format!("{n}~dev.json")).is_file())
        .collect();
    assert!(names.len() > 20, "{fx}: {} names with ~dev", names.len());
    let expected = oracle(&s, &names, &[]);
    let got = port(&s, &names, &PlatformRequirementFilter::IgnoreNothing);
    let failures: Vec<String> = names
        .iter()
        .filter(|n| expected[n.as_str()] != got[n.as_str()])
        .map(|n| {
            format!(
                "{n}\n  php : {}\n  rust: {}",
                expected[n.as_str()],
                got[n.as_str()]
            )
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{fx} {minimum_stability}/{prefer_stable}: {} divergence(s) over {} names:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
    if std::env::var_os("VIVACE_ORACLE_STATS").is_some() {
        let recs: Vec<&str> = expected
            .as_object()
            .expect("map")
            .values()
            .filter_map(|e| e["recommended"].as_str())
            .collect();
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        for r in &recs {
            let kind = if r.starts_with("dev-") {
                "dev-*".to_owned()
            } else if r.contains('@') {
                format!("^…@{}", r.rsplit('@').next().unwrap_or(""))
            } else if r.starts_with('^') {
                "^x.y".to_owned()
            } else {
                r.to_string()
            };
            *kinds.entry(kind).or_default() += 1;
        }
        eprintln!(
            "{fx} {minimum_stability}/{prefer_stable}: {} names, {kinds:?}",
            names.len()
        );
    }
    names.len()
}

fn check(fx: &str, filter_args: &[&str], filter: &PlatformRequirementFilter) -> usize {
    let s = setup(fx);
    let mut names = snapshot_names(&s);
    assert!(names.len() > 50, "{fx}: {} names", names.len());
    // A name in a different case: `findPackages(strtolower($name))`.
    let mixed_case = names[0].to_uppercase();
    names.push(mixed_case);
    names.extend(
        [
            "php",
            "php-64bit",
            "ext-json",
            "ext-mbstring",
            "ext-nope",
            "lib-icu",
            "composer-plugin-api",
            "composer-runtime-api",
        ]
        .map(str::to_owned),
    );
    let expected = oracle(&s, &names, filter_args);
    let got = port(&s, &names, filter);
    if std::env::var_os("VIVACE_ORACLE_STATS").is_some() {
        let rec = |f: &dyn Fn(&str) -> bool| {
            expected
                .as_object()
                .expect("map")
                .values()
                .filter(|e| e["recommended"].as_str().is_some_and(f))
                .count()
        };
        eprintln!(
            "{fx}: {} found, {} with a warning, {} `@`, {} `dev-`, {} `^0.`, {} `*`",
            expected
                .as_object()
                .expect("map")
                .values()
                .filter(|e| e["found"] == true)
                .count(),
            expected
                .as_object()
                .expect("map")
                .values()
                .filter(|e| !e["warnings"].as_array().is_none_or(Vec::is_empty))
                .count(),
            rec(&|r| r.contains('@')),
            rec(&|r| r.starts_with("dev-")),
            rec(&|r| r.starts_with("^0.")),
            rec(&|r| r == "*"),
        );
    }
    let mut failures = Vec::new();
    for name in &names {
        if expected[name] != got[name] {
            failures.push(format!(
                "{name}\n  php : {}\n  rust: {}",
                expected[name], got[name]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{fx} {filter_args:?}: {} divergence(s) over {} names:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
    names.len()
}

#[test]
fn best_candidates_match_composer_on_snapshots() {
    let mut total = 0;
    for fx in ["laravel", "symfony", "sylius", "rector", "drupal"] {
        if !root()
            .join("fixtures/registry")
            .join(format!("{fx}.tar.gz"))
            .is_file()
        {
            continue;
        }
        total += check(fx, &[], &PlatformRequirementFilter::IgnoreNothing);
    }
    assert!(total > 0, "no snapshot");
    eprintln!("{total} names");
}

#[test]
fn best_candidates_match_composer_with_platform_filters() {
    let s = ["php", "ext-*", "lib-icu+"].map(str::to_owned);
    check(
        "laravel",
        &["--ignore-platform-reqs"],
        &PlatformRequirementFilter::IgnoreAll,
    );
    check(
        "sylius",
        &[
            "--ignore-platform-req=php",
            "--ignore-platform-req=ext-*",
            "--ignore-platform-req=lib-icu+",
        ],
        &PlatformRequirementFilter::from_list(&s),
    );
}

#[test]
fn best_candidates_match_composer_with_dev_stability() {
    check_stability("rector", "dev", false);
    check_stability("rector", "dev", true);
    check_stability("rector", "beta", false);
}
