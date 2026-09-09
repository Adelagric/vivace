//! Port de `Composer\Package\Locker::getContentHash` (2.10.3, voir
//! docs/reference/Locker.php) : md5 d'un sous-ensemble de composer.json
//! ré-encodé via `JsonFile::encode($relevantContent, 0)`.

use crate::error::{Error, Result};
use crate::phpjson::php_json_encode;
use md5::{Digest, Md5};
use serde_json::{Map, Value};

/// Ordre canonique de $relevantKeys dans Locker::getContentHash. L'ordre
/// d'insertion importe peu (ksort suit) mais on le préserve par fidélité.
const RELEVANT_KEYS: [&str; 11] = [
    "name",
    "version",
    "require",
    "require-dev",
    "conflict",
    "replace",
    "provide",
    "minimum-stability",
    "prefer-stable",
    "repositories",
    "extra",
];

pub fn content_hash(composer_json_text: &str) -> Result<String> {
    let content: Value =
        serde_json::from_str(composer_json_text).map_err(|source| Error::Json {
            context: "composer.json".to_owned(),
            source,
        })?;

    let mut relevant = Map::new();
    if let Value::Object(obj) = &content {
        for key in RELEVANT_KEYS {
            if let Some(v) = obj.get(key) {
                relevant.insert(key.to_owned(), v.clone());
            }
        }
        if let Some(platform) = obj.get("config").and_then(|c| c.get("platform")) {
            let mut config = Map::new();
            config.insert("platform".to_owned(), platform.clone());
            relevant.insert("config".to_owned(), Value::Object(config));
        }
    }

    // ksort($relevantContent) : tri des clés de premier niveau. Toutes les clés
    // possibles ici sont non numériques → l'ordre lexicographique octet à octet
    // de PHP (strcmp) est celui de Rust.
    let mut entries: Vec<(String, Value)> = relevant.into_iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    let sorted: Map<String, Value> = entries.into_iter().collect();

    let encoded = php_json_encode(&Value::Object(sorted))?;
    let mut hasher = Md5::new();
    hasher.update(encoded.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_manifest_is_stable() {
        // Vecteur auto-généré puis figé après validation contre l'oracle PHP
        // (tests/oracle_content_hash.rs fait la validation vivante).
        let h = content_hash(r#"{"require":{"php":">=8.1"}}"#).expect("hash");
        assert_eq!(h.len(), 32);
        // Les clés hors liste ne participent pas au hash.
        let h2 = content_hash(r#"{"require":{"php":">=8.1"},"description":"x"}"#).expect("hash");
        assert_eq!(h, h2);
        // Les clés pertinentes si.
        let h3 = content_hash(r#"{"require":{"php":">=8.2"}}"#).expect("hash");
        assert_ne!(h, h3);
    }

    #[test]
    fn config_platform_is_renested() {
        let a =
            content_hash(r#"{"require":{},"config":{"platform":{"php":"8.2.0"}}}"#).expect("hash");
        let b = content_hash(r#"{"require":{},"config":{"sort-packages":true}}"#).expect("hash");
        let c = content_hash(r#"{"require":{}}"#).expect("hash");
        assert_ne!(a, c); // config.platform compte
        assert_eq!(b, c); // le reste de config ne compte pas
    }
}
