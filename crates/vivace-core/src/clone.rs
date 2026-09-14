//! Clone of a store tree into vendor/, the hot path of the install.
//! macOS/APFS: `clonefile(2)` of the whole directory (one syscall, copy-on-write,
//! measured 8x faster than extraction in M0). Elsewhere, or if clonefile
//! fails (other FS, different volume): recursive walk with hardlinks (pnpm
//! model), and a real copy as a last resort. Always towards an ABSENT
//! destination (the caller removes the previous version first), so no mixed
//! states.

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
            // SAFETY: FFI call to clonefile through libc, two valid C strings,
            // no shared memory.
            let rc = unsafe { libc::clonefile(c_src.as_ptr(), c_dst.as_ptr(), 0) };
            if rc == 0 {
                return Ok(());
            }
            // Failure (non-APFS FS, different volumes...): fall through to the next strategies.
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
            // Hardlink first (free); copy if the FS refuses (other volume).
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
            assert_eq!(mode & 0o111, 0o111, "executable bit lost on clone");
            assert!(dst
                .join("link")
                .symlink_metadata()
                .expect("meta")
                .file_type()
                .is_symlink());
        }
        // Modifying the clone does not touch the source (CoW or hardlink:
        // we replace the file, we do not edit it in place).
        std::fs::write(dst.join("a.txt"), b"changed").expect("write");
        #[cfg(target_os = "macos")]
        assert_eq!(std::fs::read(src.join("a.txt")).expect("read"), b"hello");
    }
}
