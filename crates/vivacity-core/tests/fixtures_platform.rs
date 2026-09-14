//! End-to-end platform check on the fixtures: Composer installed these locks
//! on this machine (M0), so our check must declare them installable here
//! too; a divergence is a bug on our side, by construction.

use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/work")
        .join(name);
    assert!(
        dir.is_dir(),
        "fixture {name} missing — run fixtures/make.sh"
    );
    dir
}

#[test]
fn detection_is_cached_and_sane() {
    let p = vivacity_core::platform::Platform::detect()
        .expect("detection")
        .expect("php present on the dev machine");
    assert!(p.php_version.starts_with(|c: char| c.is_ascii_digit()));
    assert!(p.is_64bit);
    assert!(p.extensions.contains_key("json"), "ext json always present");
    // Second call: served by the cache (same result).
    let p2 = vivacity_core::platform::Platform::detect()
        .expect("detection")
        .expect("php");
    assert_eq!(p.php_version, p2.php_version);
}

#[test]
fn fixture_locks_are_installable_here() {
    let mut platform = vivacity_core::platform::Platform::detect()
        .expect("detection")
        .expect("php present");
    for name in ["laravel", "symfony", "sylius"] {
        let dir = fixture(name);
        let lock = vivacity_core::lock::Lock::read(&dir.join("composer.lock")).expect("lock");
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("composer.json")).expect("composer.json"),
        )
        .expect("manifest");
        platform.apply_overrides(&manifest);
        let failures = vivacity_core::platform::check(&lock, &platform, true, &[]);
        assert!(
            failures.is_empty(),
            "fixture {name}: unexpected platform failures: {failures:?}"
        );
    }
}
