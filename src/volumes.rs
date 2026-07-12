use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedVolume {
    pub name: String,
    pub mount_path: PathBuf,
    pub fs_type: String,
    pub total_bytes: i64,
    pub free_bytes: i64,
    pub read_only: bool,
    pub device_id: u64,
}

pub fn eligible_mount(fs_type: &str, local: bool, is_symlink: bool) -> bool {
    local && !is_symlink && !matches!(fs_type, "autofs" | "nfs" | "smbfs" | "afpfs" | "webdav")
}

fn statfs_info(path: &Path) -> Option<(String, i64, i64, bool, bool)> {
    let cpath = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a zeroed statfs is valid, the C string is NUL-terminated, and
    // every field is read only after libc reports success.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let fs_type = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let block_size = stat.f_bsize as i64;
    let total_bytes = (stat.f_blocks as i64).saturating_mul(block_size);
    let free_bytes = (stat.f_bavail as i64).saturating_mul(block_size);
    let read_only = stat.f_flags & (libc::MNT_RDONLY as u32) != 0;
    let local = stat.f_flags & (libc::MNT_LOCAL as u32) != 0;
    Some((fs_type, total_bytes, free_bytes, read_only, local))
}

pub fn inspect_mounted_volume(path: &Path) -> Option<MountedVolume> {
    if path.parent() != Some(Path::new("/Volumes")) || path.file_name().is_none() {
        return None;
    }
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let is_symlink = metadata.file_type().is_symlink();
    if !metadata.is_dir() || is_symlink {
        return None;
    }
    let (fs_type, total_bytes, free_bytes, read_only, local) = statfs_info(path)?;
    if !eligible_mount(&fs_type, local, is_symlink) {
        return None;
    }
    Some(MountedVolume {
        name: path.file_name()?.to_string_lossy().into_owned(),
        mount_path: path.to_path_buf(),
        fs_type,
        total_bytes,
        free_bytes,
        read_only,
        device_id: metadata.dev(),
    })
}

pub fn is_same_mount(expected: &MountedVolume, current: &MountedVolume) -> bool {
    expected.mount_path == current.mount_path
        && expected.fs_type == current.fs_type
        && expected.device_id == current.device_id
}

pub fn sort_volumes(volumes: &mut [MountedVolume]) {
    volumes.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| {
                left.mount_path
                    .as_os_str()
                    .as_bytes()
                    .cmp(right.mount_path.as_os_str().as_bytes())
            })
    });
}

pub fn mounted_external_volumes() -> Vec<MountedVolume> {
    let Ok(entries) = std::fs::read_dir("/Volumes") else {
        return Vec::new();
    };
    let mut volumes: Vec<_> = entries
        .flatten()
        .filter_map(|entry| inspect_mounted_volume(&entry.path()))
        .collect();
    sort_volumes(&mut volumes);
    volumes
}

#[cfg(test)]
mod tests {
    use super::{eligible_mount, is_same_mount, sort_volumes, MountedVolume};
    use std::path::PathBuf;

    fn volume(name: &str, path: &str, fs_type: &str, device_id: u64) -> MountedVolume {
        MountedVolume {
            name: name.into(),
            mount_path: PathBuf::from(path),
            fs_type: fs_type.into(),
            total_bytes: 500,
            free_bytes: 125,
            read_only: false,
            device_id,
        }
    }

    #[test]
    fn external_volume_policy_accepts_local_media_only() {
        assert!(eligible_mount("apfs", true, false));
        assert!(eligible_mount("exfat", true, false));
        assert!(!eligible_mount("smbfs", true, false));
        assert!(!eligible_mount("apfs", false, false));
        assert!(!eligible_mount("apfs", true, true));
    }

    #[test]
    fn external_volumes_sort_case_insensitively_then_by_raw_path() {
        let mut values = vec![
            volume("beta", "/Volumes/beta", "apfs", 3),
            volume("alpha", "/Volumes/alpha-z", "apfs", 2),
            volume("Alpha", "/Volumes/alpha-a", "apfs", 1),
        ];

        sort_volumes(&mut values);

        let paths: Vec<_> = values
            .iter()
            .map(|value| value.mount_path.as_path())
            .collect();
        assert_eq!(
            paths,
            vec![
                std::path::Path::new("/Volumes/alpha-a"),
                std::path::Path::new("/Volumes/alpha-z"),
                std::path::Path::new("/Volumes/beta"),
            ]
        );
    }

    #[test]
    fn mount_identity_requires_path_filesystem_and_device() {
        let expected = volume("Drive", "/Volumes/Drive", "apfs", 42);
        assert!(is_same_mount(
            &expected,
            &volume("Renamed label", "/Volumes/Drive", "apfs", 42)
        ));
        assert!(!is_same_mount(
            &expected,
            &volume("Drive", "/Volumes/Drive", "exfat", 42)
        ));
        assert!(!is_same_mount(
            &expected,
            &volume("Drive", "/Volumes/Drive", "apfs", 43)
        ));
        assert!(!is_same_mount(
            &expected,
            &volume("Drive", "/Volumes/Other", "apfs", 42)
        ));
    }
}
