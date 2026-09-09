//! Store local adressé par contenu : chaque (paquet, version, référence de
//! dist) est extrait UNE fois dans `<cache>/store/<vendor>/<pkg>/<clé>/`, puis
//! cloné vers les vendor/ des projets (voir clone.rs). Écriture atomique :
//! extraction dans un dossier temporaire voisin puis `rename` — le dossier
//! final n'existe que complet, et deux processus concurrents convergent (le
//! perdant du rename jette son temporaire).

use crate::error::{Error, Result};
use crate::extract::extract_zip;
use std::path::PathBuf;

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn at(root: PathBuf) -> Store {
        Store { root }
    }

    pub fn default_location() -> Store {
        Store::at(crate::platform::cache_dir().join("store"))
    }

    /// Clé de l'entrée : version + 12 premiers hex de la référence de dist,
    /// assainis pour le système de fichiers.
    pub fn entry_path(&self, name: &str, version: &str, dist_ref: Option<&str>) -> PathBuf {
        let sane = |s: &str| -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect()
        };
        let short_ref = dist_ref.unwrap_or("noref");
        let short_ref = &short_ref[..short_ref.len().min(12)];
        self.root
            .join(name) // vendor/pkg : deux segments sûrs (validés par le lock)
            .join(format!("{}-{}", sane(version), sane(short_ref)))
    }

    /// Garantit la présence de l'entrée, en l'extrayant si nécessaire.
    /// Renvoie le chemin de l'arbre extrait.
    pub fn ensure(
        &self,
        name: &str,
        version: &str,
        dist_ref: Option<&str>,
        zip_bytes: &[u8],
    ) -> Result<PathBuf> {
        let final_path = self.entry_path(name, version, dist_ref);
        if final_path.is_dir() {
            return Ok(final_path);
        }
        let parent = final_path.parent().unwrap_or(&self.root).to_path_buf();
        std::fs::create_dir_all(&parent).map_err(Error::io(&parent))?;
        let tmp = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempdir_in(&parent)
            .map_err(Error::io(&parent))?;
        extract_zip(zip_bytes, tmp.path())?;
        let tmp_path = tmp.keep();
        match std::fs::rename(&tmp_path, &final_path) {
            Ok(()) => Ok(final_path),
            Err(_) if final_path.is_dir() => {
                // Un concurrent a gagné le rename : son entrée est complète.
                let _ = std::fs::remove_dir_all(&tmp_path);
                Ok(final_path)
            }
            Err(source) => {
                let _ = std::fs::remove_dir_all(&tmp_path);
                Err(Error::Io {
                    path: final_path,
                    source,
                })
            }
        }
    }

    pub fn contains(&self, name: &str, version: &str, dist_ref: Option<&str>) -> bool {
        self.entry_path(name, version, dist_ref).is_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    fn sample_zip() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.add_directory("root-x", SimpleFileOptions::default())
            .expect("dir");
        w.start_file("root-x/composer.json", SimpleFileOptions::default())
            .expect("f");
        w.write_all(b"{}").expect("w");
        w.finish().expect("finish").into_inner()
    }

    #[test]
    fn ensure_is_idempotent_and_atomic() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = Store::at(dir.path().join("store"));
        let zip = sample_zip();
        let p1 = store
            .ensure("a/b", "1.0.0", Some("deadbeefcafe1234"), &zip)
            .expect("ensure");
        assert!(p1.join("composer.json").is_file());
        assert!(store.contains("a/b", "1.0.0", Some("deadbeefcafe1234")));
        // Deuxième appel : mêmes octets ou pas, l'entrée existante gagne.
        let p2 = store
            .ensure("a/b", "1.0.0", Some("deadbeefcafe1234"), b"garbage")
            .expect("hit");
        assert_eq!(p1, p2);
        // Clé différente → autre entrée.
        assert!(!store.contains("a/b", "1.0.0", Some("feedfacefeed5678")));
        // Version hostile assainie (pas de traversée).
        let p3 = store.ensure("a/b", "../../evil", None, &zip).expect("sane");
        assert!(p3.starts_with(dir.path().join("store").join("a/b")));
    }
}
