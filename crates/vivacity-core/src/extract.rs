//! Extraction of a zip dist into a directory, stripping the single root
//! directory of GitHub/Packagist zipballs (ArchiveDownloader's rule: strip iff
//! the archive has exactly one top-level entry and it is a directory;
//! otherwise everything is extracted as is), and a DISTRUSTFUL extraction:
//! - paths: `enclosed_name()` (rejects `..` and absolute paths);
//! - symlinks (unix mode S_IFLNK): relative target only, and the lexically
//!   resolved path must stay inside the package root;
//! - executable bits preserved (binaries depend on them);
//! - refusal of absurd decompressed sizes (crude zip bomb).

use crate::error::{Error, Result};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Maximum decompressed size of a dist (512 MB); beyond that, something is
/// wrong (the largest real packages weigh a few tens of MB).
const MAX_UNCOMPRESSED: u64 = 512 * 1024 * 1024;

const S_IFMT: u32 = 0o170000;
const S_IFLNK: u32 = 0o120000;

/// Path of an entry, or an error if it is absolute or contains `..`:
/// enclosed_name accepts an inner `..` (`r/../x` stays inside the root) that
/// stripping the first component would turn into an escape, and no
/// legitimate dist contains one.
fn entry_path(entry: &zip::read::ZipFile<'_>, dest: &Path) -> Result<PathBuf> {
    entry
        .enclosed_name()
        .filter(|p| {
            p.components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        })
        .ok_or_else(|| Error::HostileArchive {
            dest: dest.to_path_buf(),
            reason: format!("invalid entry path: {:?}", entry.name()),
        })
}

/// Number of leading components to strip from each entry: 1 if the archive
/// has exactly one top-level entry and it is a directory
/// (`ArchiveDownloader::install`, `$singleDirAtTopLevel`; a top-level
/// `.DS_Store` is not counted, and then disappears along with the root
/// directory), 0 otherwise, in which case everything is moved, `.DS_Store`
/// included (`rename($temporaryDir, $path)` onto an empty target).
fn root_strip(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, dest: &Path) -> Result<usize> {
    let mut top: std::collections::BTreeMap<std::ffi::OsString, bool> =
        std::collections::BTreeMap::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(Error::zip(dest))?;
        let raw = entry_path(&entry, dest)?;
        let mut comps = raw
            .components()
            .filter(|c| matches!(c, Component::Normal(_)));
        let Some(Component::Normal(first)) = comps.next() else {
            continue;
        };
        let is_dir = entry.is_dir() || comps.next().is_some();
        if !is_dir && first == ".DS_Store" {
            continue;
        }
        *top.entry(first.to_owned()).or_insert(false) |= is_dir;
    }
    Ok(usize::from(top.len() == 1 && top.values().all(|d| *d)))
}

/// Memoized `create_dir_all`: avoids one `create_dir_all` (hence one stat +
/// N syscalls) per zip entry when hundreds of files share the same parent
/// directory. Inserts the path and its newly created ancestors.
fn ensure_dir(p: &Path, made: &mut std::collections::HashSet<PathBuf>) -> Result<()> {
    if made.contains(p) {
        return Ok(());
    }
    std::fs::create_dir_all(p).map_err(Error::io(p))?;
    let mut cur = Some(p);
    while let Some(c) = cur {
        if !made.insert(c.to_path_buf()) {
            break; // ancestor already known: so is the rest
        }
        cur = c.parent();
    }
    Ok(())
}

