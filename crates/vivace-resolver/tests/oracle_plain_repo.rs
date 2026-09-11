//! Oracle d'un dépôt « plein » (Satis, `packages.json` statique, sans
//! `metadata-url`) avec `mirrors` et `options` de dépôt : le lock écrit
//! par `composer update --no-install` et celui calculé par
//! `UpdateSession::update` doivent être identiques à l'octet — y compris
//! `dist.mirrors`, `source.mirrors` et `transport-options`, que
//! `ComposerRepository::createPackages` pose après le chargement.

use std::process::Command;
use vivace_resolver::platform_filter::PlatformRequirementFilter;
use vivace_resolver::session::UpdateSession;

#[test]
fn plain_repository_with_mirrors_and_options() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    let repo_url = format!("file://{}", repo.display());
    std::fs::write(
        repo.join("packages.json"),
        format!(
            r#"{{"packages": {{"acme/lib": [{{"name": "acme/lib", "version": "1.0.0", "dist": {{"type": "zip", "url": "{repo_url}/dist/acme-lib-1.0.0.zip", "reference": "abc"}}, "require": {{"acme/other": "^1"}}, "transport-options": {{"ignored": true}}, "target-dir": "../Acme/./Lib/", "type": "", "keywords": ["b", "a"], "bin": {{"x": "/bin/a"}}}}], "acme/other": [{{"name": "acme/other", "version": "1.2.0", "dist": {{"type": "zip", "url": "https://elsewhere.example.org/other.zip"}}, "source": {{"type": "git", "url": "https://github.com/acme/other.git", "reference": "def"}}, "time": "2020-11-13 09:40:50 +0100"}}]}}, "mirrors": [{{"dist-url": "https://mirror.example.org/%package%/%version%.zip", "preferred": true}}, {{"git-url": "https://gitmirror.example.org/%package%.git"}}]}}"#
        ),
    )
    .expect("packages.json");
    let manifest = format!(
        r#"{{"repositories": [{{"type": "composer", "url": "{repo_url}", "options": {{"ssl": {{"verify_peer": false}}}}}}, {{"packagist.org": false}}], "require": {{"acme/lib": "^1"}}}}"#
    );
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    for side in ["composer", "vivace"] {
        let d = tmp.path().join(side);
        std::fs::create_dir_all(&d).expect("dir");
        std::fs::write(d.join("composer.json"), &manifest).expect("manifest");
    }
    let out = Command::new("composer")
        .args([
            "update",
            "--no-install",
            "--no-scripts",
            "--no-plugins",
            "--no-interaction",
            "--no-audit",
            "--quiet",
        ])
        .current_dir(tmp.path().join("composer"))
        .env("COMPOSER_HOME", &home)
        .env("COMPOSER_CACHE_DIR", home.join("cache"))
        .output()
        .expect("composer");
    assert!(
        out.status.success(),
        "composer: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected =
        std::fs::read_to_string(tmp.path().join("composer/composer.lock")).expect("lock");

    let project = tmp.path().join("vivace");
    let mut session = UpdateSession::prepare(&project, Some(&home), true).expect("prepare");
    let (lock, _) = session
        .update(&manifest, &PlatformRequirementFilter::IgnoreNothing)
        .expect("update");
    let mut text =
        vivace_core::phpjson::php_json_encode_with(&lock, vivace_core::phpjson::FLAGS_JSONFILE)
            .expect("encode");
    text.push('\n');
    assert_eq!(text, expected);
}
