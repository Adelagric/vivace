//! Oracle du pool : pour chaque instantané (fixtures/registry/*.tar.gz), le
//! pool construit par Composer (tools/oracle-pool.php, sans optimiseur ni
//! filtres) et celui de vivace-resolver doivent être identiques paquet par
//! paquet, dans l'ordre — nom, version, dépôt d'origine, alias, références,
//! liens. Le dépôt local est injecté par COMPOSER_HOME/config.json comme
//! dans harness/update.sh.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use vivace_resolver::package::{Links, Origin, Package};
use vivace_resolver::session::UpdateSession;

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
];

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
        assert!(!src.trim().is_empty(), "composer requis");
        std::fs::copy(src.trim(), &phar).expect("copie");
    }
    phar
}

fn links(l: &Links) -> Value {
    // Tableau PHP : clé du lien, ou numéros 0.. pour les entrées sans clé
    // (array_merge). Vide → `[]` chez json_encode.
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
    std::fs::write(
        reg.join("packages.json"),
        format!(
            "{{\"packages\": [], \"notify-batch\": \"https://packagist.org/downloads/\", \"metadata-url\": \"file://{}/p2/%package%.json\"}}\n",
            reg.display()
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
    // Les dépôts distants du manifeste (packages.drupal.org) ne sont pas
    // joignables hors ligne : l'oracle et le port lisent le seul instantané.
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

fn oracle(s: &Setup, solve: bool) -> Value {
    let mut cmd = Command::new("php");
    cmd.arg(root().join("tools/oracle-pool.php")).arg(phar());
    if solve {
        cmd.arg("--solve");
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

fn compare(fx: &str, expected: &Value, got: &[Value]) -> usize {
    let exp = expected["packages"].as_array().expect("packages");
    let mut divergences = 0;
    let n = exp.len().max(got.len());
    for i in 0..n {
        match (exp.get(i), got.get(i)) {
            (Some(e), Some(g)) if e == g => {}
            (e, g) => {
                divergences += 1;
                if divergences <= 5 {
                    eprintln!(
                        "{fx}: divergence à l'index {i}\n  composer: {}\n  vivace:   {}",
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
        "{fx}: composer {} paquets, vivace {} paquets, {divergences} divergence(s)",
        exp.len(),
        got.len()
    );
    divergences
}

#[test]
fn pool_matches_composer_on_snapshots() {
    let only: Option<String> = std::env::var("VIVACE_ORACLE_FIXTURE").ok();
    let mut total = 0;
    for fx in FIXTURES {
        if only.as_deref().is_some_and(|o| o != *fx) {
            continue;
        }
        let s = setup(fx);
        let expected = oracle(&s, false);
        // Même variable d'environnement que l'oracle (lue par la détection de
        // version racine) ; les fixtures s'enchaînent dans un seul test.
        std::env::set_var("COMPOSER_ROOT_VERSION", &s.root_version);
        let mut session = UpdateSession::prepare(&s.project, Some(&s.home), true)
            .unwrap_or_else(|e| panic!("{fx}: {e}"));
        let pool = session
            .create_pool()
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

/// R2 : pool optimisé, règles, décisions dans l'ordre, opérations et paquets
/// du lock identiques à Composer.
#[test]
fn solve_matches_composer_on_snapshots() {
    use vivace_resolver::platform_filter::PlatformRequirementFilter;
    use vivace_resolver::transaction::Operation;
    let only: Option<String> = std::env::var("VIVACE_ORACLE_FIXTURE").ok();
    let mut total = 0;
    for fx in FIXTURES {
        if only.as_deref().is_some_and(|o| o != *fx) {
            continue;
        }
        let s = setup(fx);
        let expected = oracle(&s, true);
        std::env::set_var("COMPOSER_ROOT_VERSION", &s.root_version);
        let mut session = UpdateSession::prepare(&s.project, Some(&s.home), true)
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
            continue;
        }
        let solved = session.solve(
            &pool,
            &mut policy,
            &PlatformRequirementFilter::IgnoreNothing,
        );
        if let Some(problems) = expected.get("problems") {
            match solved {
                Err(vivace_resolver::solver::SolveError::Problems(p)) => {
                    eprintln!(
                        "{fx}: insoluble des deux côtés ({} problème(s) chez vivace)",
                        p.len()
                    );
                }
                Err(e) => {
                    eprintln!(
                        "{fx}: Composer n'a pas de solution, vivace échoue autrement : {e:?}"
                    );
                    total += 1;
                }
                Ok(_) => {
                    eprintln!(
                        "{fx}: Composer n'a pas de solution, vivace en trouve une :\n{problems}"
                    );
                    total += 1;
                }
            }
            continue;
        }
        let report = match solved {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{fx}: vivace n'a pas de solution : {e:?}");
                total += 1;
                continue;
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
            eprintln!("{fx}: règles composer {exp_rules} (apprises {exp_learned}), vivace {rules} (apprises {learned})");
            d += 1;
        }
        if decisions != exp_decisions {
            let first = decisions
                .iter()
                .zip(&exp_decisions)
                .position(|(a, b)| a != b);
            eprintln!(
                "{fx}: décisions composer {} / vivace {}, première divergence à {:?} (composer {:?}, vivace {:?})",
                exp_decisions.len(),
                decisions.len(),
                first,
                first.map(|i| exp_decisions[i]),
                first.map(|i| decisions[i])
            );
            d += 1;
        }
        if ops != exp_ops {
            eprintln!("{fx}: opérations composer {exp_ops:?}\n  vivace {ops:?}");
            d += 1;
        }
        if lock != exp_lock {
            eprintln!(
                "{fx}: lock composer {} paquets, vivace {} paquets",
                exp_lock.len(),
                lock.len()
            );
            d += 1;
        }
        eprintln!(
            "{fx}: pool optimisé {} paquets, {rules} règles ({learned} apprises), {} décisions, {} opérations, {} paquets de lock — {d} divergence(s)",
            pool.len(),
            decisions.len(),
            ops.len(),
            lock.len()
        );
        total += d;
    }
    assert_eq!(total, 0, "solve ≠ Composer");
}
