//! Oracle de bout en bout du scaffold Drupal : pour chaque cas, un mini-projet
//! (paquets synthétiques en dépôts `path`, plugin vendoré ou depuis Packagist)
//! est installé deux fois — par Composer avec le plugin actif (référence), et
//! par Composer `--no-plugins` puis le port Rust (`scaffold::plan/apply` +
//! `pre_autoload_dump`). Les arbres (hors vendor/composer) doivent être
//! identiques, ainsi que `vendor/drupal/DrupalInstalled.php` et les entrées
//! de classmap ajoutées à la racine.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use vivace_core::scaffold::{self, ScaffoldPackage};

/// Un paquet synthétique : (nom, extra, fichiers (chemin, contenu)).
type SyntheticPackage = (&'static str, Value, Vec<(&'static str, &'static str)>);

struct Case {
    name: &'static str,
    /// `extra` de la racine.
    root_extra: Value,
    /// Paquets synthétiques : (nom, extra.drupal-scaffold, fichiers (chemin, contenu)).
    packages: Vec<SyntheticPackage>,
    /// Fichiers présents dans le projet avant l'install (chemin, contenu).
    preexisting: Vec<(&'static str, &'static str)>,
    /// `git init` + `.gitignore` ignorant vendor/ ; les chemins listés sont
    /// committés avant l'install (fichiers « trackés »).
    git: Option<Vec<&'static str>>,
    /// Version du plugin : None = source vendorée (11.4.6), Some = Packagist.
    plugin_version: Option<&'static str>,
}

fn sc(mapping: Value) -> Value {
    json!({"drupal-scaffold": {"file-mapping": mapping}})
}

fn cases() -> Vec<Case> {
    let core_files = vec![
        ("assets/index.php", "<?php\n// index from core\n"),
        ("assets/robots.txt", "User-agent: *\n"),
        ("assets/htaccess", "# htaccess\n"),
        ("assets/append.txt", "# appended by core\n"),
        ("assets/empty.txt", ""),
        ("assets/default.txt", "default content\n"),
        ("assets/zero.txt", "0"),
    ];
    let core = |mapping: Value| ("drupal/core", sc(mapping), core_files.clone());
    let web = json!({"drupal-scaffold": {"locations": {"web-root": "web/"}}});
    vec![
        Case {
            name: "replace-defaults",
            root_extra: web.clone(),
            packages: vec![core(json!({
                "[web-root]/index.php": "assets/index.php",
                "[web-root]/robots.txt": "assets/robots.txt",
                "[web-root]/.htaccess": "assets/htaccess",
                "[project-root]/README.core": "assets/robots.txt",
                "[web-root]/skipped.txt": false
            }))],
            preexisting: vec![],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "overwrite-false-existing-and-absent",
            root_extra: web.clone(),
            packages: vec![core(json!({
                "[web-root]/robots.txt": {"path": "assets/robots.txt", "overwrite": false},
                "[web-root]/index.php": {"path": "assets/index.php", "overwrite": "false"},
                "[web-root]/new.txt": {"path": "assets/robots.txt", "overwrite": false}
            }))],
            preexisting: vec![
                ("web/robots.txt", "custom robots\n"),
                ("web/index.php", "custom index\n"),
            ],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "override-between-packages-and-append",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "allowed-packages": ["acme/site"]}}),
            packages: vec![
                core(json!({
                    "[web-root]/index.php": "assets/index.php",
                    "[web-root]/robots.txt": "assets/robots.txt"
                })),
                (
                    "acme/site",
                    sc(json!({
                        "[web-root]/index.php": "files/index.php",
                        "[web-root]/robots.txt": {"append": "files/extra.txt", "prepend": "files/head.txt"},
                        "[web-root]/.htaccess": {"append": "files/extra.txt"},
                        "[web-root]/forced.txt": {"append": "files/extra.txt", "force-append": true},
                        "[web-root]/defaulted.txt": {"append": "files/extra.txt", "default": "files/default.txt"},
                        "[web-root]/path-and-append.txt": {"path": "files/index.php", "append": "files/extra.txt", "force-append": true, "default": "files/default.txt"}
                    })),
                    vec![
                        ("files/index.php", "<?php\n// index from site\n"),
                        ("files/extra.txt", "Disallow: /private\n"),
                        ("files/head.txt", "# head\n"),
                        ("files/default.txt", "DEFAULT\n"),
                    ],
                ),
            ],
            preexisting: vec![("web/forced.txt", "existing forced\n")],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "append-skips-when-data-present-or-empty-source",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "allowed-packages": ["acme/site"]}}),
            packages: vec![
                core(json!({"[web-root]/robots.txt": "assets/robots.txt"})),
                (
                    "acme/site",
                    sc(json!({
                        "[web-root]/has-data.txt": {"append": "files/extra.txt", "force-append": true},
                        "[web-root]/empty-append.txt": {"append": "files/empty.txt", "force-append": true},
                        "[web-root]/robots.txt": {"append": "files/empty.txt"},
                        "[web-root]/zero.txt": {"append": "files/extra.txt", "default": "files/default.txt"}
                    })),
                    vec![
                        ("files/extra.txt", "Disallow: /private\n"),
                        ("files/empty.txt", ""),
                        ("files/default.txt", "DEFAULT\n"),
                    ],
                ),
            ],
            preexisting: vec![
                ("web/has-data.txt", "already Disallow: /private\n here\n"),
                ("web/zero.txt", "0"),
            ],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "recursive-allowed-and-root-mapping-last",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "allowed-packages": ["acme/profile"], "file-mapping": {"[web-root]/robots.txt": "root-assets/robots.txt", "[web-root]/index.php": false}}}),
            packages: vec![
                core(
                    json!({"[web-root]/index.php": "assets/index.php", "[web-root]/robots.txt": "assets/robots.txt"}),
                ),
                (
                    "acme/profile",
                    json!({"drupal-scaffold": {"allowed-packages": ["acme/theme"], "file-mapping": {"[web-root]/profile.txt": "p.txt"}}}),
                    vec![("p.txt", "profile\n")],
                ),
                (
                    "acme/theme",
                    sc(json!({"[web-root]/theme.txt": "t.txt", "[web-root]/robots.txt": "t.txt"})),
                    vec![("t.txt", "theme\n")],
                ),
                (
                    "acme/notallowed",
                    sc(json!({"[web-root]/never.txt": "n.txt"})),
                    vec![("n.txt", "never\n")],
                ),
            ],
            preexisting: vec![("root-assets/robots.txt", "root robots\n")],
            git: None,
            plugin_version: None,
        },
        Case {
            // Un metapackage n'a pas de chemin d'installation mais reste
            // trouvé par findPackage : ses allowed-packages comptent.
            name: "allowed-through-metapackage",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "allowed-packages": ["acme/metapackage"]}}),
            packages: vec![
                core(json!({"[web-root]/index.php": "assets/index.php"})),
                (
                    "acme/metapackage",
                    json!({"drupal-scaffold": {"allowed-packages": ["acme/theme"]}}),
                    vec![],
                ),
                (
                    "acme/theme",
                    sc(json!({"[web-root]/theme.txt": "t.txt"})),
                    vec![("t.txt", "theme\n")],
                ),
            ],
            preexisting: vec![],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "custom-locations-and-tokens",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "docroot/", "extra": "shared"}}}),
            packages: vec![core(json!({
                "[web-root]/index.php": "assets/index.php",
                "[extra]/robots.txt": "assets/robots.txt",
                "[web_root]/legacy.txt": "assets/robots.txt"
            }))],
            preexisting: vec![],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "no-allowed-packages",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}}}),
            packages: vec![(
                "acme/lib",
                sc(json!({"[web-root]/x.txt": "x.txt"})),
                vec![("x.txt", "x\n")],
            )],
            preexisting: vec![],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "gitignore-managed",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}}}),
            packages: vec![core(json!({
                "[web-root]/index.php": "assets/index.php",
                "[web-root]/sites/README.txt": "assets/robots.txt",
                "[web-root]/.gitignore": "assets/htaccess",
                "[project-root]/.editorconfig": "assets/robots.txt"
            }))],
            preexisting: vec![("web/existing.txt", "keep\n")],
            git: Some(vec!["web/existing.txt"]),
            plugin_version: None,
        },
        Case {
            name: "gitignore-tracked-autoload-kept",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}}}),
            packages: vec![core(json!({"[web-root]/index.php": "assets/index.php"}))],
            preexisting: vec![
                ("web/autoload.php", "<?php // committed autoload\n"),
                ("web/autoload_runtime.php", "<?php // committed runtime\n"),
            ],
            git: Some(vec!["web/autoload.php", "web/autoload_runtime.php"]),
            plugin_version: None,
        },
        Case {
            name: "gitignore-forced-true-without-repo",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "gitignore": true}}),
            packages: vec![core(json!({"[web-root]/index.php": "assets/index.php"}))],
            preexisting: vec![],
            git: None,
            plugin_version: None,
        },
        Case {
            name: "gitignore-forced-false-in-repo",
            root_extra: json!({"drupal-scaffold": {"locations": {"web-root": "web/"}, "gitignore": false}}),
            packages: vec![core(json!({"[web-root]/index.php": "assets/index.php"}))],
            preexisting: vec![],
            git: Some(vec![]),
            plugin_version: None,
        },
        Case {
            name: "rerun-unchanged",
            root_extra: web.clone(),
            packages: vec![core(
                json!({"[web-root]/index.php": "assets/index.php", "[web-root]/robots.txt": "assets/robots.txt"}),
            )],
            preexisting: vec![
                ("web/index.php", "<?php\n// index from core\n"),
                ("web/robots.txt", "stale\n"),
            ],
            git: Some(vec![]),
            plugin_version: None,
        },
        Case {
            name: "plugin-11.3.16-profile-sorted-no-runtime",
            root_extra: web.clone(),
            packages: vec![core(json!({"[web-root]/index.php": "assets/index.php"}))],
            preexisting: vec![],
            git: None,
            plugin_version: Some("11.3.16"),
        },
        Case {
            name: "plugin-11.2.14-profile-no-predump",
            root_extra: web.clone(),
            packages: vec![core(json!({"[web-root]/index.php": "assets/index.php"}))],
            preexisting: vec![],
            git: None,
            plugin_version: Some("11.2.14"),
        },
    ]
}