pub fn extract_zip(zip_bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(Error::zip(dest))?;
    let mut made: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    ensure_dir(dest, &mut made)?;
    let strip = root_strip(&mut archive, dest)?;

    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(Error::zip(dest))?;
        total = total.saturating_add(entry.size());
        if total > MAX_UNCOMPRESSED {
            return Err(Error::HostileArchive {
                dest: dest.to_path_buf(),
                reason: format!("uncompressed size > {MAX_UNCOMPRESSED} bytes"),
            });
        }
        let raw = entry_path(&entry, dest)?;
        let stripped: PathBuf = raw
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .skip(strip)
            .collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&stripped);

        let mode = entry.unix_mode();
        let is_symlink = mode.is_some_and(|m| m & S_IFMT == S_IFLNK);

        if entry.is_dir() {
            ensure_dir(&out, &mut made)?;
        } else if is_symlink {
            let mut target = String::new();
            entry.read_to_string(&mut target).map_err(Error::io(&out))?;
            check_symlink_target(&stripped, &target, dest)?;
            if let Some(p) = out.parent() {
                ensure_dir(p, &mut made)?;
            }
            let _ = std::fs::remove_file(&out);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &out).map_err(Error::io(&out))?;
            // Off Unix: PHP ZipArchive (what Composer uses on Windows) does
            // not recreate symlinks — the entry becomes an ordinary file
            // whose content is the target. Same here, after the same
            // hostility checks.
            #[cfg(not(unix))]
            std::fs::write(&out, target.as_bytes()).map_err(Error::io(&out))?;
        } else {
            if let Some(p) = out.parent() {
                ensure_dir(p, &mut made)?;
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

/// A symlink target must be relative and stay lexically inside the extracted
/// root (the classic attack: `link -> ../../../../etc/passwd`).
fn check_symlink_target(link_rel: &Path, target: &str, dest: &Path) -> Result<()> {
    let hostile = |reason: String| Error::HostileArchive {
        dest: dest.to_path_buf(),
        reason,
    };
    let target_path = Path::new(target);
    if target_path.is_absolute() {
        return Err(hostile(format!(
            "absolute symlink: {link_rel:?} -> {target}"
        )));
    }
    let mut depth: i64 = link_rel.components().count() as i64 - 1; // depth of the link's directory
    for c in target_path.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(hostile(format!(
                        "symlink escaping the archive: {link_rel:?} -> {target}"
                    )));
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => {
                return Err(hostile(format!(
                    "invalid symlink: {link_rel:?} -> {target}"
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
            assert_eq!(mode & 0o111, 0o111, "executable bit lost");
        }
    }

    #[test]
    fn strips_only_a_single_top_level_directory() {
        // Single directory without an explicit directory entry: strip.
        let single = build_zip(&[("pkg/a.txt", b"a", None), ("pkg/sub/b.txt", b"b", None)]);
        let d = tmpdir();
        extract_zip(&single, d.path()).expect("extract");
        assert!(d.path().join("a.txt").is_file() && d.path().join("sub/b.txt").is_file());

        // File at the root + directory: nothing is stripped, nothing is lost.
        let mixed = build_zip(&[("README", b"r", None), ("src/a.php", b"<?php", None)]);
        let d = tmpdir();
        extract_zip(&mixed, d.path()).expect("extract");
        assert!(d.path().join("README").is_file(), "root file lost");
        assert!(
            d.path().join("src/a.php").is_file(),
            "directory wrongly flattened"
        );

        // Two top-level directories: nothing is stripped.
        let two = build_zip(&[("a/x", b"x", None), ("b/y", b"y", None)]);
        let d = tmpdir();
        extract_zip(&two, d.path()).expect("extract");
        assert!(d.path().join("a/x").is_file() && d.path().join("b/y").is_file());

        // A single file at the root: not a directory, no strip.
        let file = build_zip(&[("only.txt", b"o", None)]);
        let d = tmpdir();
        extract_zip(&file, d.path()).expect("extract");
        assert!(d.path().join("only.txt").is_file(), "single file lost");

        // Top-level .DS_Store: ignored for the count (the single directory is
        // stripped, it disappears); without a single directory, extracted like
        // the rest.
        let ds = build_zip(&[(".DS_Store", b"junk", None), ("pkg/a.txt", b"a", None)]);
        let d = tmpdir();
        extract_zip(&ds, d.path()).expect("extract");
        assert!(d.path().join("a.txt").is_file());
        assert!(!d.path().join(".DS_Store").exists());
        let ds2 = build_zip(&[(".DS_Store", b"junk", None), ("a.txt", b"a", None)]);
        let d = tmpdir();
        extract_zip(&ds2, d.path()).expect("extract");
        assert!(d.path().join("a.txt").is_file() && d.path().join(".DS_Store").is_file());
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
        #[cfg(unix)]
        assert!(meta.file_type().is_symlink());
        #[cfg(not(unix))]
        {
            // Like PHP ZipArchive: an ordinary file containing the target.
            assert!(meta.file_type().is_file());
            assert_eq!(
                std::fs::read(d.path().join("sub/link")).expect("read"),
                b"../real.txt"
            );
        }

        for target in ["../../etc/passwd", "/etc/passwd", "../../../x"] {
            let bad = build_zip_with_symlink("r/link", target);
            let d = tmpdir();
            assert!(
                extract_zip(&bad, d.path()).is_err(),
                "hostile symlink accepted: {target}"
            );
        }
    }

    #[test]
    fn zip_slip_paths_are_rejected() {
        // The writer sanitises names: we forge the `..` by byte-patching
        // (same length), as a crafted archive would.
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
        assert_ne!(benign, patched, "the patch replaced nothing");
        let d = tmpdir();
        assert!(
            extract_zip(&patched, d.path()).is_err(),
            "zip-slip accepted"
        );
    }
}
