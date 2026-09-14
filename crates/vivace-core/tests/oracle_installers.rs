//! End-to-end oracle of the composer/installers layout: Composer (phar) +
//! the real plugin (docs/reference/installers, v2.3.0) activated on a
//! Composer built with the case's root extra; we query
//! `InstallationManager::getInstallPath` (real dispatch: `supports` false ->
//! LibraryInstaller) then `Filesystem::findShortestPath(vendor/composer, ...)`.
//! The port (`layout::resolve`) must return the same project-relative path
//! and the same `install-path`.
//!
//! When vivace refuses a case (custom framework, refused target...), the test
//! only checks that the refusal is of the expected category; when it accepts
//! it, equality is mandatory.

use serde_json::{json, Value};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use vivace_core::layout::Layout;
use vivace_core::lock::Lock;

/// A case: root extra + packages (name, type, extra, target-dir).
struct Case {
    root_extra: Value,
    packages: Vec<Value>,
}

fn pkg(name: &str, ty: &str) -> Value {
    json!({"name": name, "type": ty})
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let table = vivace_core::installers::table_for("v2.3.0").expect("table");

    // All frameworks x all their locations x several names, without
    // installer-paths: the table default (custom frameworks included; vivace
    // must refuse them).
    let mut defaults = Vec::new();
    for fw in table.frameworks() {
        for loc in fw.locations.keys() {
            let ty = format!("{}-{loc}", fw.key);
            defaults.push(pkg(&format!("acme/{}-{loc}", fw.key), &ty));
            defaults.push(pkg(&format!("Acme/Camel{}", fw.key), &ty));
            defaults.push(pkg(&format!("noslash-{}-{loc}", fw.key), &ty));
        }
    }
    out.push(Case {
        root_extra: json!({}),
        packages: defaults,
    });

    // Types not taken by the plugin or without a location, legacy target-dir.
    out.push(Case {
        root_extra: json!({}),
        packages: vec![
            pkg("acme/lib", "library"),
            pkg("acme/core", "wordpress-core"),
            pkg("acme/x", "wordpressx-plugin"),
            pkg("acme/extra", "wordpress-plugin-extra"),
            pkg("acme/plugin", "composer-plugin"),
            pkg("acme/meta", "metapackage"),
            pkg("acme/bare", "wordpress"),
            pkg("acme/dash", "wordpress-"),
            json!({"name": "acme/legacy", "type": "library", "target-dir": "Acme/Legacy"}),
            json!({"name": "acme/named", "type": "drupal-module", "extra": {"installer-name": "renamed"}}),
            json!({"name": "acme/named-empty", "type": "drupal-module", "extra": {"installer-name": ""}}),
            json!({"name": "acme/named-zero", "type": "drupal-module", "extra": {"installer-name": "0"}}),
            json!({"name": "acme/named-slash", "type": "drupal-module", "extra": {"installer-name": "a/b"}}),
        ],
    });

    // installer-paths: by type, by name, by vendor, overlaps, templates with
    // all the variables, lists and scalars.
    out.push(Case {
        root_extra: json!({"installer-paths": {
            "web/app/plugins/dolly/": ["wpackagist-plugin/hello-dolly"],
            "web/app/mu-plugins/{$name}/": ["type:wordpress-muplugin"],
            "web/app/plugins/{$name}/": ["type:wordpress-plugin"],
            "web/app/themes/{$vendor}-{$name}-{$type}/": ["type:wordpress-theme"],
            "modules/contrib/{$name}": ["type:drupal-module", "type:drupal-theme"],
            "by-vendor/{$name}": ["vendor:special"],
            "scalar/{$name}": "type:laravel-library"
        }}),
        packages: vec![
            pkg("wpackagist-plugin/hello-dolly", "wordpress-plugin"),
            pkg("wpackagist-plugin/akismet", "wordpress-plugin"),
            pkg("acme/mu", "wordpress-muplugin"),
            pkg("wpackagist-theme/twentytwentyfour", "wordpress-theme"),
            pkg("acme/mod", "drupal-module"),
            pkg("acme/theme", "drupal-theme"),
            pkg("acme/core", "drupal-core"),
            pkg("special/anything", "drupal-library"),
            pkg("special/lib", "library"),
            pkg("acme/lar", "laravel-library"),
            pkg("acme/dropin", "wordpress-dropin"),
        ],
    });

    // installer-disable in all its forms.
    for disable in [
        json!(true),
        json!("all"),
        json!("*"),
        json!(["drupal"]),
        json!(["wordpress", "drupal"]),
        json!(false),
        json!("drupal"),
        json!(["nope"]),
    ] {
        out.push(Case {
            root_extra: json!({"installer-disable": disable}),
            packages: vec![
                pkg("acme/mod", "drupal-module"),
                pkg("acme/wp", "wordpress-plugin"),
                pkg("acme/lar", "laravel-library"),
            ],
        });
    }

    // Malformed or hostile forms of installer-paths: vivace refuses, the test
    // checks that the refusal category is the right one.
    for paths in [
        json!({"x/{$name}": true}),
        json!({"x/{$Nope}": ["acme/mod"]}),
        json!(["acme/mod"]),
        json!("type:drupal-module"),
        json!({"": ["acme/mod"]}),
        json!({"/abs/{$name}": ["acme/mod"]}),
        json!({"{$name}/../../out": ["acme/mod"]}),
        json!({"vendor/{$vendor}/{$name}": ["acme/mod"]}),
    ] {
        out.push(Case {
            root_extra: json!({"installer-paths": paths}),
            packages: vec![pkg("acme/mod", "drupal-module")],
        });
    }
    out
}