fn write(path: &Path, content: &str) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).expect("mkdir");
    }
    std::fs::write(path, content).expect("write");
}

fn run(dir: &Path, cmd: &str, args: &[&str]) -> std::process::Output {
    Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .env("LANGUAGE", "C")
        .env("LC_ALL", "C")
        .env("COMPOSER_NO_INTERACTION", "1")
        .output()
        .unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// Écrit le projet d'un cas dans `dir` (avant tout install).
fn build_project(case: &Case, dir: &Path, plugin_src: &Path) {
    let mut repos = vec![];
    let mut require = serde_json::Map::new();
    match case.plugin_version {
        None => {
            // Plugin vendoré, exposé en dépôt path avec une version explicite.
            let plugin = dir.join("plugin");
            copy_dir(plugin_src, &plugin);
            let mut cj: Value = serde_json::from_str(
                &std::fs::read_to_string(plugin.join("composer.json")).expect("plugin json"),
            )
            .expect("json");
            cj["version"] = json!("11.4.6");
            write(
                &plugin.join("composer.json"),
                &serde_json::to_string_pretty(&cj).expect("json"),
            );
            repos.push(json!({"type": "path", "url": "plugin", "options": {"symlink": false}}));
            require.insert(scaffold::PLUGIN.into(), json!("11.4.6"));
        }
        Some(v) => {
            require.insert(scaffold::PLUGIN.into(), json!(v));
        }
    }
    for (name, extra, files) in &case.packages {
        let pdir = dir.join("pkgs").join(name.replace('/', "__"));
        for (f, c) in files {
            write(&pdir.join(f), c);
        }
        let ty = if name.ends_with("/metapackage") {
            "metapackage"
        } else {
            "library"
        };
        write(
            &pdir.join("composer.json"),
            &serde_json::to_string_pretty(
                &json!({"name": name, "version": "1.0.0", "type": ty, "extra": extra}),
            )
            .expect("json"),
        );
        repos.push(json!({"type": "path", "url": format!("pkgs/{}", name.replace('/', "__")), "options": {"symlink": false}}));
        require.insert((*name).into(), json!("1.0.0"));
    }
    if case.plugin_version.is_none() {
        repos.push(json!({"packagist.org": false}));
    }
    let root = json!({
        "name": "oracle/root",
        "type": "project",
        "repositories": repos,
        "require": require,
        "config": {"allow-plugins": {scaffold::PLUGIN: true}},
        "extra": case.root_extra,
    });
    write(
        &dir.join("composer.json"),
        &serde_json::to_string_pretty(&root).expect("json"),
    );
    for (f, c) in &case.preexisting {
        write(&dir.join(f), c);
    }
    if let Some(tracked) = &case.git {
        assert!(run(dir, "git", &["init", "-q"]).status.success());
        write(&dir.join(".gitignore"), "/vendor/\n");
        let mut args = vec!["add", ".gitignore"];
        args.extend(tracked.iter().copied());
        assert!(run(dir, "git", &args).status.success());
        if !tracked.is_empty() {
            let out = Command::new("git")
                .args([
                    "-c",
                    "user.name=o",
                    "-c",
                    "user.email=o@o",
                    "commit",
                    "-q",
                    "-m",
                    "tracked",
                ])
                .current_dir(dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read_dir") {
        let e = e.expect("entry");
        let dest = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), &dest).expect("copy");
        }
    }
}

