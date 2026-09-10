//! Oracle de bout en bout de la disposition composer/installers : Composer
//! (phar) + le vrai plugin (docs/reference/installers, v2.3.0) activé sur un
//! Composer construit avec l'extra racine du cas ; on interroge
//! `InstallationManager::getInstallPath` (dispatch réel : `supports` faux →
//! LibraryInstaller) puis `Filesystem::findShortestPath(vendor/composer, …)`.
//! Le port (`layout::resolve`) doit rendre le même chemin relatif au projet
//! et le même `install-path`.
//!
//! Quand vivace refuse un cas (framework custom, cible refusée…), le test
//! vérifie seulement que le refus est de la catégorie attendue ; quand il
//! l'accepte, l'égalité est obligatoire.

use serde_json::{json, Value};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use vivace_core::layout::Layout;
use vivace_core::lock::Lock;

/// Un cas : extra racine + paquets (nom, type, extra, target-dir).
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

    // Tous les frameworks × tous leurs emplacements × plusieurs noms, sans
    // installer-paths : le défaut de la table (frameworks custom compris —
    // vivace doit les refuser).
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

    // Types non pris par le plugin ou sans emplacement, target-dir legacy.
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

    // installer-paths : par type, par nom, par vendor, chevauchements,
    // templates avec toutes les variables, listes et scalaires.
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

    // installer-disable sous toutes ses formes.
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

    // Formes malformées ou hostiles d'installer-paths : vivace refuse, le
    // test vérifie que la catégorie de refus est la bonne.
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

/// Lock minimal : composer/installers 2.3.0 + le paquet du cas.
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
    // PHP réalise le cwd (macOS : /private/var…) ; on aligne notre racine.
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
                    // Un refus est soit ce que Composer refuse aussi, soit
                    // un refus délibéré (logique non portée, cible dangereuse).
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
