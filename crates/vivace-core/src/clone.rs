//! Clone d'un arbre du store vers vendor/ — le chemin chaud de l'install.
//! macOS/APFS : `clonefile(2)` du répertoire entier (un syscall, copy-on-write,
//! mesuré 8× plus rapide que l'extraction en M0). Ailleurs, ou si clonefile
//! échoue (autre FS, volume différent) : marche récursive en hardlinks (modèle
//! pnpm), et copie réelle en dernier recours. Toujours vers une destination
//! ABSENTE (l'appelant supprime l'ancienne version avant), donc pas d'états
//! mélangés.

use crate::error::{Error, Result};
use std::path::Path;

pub fn clone_tree(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(Error::io(parent))?;
    }

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let c_src = std::ffi::CString::new(src.as_os_str().as_bytes());
        let c_dst = std::ffi::CString::new(dst.as_os_str().as_bytes());
        if let (Ok(c_src), Ok(c_dst)) = (c_src, c_dst) {
            // SAFETY-libre : appel FFI à clonefile via libc, deux chaînes C
            // valides, pas de mémoire partagée.
            let rc = unsafe { libc::clonefile(c_src.as_ptr(), c_dst.as_ptr(), 0) };
            if rc == 0 {
                return Ok(());
            }
            // Échec (FS non-APFS, volumes différents…) → stratégies suivantes.
        }
    }

    link_or_copy_tree(src, dst)
}

fn link_or_copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(Error::io(dst))?;
    for entry in std::fs::read_dir(src).map_err(Error::io(src))? {
        let entry = entry.map_err(Error::io(src))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ftype = entry.file_type().map_err(Error::io(&from))?;
        if ftype.is_dir() {
            link_or_copy_tree(&from, &to)?;
        } else if ftype.is_symlink() {
            let target = std::fs::read_link(&from).map_err(Error::io(&from))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &to).map_err(Error::io(&to))?;
        } else {
            // Hardlink d'abord (gratuit) ; copie si le FS refuse (autre volume).
            if std::fs::hard_link(&from, &to).is_err() {
                std::fs::copy(&from, &to).map_err(Error::io(&to))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_files_dirs_symlinks_and_exec_bits() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("sub")).expect("mkdir");
        std::fs::write(src.join("a.txt"), b"hello").expect("write");
        std::fs::write(src.join("sub/tool"), b"#!/bin/sh\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(src.join("sub/tool"), std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
            std::os::unix::fs::symlink("a.txt", src.join("link")).expect("ln");
        }

        let dst = tmp.path().join("dst/pkg");
        clone_tree(&src, &dst).expect("clone");
        assert_eq!(std::fs::read(dst.join("a.txt")).expect("read"), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dst.join("sub/tool"))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "bit exécutable perdu au clone");
            assert!(dst
                .join("link")
                .symlink_metadata()
                .expect("meta")
                .file_type()
                .is_symlink());
        }
        // La modification du clone ne touche pas la source (CoW ou hardlink :
        // on remplace le fichier, on ne l'édite pas en place).
        std::fs::write(dst.join("a.txt"), b"changed").expect("write");
        #[cfg(target_os = "macos")]
        assert_eq!(std::fs::read(src.join("a.txt")).expect("read"), b"hello");
    }
}
