//! Platform-check de bout en bout sur les fixtures : Composer a installé ces
//! locks sur cette machine (M0), donc notre check doit les déclarer
//! installables ici aussi — divergence = bug de notre côté, par construction.

use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/work")
        .join(name);
    assert!(
        dir.is_dir(),
        "fixture {name} absente — lancer fixtures/make.sh"
    );
    dir
}

#[test]
fn detection_is_cached_and_sane() {
    let p = vivace_core::platform::Platform::detect()
        .expect("détection")
        .expect("php présent sur la machine de dev");
    assert!(p.php_version.starts_with(|c: char| c.is_ascii_digit()));
    assert!(p.is_64bit);
    assert!(
        p.extensions.contains_key("json"),
        "ext json toujours présente"
    );
    // Deuxième appel : servi par le cache (même résultat).
    let p2 = vivace_core::platform::Platform::detect()
        .expect("détection")
        .expect("php");
    assert_eq!(p.php_version, p2.php_version);
}

#[test]
fn fixture_locks_are_installable_here() {
    let mut platform = vivace_core::platform::Platform::detect()
        .expect("détection")
        .expect("php présent");
    for name in ["laravel", "symfony", "sylius"] {
        let dir = fixture(name);
        let lock = vivace_core::lock::Lock::read(&dir.join("composer.lock")).expect("lock");
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("composer.json")).expect("composer.json"),
        )
        .expect("manifest");
        platform.apply_overrides(&manifest);
        let failures = vivace_core::platform::check(&lock, &platform, true, &[]);
        assert!(
            failures.is_empty(),
            "fixture {name}: échecs plateforme inattendus: {failures:?}"
        );
    }
}
