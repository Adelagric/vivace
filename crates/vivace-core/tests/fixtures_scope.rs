//! Le détecteur hors-scope doit déclarer les trois fixtures installables
//! nativement (c'est le contrat v1 : elles bootent sans plugins), avec les
//! plugins bénins attendus signalés — ni plus, ni moins.

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

fn analyze(name: &str) -> vivace_core::scope::ScopeReport {
    let dir = fixture(name);
    let lock = vivace_core::lock::Lock::read(&dir.join("composer.lock")).expect("lock");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("composer.json")).expect("composer.json"),
    )
    .expect("manifest");
    vivace_core::scope::analyze(&lock, &manifest, true)
}

#[test]
fn all_fixtures_are_native() {
    for name in ["laravel", "symfony", "sylius"] {
        let report = analyze(name);
        assert!(
            report.is_native_ok(),
            "fixture {name} hors scope: {:?}",
            report.issues
        );
    }
}

#[test]
fn expected_benign_plugins_are_reported() {
    assert!(analyze("laravel").skipped_plugins.is_empty());
    assert_eq!(analyze("symfony").skipped_plugins, vec!["symfony/flex"]);
    let sylius = analyze("sylius").skipped_plugins;
    assert!(sylius.contains(&"symfony/flex".to_owned()));
    assert!(sylius.contains(&"php-http/discovery".to_owned()));
    assert_eq!(sylius.len(), 6, "liste bénigne inattendue: {sylius:?}");
}
