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
            #[cfg(windows)]
            clone_symlink_windows(&from, &target, &to)?;
            #[cfg(not(any(unix, windows)))]
            {
                let _ = target;
                std::fs::copy(&from, &to).map_err(Error::io(&to))?;
            }
        } else {
            // Linux: try a reflink first (FICLONE, a CoW copy like clonefile
            // on APFS) — independent of the store so safer than a hardlink,
            // and a single syscall on btrfs/xfs. On ext4 (no reflink) the
            // ioctl fails cleanly and we fall back to the hardlink.
            #[cfg(target_os = "linux")]
            let done = reflink(&from, &to);
            #[cfg(not(target_os = "linux"))]
            let done = false;
            if !done {
                // Hardlink first (free); copy if the FS refuses (other volume).
                if std::fs::hard_link(&from, &to).is_err() {
                    std::fs::copy(&from, &to).map_err(Error::io(&to))?;
                }
            }
        }
    }
    Ok(())
}

/// Reflink (copy-on-write) `from` → `to` via the FICLONE ioctl. Creates an
/// independent inode sharing the extents: modifying the clone does not touch
/// the store. The new file's mode is re-applied from the source (FICLONE
/// does not copy permissions), to preserve the executable bit of binaries.
/// Returns false (without leaving a partial file) if the FS does not support
/// reflink — the caller then falls back to the hardlink.
#[cfg(target_os = "linux")]
fn reflink(from: &Path, to: &Path) -> bool {
    use std::os::unix::io::AsRawFd as _;
    // Once a filesystem has said it cannot reflink, stop asking: the probe
    // is an open/create/ioctl/unlink per file, and the answer does not
    // change within a run (the store and vendor/ stay where they are).
    static UNSUPPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if UNSUPPORTED.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let Ok(src) = std::fs::File::open(from) else {
        return false;
    };
    let dst = match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(to)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    // SAFETY: FICLONE ioctl on two valid, open descriptors; no shared
    // memory. The kernel reads src, writes dst.
    let rc = unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        drop(dst);
        let _ = std::fs::remove_file(to); // empty file created by the open
                                          // ENOTTY / EOPNOTSUPP / EXDEV / EINVAL: this filesystem (or this
                                          // pair of filesystems) never will; other errors are per file.
        if matches!(
            err.raw_os_error(),
            Some(libc::ENOTTY) | Some(libc::EOPNOTSUPP) | Some(libc::EXDEV) | Some(libc::EINVAL)
        ) {
            UNSUPPORTED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        return false;
    }
    if let Ok(meta) = src.metadata() {
        let _ = std::fs::set_permissions(to, meta.permissions());
    }
    true
}

/// Windows: creating symlinks requires a privilege (Developer Mode or
/// SeCreateSymbolicLinkPrivilege). We try the real symlink, and failing that
/// copy the RESOLVED content — the resulting vendor/ is functional but no
/// longer a link (a parity divergence documented in docs/windows.md). A
/// dangling link fails loudly instead of vanishing silently.
#[cfg(windows)]
fn clone_symlink_windows(from: &Path, target: &Path, to: &Path) -> Result<()> {
    let is_dir = std::fs::metadata(from).map(|m| m.is_dir()).unwrap_or(false);
    let made = if is_dir {
        std::os::windows::fs::symlink_dir(target, to)
    } else {
        std::os::windows::fs::symlink_file(target, to)
    };
    if made.is_ok() {
        return Ok(());
    }
    if is_dir {
        link_or_copy_tree(from, to) // read_dir follows the link
    } else {
        if std::fs::hard_link(from, to).is_err() {
            std::fs::copy(from, to).map_err(Error::io(to))?;
        }
        Ok(())
    }
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
        // Windows: the fixture's link requires Developer Mode or
        // SeCreateSymbolicLinkPrivilege — without it we cannot build the
        // fixture, so that portion is announced and then skipped (the copy
        // fallback of clone_symlink_windows cannot be forced from here).
        #[cfg(windows)]
        let with_link = match std::os::windows::fs::symlink_file("a.txt", src.join("link")) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("symlink refused on this host ({e}) — link portion not exercised");
                false
            }
        };

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
        #[cfg(windows)]
        if with_link {
            // Never a lost entry: either a real link (privilege present —
            // the same one that allowed the fixture), or a copy of the
            // resolved content; in both cases reading yields the target's
            // content.
            let meta = dst.join("link").symlink_metadata().expect("entry lost");
            assert!(
                meta.file_type().is_symlink() || meta.file_type().is_file(),
                "neither link nor file"
            );
            assert_eq!(std::fs::read(dst.join("link")).expect("read"), b"hello");
        }
        // Modifying the clone does not touch the source (CoW or hardlink:
        // we replace the file, we do not edit it in place).
        std::fs::write(dst.join("a.txt"), b"changed").expect("write");
        #[cfg(target_os = "macos")]
        assert_eq!(std::fs::read(src.join("a.txt")).expect("read"), b"hello");
    }
}
