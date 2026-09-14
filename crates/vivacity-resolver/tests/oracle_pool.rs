//! Pool oracle: for each snapshot (fixtures/registry/*.tar.gz), the pool
//! built by Composer (tools/oracle-pool.php, without optimizer or filters)
//! and the one built by vivacity-resolver must be identical package by
//! package, in order: name, version, origin repository, alias, references,
//! links. The local repository is injected through COMPOSER_HOME/config.json
//! as in harness/update.sh.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use vivacity_resolver::package::{Links, Origin, Package};
use vivacity_resolver::session::UpdateSession;

const FIXTURES: &[&str] = &[
    "laravel",
    "symfony",
    "sylius",
    "rector",
    "drupal",
    "solver-backtrack",
    "solver-conflict",
    "solver-aliases",
    "solver-providers",
    "solver-policies",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("root")
}

fn phar() -> PathBuf {
    let phar = std::env::temp_dir().join("vivacity-oracle-composer.phar");
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

fn links(l: &Links) -> Value {
    // PHP array: the link key, or numbers 0.. for keyless entries
    // (array_merge). Empty -> `[]` in json_encode.
    if l.is_empty() {
        return Value::Array(Vec::new());
    }
    let mut out = Map::new();
    let mut n = 0;
    for link in l.iter() {
        let key = match &link.key {
            Some(k) => k.clone(),
            None => {
                n += 1;
                (n - 1).to_string()
            }
        };
        out.insert(key, Value::String(link.pretty_constraint.clone()));
    }
    Value::Object(out)
}

fn entry(arena: &[Package], idx: usize) -> Value {
    let p = &arena[idx];
    json!({
        "name": p.name,
        "version": p.version,
        "pretty": p.pretty_version,
        "repo": match p.origin { Origin::Root => "root", Origin::Platform => "platform", Origin::Locked => "locked", Origin::Repository(_) => "repo", Origin::Detached => "none", Origin::Result => "result" },
        "alias_of": p.alias_of.map(|b| arena[b].version.clone()),
        "root_alias": p.alias_of.map(|_| p.root_package_alias),
        "default_branch": p.is_default_branch,
        "stability": p.stability,
        "dist_ref": p.dist_reference(),
        "source_ref": p.source_reference(),
        "requires": links(&p.requires),
        "conflicts": links(&p.conflicts),
        "replaces": links(&p.replaces),
        "provides": links(&p.provides),
    })
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

/// Partial update case: (fixture, packages, -w/-W mode).
const PARTIAL: &[(&str, &[&str], &str)] = &[
    ("laravel", &["laravel/pint"], ""),
    ("symfony", &["doctrine/orm"], "-w"),
    ("sylius", &["symfony/*"], "-W"),
    ("sylius", &["symfony/console", "symfony/http-kernel"], "-w"),
    ("sylius", &["sylius/sylius"], "-W"),
];

fn oracle(s: &Setup, solve: bool) -> Value {
    oracle_with(s, solve, &[], "")
}

fn oracle_with(s: &Setup, solve: bool, update: &[&str], mode: &str) -> Value {
    let mut cmd = Command::new("php");
    cmd.arg(root().join("tools/oracle-pool.php")).arg(phar());
    if solve {
        cmd.arg("--solve");
    }
    if !update.is_empty() {
        cmd.arg("--update").args(update);
        if !mode.is_empty() {
            cmd.arg(mode);
        }
    }
    let out = cmd
        .current_dir(&s.project)
        .env("COMPOSER_HOME", &s.home)
        .env("COMPOSER_CACHE_DIR", s.home.join("cache"))
        .env("COMPOSER_ROOT_VERSION", &s.root_version)
        .env("COMPOSER_MEMORY_LIMIT", "-1")
        .output()
        .expect("php");
    assert!(
        out.status.success(),
        "oracle: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("oracle json")
}

/// The `composer` platform package carries the version of Composer itself:
/// the port emulates 2.10.3, the oracle is the phar under test. Under
/// another Composer (drift job), this difference is expected and is not a
/// drift of the port: it is neutralized, and reported.
fn same_modulo_composer_version(e: &Value, g: &Value) -> bool {
    if e == g {
        return true;
    }
    if e["name"] == "composer" && g["name"] == "composer" && e["repo"] == "platform" {
        let mut g2 = g.clone();
        for k in ["version", "pretty", "stability"] {
            g2[k] = e[k].clone();
        }
        if g2 == *e {
            eprintln!(
                "note: platform package composer {} (oracle) vs {} (port, the emulated Composer) — ignored",
                e["pretty"], g["pretty"]
            );
            return true;
        }
    }
    false
}

fn compare(fx: &str, expected: &Value, got: &[Value]) -> usize {
    let exp = expected["packages"].as_array().expect("packages");
    let mut divergences = 0;
    let n = exp.len().max(got.len());
    for i in 0..n {
        match (exp.get(i), got.get(i)) {
            (Some(e), Some(g)) if same_modulo_composer_version(e, g) => {}
            (e, g) => {
                divergences += 1;
                if divergences <= 5 {
                    eprintln!(
                        "{fx}: divergence at index {i}\n  composer: {}\n  vivacity:   {}",
                        e.map(|v| v.to_string())
                            .unwrap_or_else(|| "(absent)".into()),
                        g.map(|v| v.to_string())
                            .unwrap_or_else(|| "(absent)".into())
                    );
                }
            }
        }
    }
    eprintln!(
        "{fx}: composer {} packages, vivacity {} packages, {divergences} divergence(s)",
        exp.len(),
        got.len()
    );
    divergences
}

#[test]
fn pool_matches_composer_on_snapshots() {
    let only: Option<String> = std::env::var("VIVACITY_ORACLE_FIXTURE").ok();
    let mut total = 0;
    for fx in FIXTURES {
        if only.as_deref().is_some_and(|o| o != *fx) {
            continue;
        }
        let s = setup(fx);
        let expected = oracle(&s, false);
        // Same environment variable as the oracle (read by root version
        // detection); the fixtures run back to back in a single test.
        std::env::set_var("COMPOSER_ROOT_VERSION", &s.root_version);
        let mut session = UpdateSession::prepare(&s.project, Some(&s.home), true)
            .unwrap_or_else(|e| panic!("{fx}: {e}"));
        let pool = session
            .create_filtered_pool()
            .unwrap_or_else(|e| panic!("{fx}: {e}"));
        let got: Vec<Value> = pool
            .packages
            .iter()
            .map(|idx| entry(&session.arena, *idx))
            .collect();
        total += compare(fx, &expected, &got);
    }
    assert_eq!(total, 0, "pool ≠ Composer");
}

/// R2: optimized pool, rules, decisions in order, operations and lock
/// packages identical to Composer.
#[test]
fn solve_matches_composer_on_snapshots() {
    let only: Option<String> = std::env::var("VIVACITY_ORACLE_FIXTURE").ok();
    let mut total = 0;
    for fx in FIXTURES {
        if only.as_deref().is_some_and(|o| o != *fx) {
            continue;
        }
        total += solve_case(fx, &[], "");
    }
    assert_eq!(total, 0, "solve ≠ Composer");
}

/// Partial updates: PoolBuilder's `skippedLoad` / `unlockPackage`
/// machinery, exercised on the same snapshots.
#[test]
fn partial_updates_match_composer() {
    let only: Option<String> = std::env::var("VIVACITY_ORACLE_FIXTURE").ok();
    let mut total = 0;
    for (fx, update, mode) in PARTIAL {
        if only.as_deref().is_some_and(|o| o != *fx) {
            continue;
        }
        total += solve_case(fx, update, mode);
    }
    assert_eq!(total, 0, "partial update ≠ Composer");
}

fn solve_case(fx: &str, update: &[&str], mode: &str) -> usize {
    use vivacity_resolver::platform_filter::PlatformRequirementFilter;
    use vivacity_resolver::session::UpdateOptions;
    use vivacity_resolver::transaction::Operation;
    let label = if update.is_empty() {
        fx.to_owned()
    } else {
        format!("{fx} update {} {mode}", update.join(" "))
    };
    let fx = &label;
    let mut total = 0;
    {
        let s = setup(label.split(' ').next().unwrap_or(fx));
        let expected = oracle_with(&s, true, update, mode);
        std::env::set_var("COMPOSER_ROOT_VERSION", &s.root_version);
        let transitive = match mode {
            "-w" => vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDepsNoRootRequire,
            "-W" => vivacity_resolver::pool::UpdateMode::ListedWithTransitiveDeps,
            _ => vivacity_resolver::pool::UpdateMode::OnlyListed,
        };
        let options = if update.is_empty() {
            UpdateOptions::default()
        } else {
            UpdateOptions::partial(
                &update.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
                transitive,
            )
        };
        let mut session =
            UpdateSession::prepare_update(&s.project, Some(&s.home), true, None, None, &options)
                .unwrap_or_else(|e| panic!("{fx}: {e}"));
        let mut policy = session.policy();
        let pool = session
            .create_optimized_pool(&mut policy)
            .unwrap_or_else(|e| panic!("{fx}: {e}"));
        let got: Vec<Value> = pool
            .packages
            .iter()
            .map(|idx| entry(&session.arena, *idx))
            .collect();
        let pool_divergences = compare(fx, &expected, &got);
        total += pool_divergences;
        if pool_divergences > 0 {
            return total;
        }
        let solved = session.solve(
            &pool,
            &mut policy,
            &PlatformRequirementFilter::IgnoreNothing,
        );
        if let Some(problems) = expected.get("problems") {
            match solved {
                Err(vivacity_resolver::solver::SolveError::Problems(p)) => {
                    eprintln!(
                        "{fx}: unsolvable on both sides ({} problem(s) on the vivacity side)",
                        p.len()
                    );
                }
                Err(e) => {
                    eprintln!("{fx}: Composer has no solution, vivacity fails differently: {e:?}");
                    total += 1;
                }
                Ok(_) => {
                    eprintln!("{fx}: Composer has no solution, vivacity finds one:\n{problems}");
                    total += 1;
                }
            }
            return total;
        }
        let report = match solved {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{fx}: vivacity has no solution: {e:?}");
                total += 1;
                return total;
            }
        };
        let (transaction, decisions, rules, learned) = (
            report.transaction,
            report.decisions,
            report.rules,
            report.learned,
        );
        let arena = &session.arena;
        let exp_rules = expected["rules"].as_u64().unwrap_or(0) as usize;
        let exp_learned = expected["learned"].as_u64().unwrap_or(0) as usize;
        let exp_decisions: Vec<i64> = expected["decisions"]
            .as_array()
            .expect("decisions")
            .iter()
            .map(|v| v.as_i64().expect("int"))
            .collect();
        let exp_ops: Vec<Value> = expected["operations"]
            .as_array()
            .expect("operations")
            .clone();
        let exp_lock: Vec<Value> = expected["lock"].as_array().expect("lock").clone();
        let ops: Vec<Value> = transaction
            .transaction
            .operations
            .iter()
            .map(|op| match op {
                Operation::Install(p) => json!(["install", arena[*p].name, arena[*p].version]),
                Operation::Update(i, t) => json!([
                    "update",
                    arena[*i].name,
                    arena[*i].version,
                    arena[*t].version
                ]),
                Operation::Uninstall(p) => json!(["uninstall", arena[*p].name, arena[*p].version]),
                Operation::MarkAliasInstalled(p) => {
                    json!(["markAliasInstalled", arena[*p].name, arena[*p].version])
                }
                Operation::MarkAliasUninstalled(p) => {
                    json!(["markAliasUninstalled", arena[*p].name, arena[*p].version])
                }
            })
            .collect();
        let lock: Vec<Value> = transaction
            .new_lock_packages(arena, false)
            .iter()
            .map(|&p| json!([arena[p].name, arena[p].version]))
            .collect();
        let mut d = 0;
        if rules != exp_rules || learned != exp_learned {
            eprintln!("{fx}: rules composer {exp_rules} (learned {exp_learned}), vivacity {rules} (learned {learned})");
            d += 1;
        }
        if decisions != exp_decisions {
            let first = decisions
                .iter()
                .zip(&exp_decisions)
                .position(|(a, b)| a != b);
            eprintln!(
                "{fx}: decisions composer {} / vivacity {}, first divergence at {:?} (composer {:?}, vivacity {:?})",
                exp_decisions.len(),
                decisions.len(),
                first,
                first.map(|i| exp_decisions[i]),
                first.map(|i| decisions[i])
            );
            d += 1;
        }
        if ops != exp_ops {
            eprintln!("{fx}: operations composer {exp_ops:?}\n  vivacity {ops:?}");
            d += 1;
        }
        if lock != exp_lock {
            eprintln!(
                "{fx}: lock composer {} packages, vivacity {} packages",
                exp_lock.len(),
                lock.len()
            );
            d += 1;
        }
        eprintln!(
            "{fx}: optimized pool {} packages, {rules} rules ({learned} learned), {} decisions, {} operations, {} lock packages — {d} divergence(s)",
            pool.len(),
            decisions.len(),
            ops.len(),
            lock.len()
        );
        total += d;
    }
    total
}