fn php_oracle(cases: &[Case], cwd: &Path) -> Vec<Vec<Value>> {
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
    let installers_src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/reference/installers/src/Composer/Installers")
        .canonicalize()
        .expect("docs/reference/installers");
    let script = format!(
        r#"require "phar://{phar}/vendor/autoload.php";
           $ref = {src};
           spl_autoload_register(function (string $class) use ($ref): void {{
               if (str_starts_with($class, 'Composer\\Installers\\')) {{
                   $f = $ref . '/' . substr($class, strlen('Composer\\Installers\\')) . '.php';
                   if (is_file($f)) {{ require_once $f; }}
               }}
           }});
           $io = new \Composer\IO\NullIO();
           $fs = new \Composer\Util\Filesystem();
           $cwd = getcwd();
           $out = [];
           foreach (json_decode(stream_get_contents(STDIN), true) as $case) {{
               $composer = \Composer\Factory::create($io, ['name' => 'oracle/root', 'extra' => $case['root_extra'], 'config' => ['vendor-dir' => 'vendor']], true, true);
               $plugin = new \Composer\Installers\Plugin();
               $plugin->activate($composer, $io);
               $im = $composer->getInstallationManager();
               $row = [];
               foreach ($case['packages'] as $p) {{
                   $pkg = new \Composer\Package\Package($p['name'], '1.0.0.0', '1.0.0');
                   $pkg->setType($p['type']);
                   if (isset($p['extra'])) {{ $pkg->setExtra($p['extra']); }}
                   if (isset($p['target-dir'])) {{ $pkg->setTargetDir($p['target-dir']); }}
                   try {{
                       $path = $im->getInstallPath($pkg);
                       if ($path === null || $path === '') {{ $row[] = ['path' => null, 'install_path' => null]; continue; }}
                       $abs = $fs->normalizePath($fs->isAbsolutePath($path) ? $path : $cwd . '/' . $path);
                       $rel = str_starts_with($abs, $cwd . '/') ? substr($abs, strlen($cwd) + 1) : $abs;
                       $row[] = ['path' => $rel, 'install_path' => $fs->findShortestPath($cwd . '/vendor/composer', $abs, true)];
                   }} catch (\Throwable $e) {{
                       $row[] = ['error' => get_class($e) . ': ' . $e->getMessage()];
                   }}
               }}
               $out[] = $row;
           }}
           echo json_encode($out);"#,
        phar = phar.display(),
        src = vivace_core::pathutil::php_str(&installers_src.to_string_lossy()),
    );
    let input: Vec<Value> = cases
        .iter()
        .map(|c| json!({"root_extra": c.root_extra, "packages": c.packages}))
        .collect();
    let mut child = Command::new("php")
        .args([
            "-d",
            "display_errors=stderr",
            "-d",
            "error_reporting=E_ALL & ~E_WARNING & ~E_DEPRECATED",
            "-r",
            &script,
        ])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("php requis");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(serde_json::to_string(&input).expect("json").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("php");
    assert!(
        out.status.success(),
        "php: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("json: {e}\n{}", String::from_utf8_lossy(&out.stdout)))
}

/// Minimal lock: composer/installers 2.3.0 + the case's package.
fn lock_for(p: &Value) -> Lock {
    let mut entry = p.clone();
    entry["version"] = json!("1.0.0");
    entry["dist"] = json!({"type": "zip", "url": "https://x/y.zip", "reference": "r"});
    Lock::parse(
        &json!({
            "packages": [
                {"name": "composer/installers", "version": "v2.3.0", "type": "composer-plugin",
                 "dist": {"type": "zip", "url": "https://x/i.zip", "reference": "r"}},
                entry
            ],
            "packages-dev": [],
            "plugin-api-version": "2.6.0"
        })
        .to_string(),
    )
    .expect("lock")
}

#[test]
fn layout_matches_composer_with_the_real_plugin() {
    let cwd = tempfile::tempdir().expect("tmp");
    // PHP resolves the cwd with realpath (macOS: /private/var...); align our root.
    let root = cwd.path().canonicalize().expect("canonicalize");
    let cases = cases();
    let oracle = php_oracle(&cases, &root);
    assert_eq!(oracle.len(), cases.len());

    let (mut compared, mut refused) = (0usize, 0usize);
    for (case, rows) in cases.iter().zip(oracle) {
        assert_eq!(rows.len(), case.packages.len());
        let manifest = json!({"config": {"allow-plugins": true}, "extra": case.root_extra});
        for (p, expected) in case.packages.iter().zip(rows) {
            let name = p["name"].as_str().expect("name");
            let lock = lock_for(p);
            match Layout::resolve(&root, &lock, &manifest, true, true) {
                Ok(layout) => {
                    compared += 1;
                    let ours_path = layout.rel(name).map(str::to_owned);
                    let ours_ip = layout.install_path(name);
                    assert!(
                        expected.get("error").is_none(),
                        "{name} ({}) extra={}: vivace accepte, Composer échoue: {}",
                        p["type"],
                        case.root_extra,
                        expected["error"]
                    );
                    assert_eq!(
                        ours_path.as_deref(),
                        expected["path"].as_str(),
                        "chemin de {name} ({}) extra={}",
                        p["type"],
                        case.root_extra
                    );
                    assert_eq!(
                        ours_ip.as_deref(),
                        expected["install_path"].as_str(),
                        "install-path de {name} ({}) extra={}",
                        p["type"],
                        case.root_extra
                    );
                }
                Err(issues) => {
                    refused += 1;
                    let msg = issues.join(" | ");
                    // A refusal is either what Composer refuses too, or a
                    // deliberate refusal (logic not ported, dangerous target).
                    let deliberate = msg.contains("custom path logic")
                        || msg.contains("malformed")
                        || msg.contains("unknown variable")
                        || msg.contains("project root")
                        || msg.contains("absolute path")
                        || msg.contains("outside the project")
                        || msg.contains("inside vendor/");
                    if !deliberate {
                        assert!(
                            expected.get("error").is_some(),
                            "{name} ({}) extra={}: vivace refuse ({msg}) mais Composer réussit: {expected}",
                            p["type"], case.root_extra
                        );
                    }
                }
            }
        }
    }
    eprintln!("oracle installers: {compared} chemins comparés, {refused} refus");
    assert!(compared > 300, "matrice trop petite: {compared}");
}
