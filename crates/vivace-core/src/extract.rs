//! Extraction d'une dist zip vers un répertoire, avec le strip du dossier
//! racine unique des zipballs GitHub/Packagist, et une extraction MÉFIANTE :
//! - chemins : `enclosed_name()` (rejette `..` et absolus) ;
//! - symlinks (mode unix S_IFLNK) : cible relative uniquement, et le chemin
//!   résolu lexicalement doit rester dans la racine du paquet ;
//! - bits exécutables préservés (les binaires en dépendent) ;
//! - refus des tailles décompressées aberrantes (zip bomb grossière).

use crate::error::{Error, Result};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Taille décompressée maximale d'une dist (512 Mo) — au-delà, quelque chose
/// ne va pas (les plus gros paquets réels font quelques dizaines de Mo).
const MAX_UNCOMPRESSED: u64 = 512 * 1024 * 1024;

const S_IFMT: u32 = 0o170000;
const S_IFLNK: u32 = 0o120000;

pub fn extract_zip(zip_bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(Error::zip(dest))?;
    std::fs::create_dir_all(dest).map_err(Error::io(dest))?;

    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(Error::zip(dest))?;
        total = total.saturating_add(entry.size());
        if total > MAX_UNCOMPRESSED {
            return Err(Error::HostileArchive {
                dest: dest.to_path_buf(),
                reason: format!("taille décompressée > {MAX_UNCOMPRESSED} octets"),
            });
        }
        // enclosed_name refuse les chemins absolus et ceux qui s'échappent —
        // mais accepte un `..` interne (`r/../x` reste dans la racine), que
        // notre strip du premier composant transformerait en évasion. Aucune
        // dist légitime ne contient `..` : rejet pur et simple.
        let raw = entry.enclosed_name().filter(|p| {
            p.components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        });
        let Some(raw) = raw else {
            return Err(Error::HostileArchive {
                dest: dest.to_path_buf(),
                reason: format!("chemin d'entrée invalide: {:?}", entry.name()),
            });
        };
        // Strip du dossier racine unique (zipball GitHub: `owner-repo-sha/…`).
        let stripped: PathBuf = raw.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&stripped);

        let mode = entry.unix_mode();
        let is_symlink = mode.is_some_and(|m| m & S_IFMT == S_IFLNK);

        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(Error::io(&out))?;
        } else if is_symlink {
            let mut target = String::new();
            entry.read_to_string(&mut target).map_err(Error::io(&out))?;
            check_symlink_target(&stripped, &target, dest)?;
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(Error::io(p))?;
            }
            let _ = std::fs::remove_file(&out);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &out).map_err(Error::io(&out))?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(Error::io(p))?;
            }
            let mut buf = Vec::with_capacity(entry.size().min(MAX_UNCOMPRESSED) as usize);
            entry.read_to_end(&mut buf).map_err(Error::io(&out))?;
            std::fs::write(&out, &buf).map_err(Error::io(&out))?;
            #[cfg(unix)]
            if let Some(m) = mode {
                if m & 0o111 != 0 {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))
                        .map_err(Error::io(&out))?;
                }
            }
        }
    }
    Ok(())
}

/// La cible d'un symlink doit être relative et rester lexicalement dans la
/// racine extraite (l'attaque classique : `link -> ../../../../etc/passwd`).
fn check_symlink_target(link_rel: &Path, target: &str, dest: &Path) -> Result<()> {
    let hostile = |reason: String| Error::HostileArchive {
        dest: dest.to_path_buf(),
        reason,
    };
    let target_path = Path::new(target);
    if target_path.is_absolute() {
        return Err(hostile(format!("symlink absolu: {link_rel:?} -> {target}")));
    }
    let mut depth: i64 = link_rel.components().count() as i64 - 1; // profondeur du dossier du lien
    for c in target_path.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(hostile(format!(
                        "symlink sortant de l'archive: {link_rel:?} -> {target}"
                    )));
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => {
                return Err(hostile(format!(
                    "symlink invalide: {link_rel:?} -> {target}"
                )))
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    fn build_zip(entries: &[(&str, &[u8], Option<u32>)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content, mode) in entries {
            let mut opts = SimpleFileOptions::default();
            if let Some(m) = mode {
                opts = opts.unix_permissions(*m);
            }
            if name.ends_with('/') {
                w.add_directory(name.trim_end_matches('/'), opts)
                    .expect("dir");
            } else {
                w.start_file(*name, opts).expect("start");
                w.write_all(content).expect("write");
            }
        }
        w.finish().expect("finish").into_inner()
    }

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tmpdir")
    }

    #[test]
    fn strips_root_and_preserves_exec_bit() {
        let zip = build_zip(&[
            ("root-abc/", b"", None),
            ("root-abc/src/a.php", b"<?php", None),
            (
                "root-abc/bin/tool",
                b"#!/usr/bin/env php\n<?php",
                Some(0o100755),
            ),
        ]);
        let d = tmpdir();
        extract_zip(&zip, d.path()).expect("extract");
        assert!(d.path().join("src/a.php").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(d.path().join("bin/tool"))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "bit exécutable perdu");
        }
    }

    fn build_zip_with_symlink(link_name: &str, target: &str) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.add_directory("r", SimpleFileOptions::default())
            .expect("dir");
        w.start_file("r/real.txt", SimpleFileOptions::default())
            .expect("start");
        w.write_all(b"x").expect("write");
        w.add_symlink(link_name, target, SimpleFileOptions::default())
            .expect("symlink");
        w.finish().expect("finish").into_inner()
    }

    #[test]
    fn valid_symlink_is_recreated_hostile_ones_rejected() {
        let ok = build_zip_with_symlink("r/sub/link", "../real.txt");
        let d = tmpdir();
        extract_zip(&ok, d.path()).expect("extract");
        let meta = d.path().join("sub/link").symlink_metadata().expect("meta");
        assert!(meta.file_type().is_symlink());

        for target in ["../../etc/passwd", "/etc/passwd", "../../../x"] {
            let bad = build_zip_with_symlink("r/link", target);
            let d = tmpdir();
            assert!(
                extract_zip(&bad, d.path()).is_err(),
                "symlink hostile accepté: {target}"
            );
        }
    }

    #[test]
    fn zip_slip_paths_are_rejected() {
        // Le writer assainit les noms : on fabrique le `..` par byte-patch
        // (même longueur), comme le ferait une archive forgée.
        let benign = build_zip(&[("r/", b"", None), ("r/AA/evil.txt", b"x", None)]);
        let patched: Vec<u8> = {
            let needle = b"r/AA/evil.txt";
            let replacement = b"r/../evil.txt";
            let mut bytes = benign.clone();
            let mut i = 0;
            while i + needle.len() <= bytes.len() {
                if &bytes[i..i + needle.len()] == needle {
                    bytes[i..i + needle.len()].copy_from_slice(replacement);
                }
                i += 1;
            }
            bytes
        };
        assert_ne!(benign, patched, "le patch n'a rien remplacé");
        let d = tmpdir();
        assert!(extract_zip(&patched, d.path()).is_err(), "zip-slip accepté");
    }
}