/// Arbre (chemin relatif → contenu) hors vendor/composer et .git.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).expect("read_dir") {
            let e = e.expect("entry");
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .expect("rel")
                .to_string_lossy()
                .into_owned();
            if rel == ".git" || rel == "vendor/composer" || rel.starts_with("vendor/composer/") {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(rel, std::fs::read(&p).expect("read"));
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

#[test]
fn scaffold_matches_the_real_plugin() {
    let plugin_src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/reference/drupal-scaffold")
        .canonicalize()
        .expect("reference");
    let base = tempfile::tempdir().expect("tmp");
    let mut compared = 0;
    for case in cases() {
        let dir = base.path().join(case.name);
        let reference = dir.join("ref");
        let ours = dir.join("viv");
        build_project(&case, &reference, &plugin_src);
        copy_dir(&reference, &ours);

        let out = run(
            &reference,
            "composer",
            &["update", "--no-scripts", "--quiet"],
        );
        assert!(
            out.status.success(),
            "[{}] composer (ref): {}",
            case.name,
            String::from_utf8_lossy(&out.stderr)
        );
        let out = run(
            &ours,
            "composer",
            &["update", "--no-scripts", "--no-plugins", "--quiet"],
        );
        assert!(
            out.status.success(),
            "[{}] composer (viv): {}",
            case.name,
            String::from_utf8_lossy(&out.stderr)
        );

        // Côté vivace : profil depuis la copie installée, plan + apply.
        let ours_real = ours.canonicalize().expect("real");
        let plugin_dir = ours_real.join("vendor").join(scaffold::PLUGIN);
        let fp = scaffold::fingerprint(&plugin_dir).expect("fingerprint");
        let profile = scaffold::profile_for(&fp)
            .unwrap_or_else(|| panic!("[{}] unported fingerprint {fp}", case.name));
        let lock = vivace_core::lock::Lock::read(&ours.join("composer.lock")).expect("lock");
        let root_manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(ours.join("composer.json")).expect("json"),
        )
        .expect("json");
        let packages: Vec<ScaffoldPackage> = lock
            .wanted_packages(true)
            .map(|p| ScaffoldPackage {
                name: p.name().to_owned(),
                dir: if p.is_metapackage() {
                    std::path::PathBuf::from("/")
                } else {
                    ours_real.join("vendor").join(p.name())
                },
                extra: p.raw.get("extra").cloned(),
            })
            .collect();
        let plan = scaffold::plan(
            profile,
            &ours_real,
            "oracle/root",
            root_manifest.get("extra"),
            &packages,
        )
        .unwrap_or_else(|e| panic!("[{}] plan: {e}", case.name));
        // preAutoloadDump (avant le scaffold chez Composer, l'ordre est sans effet ici).
        let root = vivace_core::state::RootPackage::detect(&root_manifest, &ours_real, true);
        if let Some(pre) = scaffold::pre_autoload_dump(
            profile,
            "vendor",
            &scaffold::hash_packages(&lock, true),
            &scaffold::root_hash_package(&root),
        ) {
            std::fs::create_dir_all(ours_real.join("vendor/drupal")).expect("mkdir");
            std::fs::write(
                ours_real.join("vendor/drupal/DrupalInstalled.php"),
                &pre.drupal_installed,
            )
            .expect("write");
            // Les entrées ajoutées à la classmap doivent être dans celle de la référence.
            let classmap =
                std::fs::read_to_string(reference.join("vendor/composer/autoload_classmap.php"))
                    .expect("classmap");
            for entry in &pre.root_classmap {
                let needle = entry.trim_start_matches("vendor");
                assert!(
                    classmap.contains(&format!("$vendorDir . '{needle}'")),
                    "[{}] classmap entry {entry} missing in reference",
                    case.name
                );
            }
            assert!(
                classmap.contains("DrupalInstalled"),
                "[{}] reference has no DrupalInstalled",
                case.name
            );
        } else {
            assert!(
                !reference.join("vendor/drupal/DrupalInstalled.php").exists(),
                "[{}] reference wrote DrupalInstalled.php but profile says no",
                case.name
            );
        }
        plan.apply().expect("apply");

        let a = snapshot(&reference);
        let b = snapshot(&ours);
        let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        for k in keys {
            match (a.get(k), b.get(k)) {
                (Some(x), Some(y)) => assert!(
                    x == y,
                    "[{}] {k} differs:\n--- composer\n{}\n--- vivace\n{}",
                    case.name,
                    String::from_utf8_lossy(x),
                    String::from_utf8_lossy(y)
                ),
                (Some(_), None) => panic!("[{}] {k} written by Composer only", case.name),
                (None, Some(_)) => panic!("[{}] {k} written by vivace only", case.name),
                (None, None) => unreachable!(),
            }
            compared += 1;
        }
        // Second passage sur la référence : idempotence de Composer = la nôtre
        // (fichiers inchangés non réécrits, autoload*.php réécrits).
        if case.name == "rerun-unchanged" {
            let out = run(
                &reference,
                "composer",
                &["install", "--no-scripts", "--quiet"],
            );
            assert!(out.status.success());
            let plan2 = scaffold::plan(
                profile,
                &ours_real,
                "oracle/root",
                root_manifest.get("extra"),
                &packages,
            )
            .expect("plan2");
            plan2.apply().expect("apply2");
            assert_eq!(
                snapshot(&reference),
                snapshot(&ours),
                "[rerun] second pass differs"
            );
        }
    }
    eprintln!(
        "oracle scaffold: {compared} fichiers comparés sur {} cas",
        cases().len()
    );
    assert!(compared > 80);
}
