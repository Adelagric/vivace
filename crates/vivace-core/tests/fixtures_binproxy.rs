//! Différentiel des proxies bin : pour chaque proxy de vendor/bin généré par
//! Composer dans la fixture Laravel, notre générateur doit produire les mêmes
//! octets à partir de la même cible.

use std::path::{Path, PathBuf};

fn laravel_vendor() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/work/laravel/vendor");
    assert!(
        dir.is_dir(),
        "fixture laravel absente — lancer fixtures/make.sh"
    );
    dir
}

#[test]
fn proxies_match_composer_byte_for_byte() {
    let vendor = laravel_vendor();
    let installed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(vendor.join("composer/installed.json")).expect("installed.json"),
    )
    .expect("json");

    let mut checked = 0;
    for pkg in installed["packages"].as_array().expect("packages") {
        let name = pkg["name"].as_str().expect("name");
        let Some(bins) = pkg.get("bin").and_then(|b| b.as_array()) else {
            continue;
        };
        for bin in bins {
            let bin = bin.as_str().expect("bin").trim_start_matches("./");
            let link_name = bin.rsplit_once('/').map(|(_, f)| f).unwrap_or(bin);
            let link = vendor.join("bin").join(link_name);
            if !link.exists() {
                continue;
            }
            let expected = std::fs::read_to_string(&link).expect("proxy composer");
            let ours =
                vivace_core::binproxy::proxy_content(&vendor, &link, &vendor.join(name).join(bin))
                    .expect("proxy vivace");
            assert_eq!(
                ours, expected,
                "proxy divergent pour {link_name} (paquet {name})"
            );
            checked += 1;
        }
    }
    assert!(checked >= 5, "trop peu de proxies comparés: {checked}");
}
