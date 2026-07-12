use crate::scan::DATA_ROOT;
use crate::volumes::{eligible_mount, inspect_local_filesystem, LocalFilesystem};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const PICKER_SCRIPT: &str = r#"
try
    set chosenFolder to choose folder with prompt "Choose a folder for DiskDeck to inspect"
    return "PATH:" & POSIX path of chosenFolder
on error number -128
    return "CANCEL"
end try
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderTarget {
    pub name: String,
    pub path: PathBuf,
    pub fs_type: String,
    pub total_bytes: i64,
    pub free_bytes: i64,
    pub read_only: bool,
    pub device_id: u64,
    pub inode: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderBlock {
    NotAbsolute,
    DotComponent,
    Missing,
    NotDirectory,
    Symlink,
    SymlinkAncestor,
    Network,
    WholeVolume,
    Unavailable,
}

impl FolderBlock {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAbsolute | Self::DotComponent => {
                "Choose a direct folder instead of a relative path."
            }
            Self::Missing => "That folder is no longer available. Choose it again.",
            Self::NotDirectory => "Choose one folder, not a file.",
            Self::Symlink | Self::SymlinkAncestor => {
                "Choose the real folder instead of a symbolic link."
            }
            Self::Network => "Folder Lens supports local folders only.",
            Self::WholeVolume => "Use Macintosh HD or External drives for a whole-volume scan.",
            Self::Unavailable => "That folder cannot be inspected right now.",
        }
    }
}

fn has_raw_dot_component(path: &Path) -> bool {
    path.as_os_str()
        .as_bytes()
        .split(|byte| *byte == b'/')
        .any(|part| part == b"." || part == b"..")
}

fn direct_volume_root(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::RootDir))
        && matches!(components.next(), Some(Component::Normal(name)) if name == "Volumes")
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}

fn classify_folder_shape(path: &Path) -> Result<(), FolderBlock> {
    if !path.is_absolute() {
        return Err(FolderBlock::NotAbsolute);
    }
    if has_raw_dot_component(path) {
        return Err(FolderBlock::DotComponent);
    }
    if path == Path::new("/") || path == Path::new(DATA_ROOT) || direct_volume_root(path) {
        return Err(FolderBlock::WholeVolume);
    }
    Ok(())
}

fn classify_filesystem(filesystem: &LocalFilesystem) -> Result<(), FolderBlock> {
    if eligible_mount(&filesystem.fs_type, filesystem.local, false) {
        Ok(())
    } else {
        Err(FolderBlock::Network)
    }
}

