//! Neutral, policy-free primitives shared by offload and restore.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PathIdentity {
    pub dev: u64,
    pub ino: u64,
}

pub(crate) fn path_identity(path: &Path) -> Result<PathIdentity, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect path identity: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("path became a symlink; data left intact".into());
    }
    Ok(PathIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
    })
}

pub(crate) fn ensure_same_identity(path: &Path, expected: PathIdentity) -> Result<(), String> {
    let current = path_identity(path)?;
    if current != expected {
        return Err("path changed during transfer; data left intact".into());
    }
    Ok(())
}

pub(crate) fn ensure_absent(path: &Path, label: &str) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(format!(
            "{label} already exists at {}; source left intact",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspect {label} before copy: {error}")),
    }
}

/// Logical file bytes, independent of filesystem block allocation. Symlinks
/// are never followed and contribute zero.
pub(crate) fn apparent_size(path: &Path) -> i64 {
    let mut total = 0i64;
    if let Ok(metadata) = path.symlink_metadata() {
        if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    total += apparent_size(&entry.path());
                }
            }
        } else if metadata.is_file() {
            total += metadata.len() as i64;
        }
    }
    total
}

/// Copy with ditto and verify logical bytes. This function never removes the
/// source or destination.
pub(crate) fn verified_ditto_copy(src: &Path, dest: &Path) -> Result<i64, String> {
    ensure_absent(dest, "destination")?;
    let src_apparent = apparent_size(src);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("prepare destination: {error}"))?;
    }
    let output = std::process::Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(dest)
        .output()
        .map_err(|error| format!("ditto: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "copy failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let dest_apparent = apparent_size(dest);
    if dest_apparent < src_apparent {
        return Err(format!(
            "verify failed: copied {dest_apparent} < source {src_apparent} bytes; source left intact"
        ));
    }
    Ok(src_apparent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_recheck_detects_path_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("item");
        std::fs::write(&path, b"first").unwrap();
        let identity = path_identity(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"second").unwrap();
        assert!(ensure_same_identity(&path, identity).is_err());
    }

    #[test]
    fn verified_copy_preserves_source_and_matches_apparent_size() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("nested/dest");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("data"), b"payload").unwrap();

        let copied = verified_ditto_copy(&src, &dest).unwrap();

        assert_eq!(copied, apparent_size(&src));
        assert!(src.exists());
        assert_eq!(std::fs::read(dest.join("data")).unwrap(), b"payload");
    }
}