fn has_symlink_ancestor(path: &Path) -> Result<bool, FolderBlock> {
    for ancestor in path.ancestors().skip(1) {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(|_| FolderBlock::Unavailable)?;
        if metadata.file_type().is_symlink() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn inspect_folder(path: &Path) -> Result<FolderTarget, FolderBlock> {
    classify_folder_shape(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            FolderBlock::Missing
        } else {
            FolderBlock::Unavailable
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(FolderBlock::Symlink);
    }
    if !metadata.is_dir() {
        return Err(FolderBlock::NotDirectory);
    }
    if has_symlink_ancestor(path)? {
        return Err(FolderBlock::SymlinkAncestor);
    }
    let filesystem = inspect_local_filesystem(path).ok_or(FolderBlock::Unavailable)?;
    classify_filesystem(&filesystem)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or(FolderBlock::WholeVolume)?;
    Ok(FolderTarget {
        name,
        path: path.to_path_buf(),
        fs_type: filesystem.fs_type,
        total_bytes: filesystem.total_bytes,
        free_bytes: filesystem.free_bytes,
        read_only: filesystem.read_only,
        device_id: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub fn is_same_folder(expected: &FolderTarget, current: &FolderTarget) -> bool {
    expected.path == current.path
        && expected.fs_type == current.fs_type
        && expected.device_id == current.device_id
        && expected.inode == current.inode
}

pub fn parse_picker_output(mut bytes: Vec<u8>) -> Result<Option<PathBuf>, String> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    if bytes == b"CANCEL" {
        return Ok(None);
    }
    let path = bytes
        .strip_prefix(b"PATH:")
        .ok_or("folder picker returned an unsupported response")?;
    if path.is_empty() {
        return Err("folder picker returned an empty path".into());
    }
    Ok(Some(PathBuf::from(OsString::from_vec(path.to_vec()))))
}

pub fn choose_folder() -> Result<Option<PathBuf>, String> {
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", PICKER_SCRIPT])
        .output()
        .map_err(|error| format!("open folder chooser: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "folder chooser failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_picker_output(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volumes::LocalFilesystem;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    fn local_filesystem(read_only: bool) -> LocalFilesystem {
        LocalFilesystem {
            fs_type: "apfs".into(),
            total_bytes: 500,
            free_bytes: 125,
            read_only,
            local: true,
        }
    }

    #[test]
    fn folder_policy_accepts_a_direct_local_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let direct = tmp.path().canonicalize().unwrap();
        let target = inspect_folder(&direct).unwrap();
        let metadata = std::fs::symlink_metadata(&direct).unwrap();
        assert_eq!(target.path, direct);
        assert_eq!(target.device_id, metadata.dev());
        assert_eq!(target.inode, metadata.ino());
        assert!(!target.fs_type.is_empty());
        assert!(target.total_bytes > 0);
    }

    #[test]
    fn folder_policy_rejects_missing_file_relative_symlink_and_whole_volume_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file.txt");
        std::fs::write(&file, b"fixture").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(tmp.path(), &link).unwrap();

        assert_eq!(
            inspect_folder(Path::new("relative")),
            Err(FolderBlock::NotAbsolute)
        );
        assert_eq!(
            inspect_folder(&tmp.path().join("missing")),
            Err(FolderBlock::Missing)
        );
        assert_eq!(inspect_folder(&file), Err(FolderBlock::NotDirectory));
        assert_eq!(inspect_folder(&link), Err(FolderBlock::Symlink));
        assert_eq!(
            classify_folder_shape(Path::new("/")),
            Err(FolderBlock::WholeVolume)
        );
        assert_eq!(
            classify_folder_shape(Path::new("/System/Volumes/Data")),
            Err(FolderBlock::WholeVolume)
        );
        assert_eq!(
            classify_folder_shape(Path::new("/Volumes/Archive")),
            Err(FolderBlock::WholeVolume)
        );
    }

    #[test]
    fn folder_policy_rejects_dot_segments_and_symlinked_ancestors() {
        assert_eq!(
            classify_folder_shape(Path::new("/tmp/./folder")),
            Err(FolderBlock::DotComponent)
        );
        assert_eq!(
            classify_folder_shape(Path::new("/tmp/../folder")),
            Err(FolderBlock::DotComponent)
        );

        let container = tempfile::tempdir().unwrap();
        let target = container.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let child = target.join("child");
        std::fs::create_dir(&child).unwrap();
        let link = container.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            inspect_folder(&link.join("child")),
            Err(FolderBlock::SymlinkAncestor)
        );
    }

    #[test]
    fn filesystem_policy_accepts_read_only_local_and_rejects_network() {
        assert_eq!(classify_filesystem(&local_filesystem(true)), Ok(()));

        let mut network = local_filesystem(false);
        network.fs_type = "smbfs".into();
        assert_eq!(classify_filesystem(&network), Err(FolderBlock::Network));

        let mut non_local = local_filesystem(false);
        non_local.local = false;
        assert_eq!(classify_filesystem(&non_local), Err(FolderBlock::Network));
    }

    #[test]
    fn folder_identity_requires_path_filesystem_device_and_inode_only() {
        let tmp = tempfile::tempdir().unwrap();
        let direct = tmp.path().canonicalize().unwrap();
        let expected = inspect_folder(&direct).unwrap();
        assert!(is_same_folder(&expected, &expected));

        let mut changed = expected.clone();
        changed.path.push("different");
        assert!(!is_same_folder(&expected, &changed));
        changed = expected.clone();
        changed.fs_type = "exfat".into();
        assert!(!is_same_folder(&expected, &changed));
        changed = expected.clone();
        changed.device_id = changed.device_id.wrapping_add(1);
        assert!(!is_same_folder(&expected, &changed));
        changed = expected.clone();
        changed.inode = changed.inode.wrapping_add(1);
        assert!(!is_same_folder(&expected, &changed));

        changed = expected.clone();
        changed.free_bytes = changed.free_bytes.saturating_sub(1);
        changed.read_only = !changed.read_only;
        assert!(is_same_folder(&expected, &changed));
    }

    #[test]
    fn picker_parser_preserves_path_bytes_and_cancellation() {
        assert_eq!(parse_picker_output(b"CANCEL\n".to_vec()).unwrap(), None);
        assert_eq!(
            parse_picker_output(b"PATH:/tmp/Folder Name/\n".to_vec()).unwrap(),
            Some(PathBuf::from("/tmp/Folder Name/"))
        );
        assert_eq!(
            parse_picker_output("PATH:/tmp/फ़ोल्डर/\n".as_bytes().to_vec()).unwrap(),
            Some(PathBuf::from("/tmp/फ़ोल्डर/"))
        );
        assert_eq!(
            parse_picker_output(b"PATH:/tmp/line\nname/\n".to_vec()).unwrap(),
            Some(PathBuf::from("/tmp/line\nname/"))
        );
        assert!(parse_picker_output(b"PATH:\n".to_vec()).is_err());
        assert!(parse_picker_output(b"OTHER:/tmp\n".to_vec()).is_err());
        assert!(parse_picker_output(b"CANCEL\n\n".to_vec()).is_err());
    }

    #[test]
    fn folder_block_messages_are_actionable() {
        for block in [
            FolderBlock::NotAbsolute,
            FolderBlock::DotComponent,
            FolderBlock::Missing,
            FolderBlock::NotDirectory,
            FolderBlock::Symlink,
            FolderBlock::SymlinkAncestor,
            FolderBlock::Network,
            FolderBlock::WholeVolume,
            FolderBlock::Unavailable,
        ] {
            assert!(!block.message().is_empty());
        }
    }
}
