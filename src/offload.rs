//! Offload: relocate an item to an attached external volume via a verified
//! copy-then-remove move. Copy → verify → delete original → optional symlink
//! → ledger. The original is removed only after a verified copy.

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::Sender;

use crate::clean::{delete_path, quick_du};

/// An attached external volume eligible as an offload target.
#[derive(Clone)]
pub struct Volume {
    pub name: String,
    pub mount_path: PathBuf,
    pub fs_type: String,
    pub free_bytes: i64,
}

/// Where an item lands on a target volume: its absolute path mirrored beneath
/// `<mount>/DiskDeck Offload/`. `/Users/<user>/Movies/Big.mov` on
/// `/Volumes/<external>` becomes
/// `/Volumes/<external>/DiskDeck Offload/Users/<user>/Movies/Big.mov`.
pub fn dest_for(src: &Path, mount_path: &Path) -> PathBuf {
    let rel = src.strip_prefix("/").unwrap_or(src);
    mount_path.join("DiskDeck Offload").join(rel)
}

/// Free-space margin required on the target beyond the item's own size.
pub const MARGIN_BYTES: i64 = 100 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffloadBlock {
    NotAbsolute,
    NotNormalized,
    AlreadyExternal,
    OutsideHome,
    HomeRoot,
    HiddenRoot,
    ProtectedRoot,
    CloudSyncRoot,
    ManagedBundle,
    Missing,
    Symlink,
    SymlinkAncestor,
}

impl OffloadBlock {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAbsolute => "Only absolute paths can be offloaded.",
            Self::NotNormalized => "This path must be normalized before it can be offloaded.",
            Self::AlreadyExternal => "This item is already on an external volume.",
            Self::OutsideHome => "Only items inside your home folder can be offloaded.",
            Self::HomeRoot => "Your entire home folder cannot be offloaded.",
            Self::HiddenRoot => "Hidden home-folder data stays on this Mac.",
            Self::ProtectedRoot => "App-managed home data stays on this Mac.",
            Self::CloudSyncRoot => "Cloud-synced folders cannot be offloaded safely.",
            Self::ManagedBundle => {
                "Application and managed-library bundles cannot be offloaded safely."
            }
            Self::Missing => "This item is no longer available.",
            Self::Symlink => "Symlinks cannot be offloaded.",
            Self::SymlinkAncestor => "Items reached through a symlink cannot be offloaded.",
        }
    }
}

fn has_dot_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

fn is_protected_root(name: &str) -> bool {
    ["library", "applications", "public", "trash"]
        .iter()
        .any(|protected| name.eq_ignore_ascii_case(protected))
}

fn is_cloud_root(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "dropbox",
        "onedrive",
        "google drive",
        "icloud drive (archive",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn is_managed_bundle(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".app",
        ".photoslibrary",
        ".musiclibrary",
        ".imovielibrary",
        ".fcpbundle",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

/// Pure, filesystem-independent policy for whether a path may be offloaded.
pub fn classify_movable(src: &Path, home: &Path) -> Result<(), OffloadBlock> {
    if !src.is_absolute() || !home.is_absolute() {
        return Err(OffloadBlock::NotAbsolute);
    }
    if has_dot_component(src) || has_dot_component(home) {
        return Err(OffloadBlock::NotNormalized);
    }
    if src.starts_with("/Volumes") {
        return Err(OffloadBlock::AlreadyExternal);
    }
    if !src.starts_with(home) {
        return Err(OffloadBlock::OutsideHome);
    }

    let relative = src
        .strip_prefix(home)
        .map_err(|_| OffloadBlock::OutsideHome)?;
    if relative.as_os_str().is_empty() {
        return Err(OffloadBlock::HomeRoot);
    }

    let first = relative
        .components()
        .find_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .ok_or(OffloadBlock::HomeRoot)?;
    if first.starts_with('.') {
        return Err(OffloadBlock::HiddenRoot);
    }
    if is_protected_root(&first) {
        return Err(OffloadBlock::ProtectedRoot);
    }
    if is_cloud_root(&first) {
        return Err(OffloadBlock::CloudSyncRoot);
    }

    if relative.components().any(|component| match component {
        Component::Normal(name) => is_managed_bundle(&name.to_string_lossy()),
        _ => false,
    }) {
        return Err(OffloadBlock::ManagedBundle);
    }
    Ok(())
}

/// Full movable check. Filesystem-specific protection is added below the
/// lexical policy so UI code can use `classify_movable` without per-frame I/O.
pub fn check_movable(src: &Path, home: &Path) -> Result<(), OffloadBlock> {
    classify_movable(src, home)?;
    let relative = src
        .strip_prefix(home)
        .map_err(|_| OffloadBlock::OutsideHome)?;
    let mut current = home.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(OffloadBlock::NotNormalized);
        };
        current.push(name);
        let metadata = std::fs::symlink_metadata(&current).map_err(|_| OffloadBlock::Missing)?;
        if metadata.file_type().is_symlink() {
            return Err(if current == src {
                OffloadBlock::Symlink
            } else {
                OffloadBlock::SymlinkAncestor
            });
        }
    }
    Ok(())
}

/// Does the target have room for the item plus the safety margin?
pub fn has_room(item_size: i64, free_bytes: i64) -> bool {
    free_bytes >= item_size + MARGIN_BYTES
}

/// The destructive half of an offload stays locked until all safety gates pass.
pub fn can_confirm_offload(acknowledged: bool, room: bool, movable: bool) -> bool {
    acknowledged && room && movable
}

/// Filter rule for a `/Volumes/*` entry. External drives mount as real,
/// writable directories; the boot volume appears (if at all) as a read-only
/// system volume or a symlink alias, and network shares report autofs/nfs/smbfs.
fn eligible_volume(fs_type: &str, read_only: bool, is_symlink: bool) -> bool {
    !read_only && !is_symlink && !matches!(fs_type, "autofs" | "nfs" | "smbfs" | "afpfs" | "webdav")
}

/// Enumerate attached external volumes eligible as offload targets. Empty vec
/// means "no SSD present" and the Offload action stays hidden.
pub fn external_volumes() -> Vec<Volume> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/Volumes") else {
        return out;
    };
    for entry in entries.flatten() {
        let mount = entry.path();
        let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(true); // treat unreadable as symlink → skip
        let Some((fs_type, read_only, free)) = statfs_info(&mount) else {
            continue;
        };
        if !eligible_volume(&fs_type, read_only, is_symlink) {
            continue;
        }
        out.push(Volume {
            name: mount
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            mount_path: mount,
            fs_type,
            free_bytes: free,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// (fs_type, read_only, free_bytes) for a mount point, or None if statfs fails.
fn statfs_info(mount: &Path) -> Option<(String, bool, i64)> {
    let cpath = CString::new(mount.as_os_str().as_bytes()).ok()?;
    // SAFETY: zeroed statfs is valid; we pass a valid C string and check the rc.
    let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut sfs) } != 0 {
        return None;
    }
    let fs_type = unsafe { CStr::from_ptr(sfs.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let read_only = sfs.f_flags & (libc::MNT_RDONLY as u32) != 0;
    let free = sfs.f_bavail as i64 * sfs.f_bsize as i64;
    Some((fs_type, read_only, free))
}

/// Logical (apparent) size of a path: the sum of regular files' `len()`,
/// filesystem-independent unlike block-based `quick_du`. A single file
/// returns its own length; a directory sums its regular-file descendants.
/// Symlinks are not followed — their entries contribute 0.
fn apparent_size(p: &Path) -> i64 {
    let mut total = 0i64;
    if let Ok(meta) = p.symlink_metadata() {
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(p) {
                for e in entries.flatten() {
                    total += apparent_size(&e.path());
                }
            }
        } else if meta.is_file() {
            total += meta.len() as i64;
        }
        // symlinks (and other non-regular entries) contribute 0
    }
    total
}

pub struct MoveOutcome {
    pub dest: PathBuf,
    pub reclaimed: i64,
    pub symlinked: bool,
}

/// Verified cross-volume move. Order is the safety contract:
/// copy (ditto) → verify size → delete original → optional symlink → ledger.
/// Any failure before the delete leaves `src` untouched.
pub fn perform_move(
    src: &Path,
    dest: &Path,
    ledger_path: &Path,
    leave_symlink: bool,
) -> Result<MoveOutcome, String> {
    let total = quick_du(src);
    let src_apparent = apparent_size(src);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("prepare destination: {e}"))?;
    }

    // 1. copy
    let out = std::process::Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(dest)
        .output()
        .map_err(|e| format!("ditto: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "copy failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // 2. verify — by apparent (logical) size, not disk blocks. ditto drops
    // xattrs/resource forks when copying onto exFAT, which legitimately
    // shrinks block-based usage even on a byte-perfect copy; apparent size
    // is filesystem-independent.
    let dest_apparent = apparent_size(dest);
    if dest_apparent < src_apparent {
        return Err(format!(
            "verify failed: copied {dest_apparent} < source {src_apparent} bytes; original left intact"
        ));
    }

    // 3. delete original (only now)
    delete_path(src)?;

    // 4. optional symlink at the origin, pointing at the new location
    let mut symlinked = false;
    if leave_symlink {
        symlinked = std::os::unix::fs::symlink(dest, src).is_ok();
    }

    // 5. ledger
    append_ledger(ledger_path, src, dest, symlinked);

    Ok(MoveOutcome {
        dest: dest.to_path_buf(),
        reclaimed: total,
        symlinked,
    })
}

/// Append one JSON line recording the move. Hand-rolled (no serde dependency).
fn append_ledger(ledger_path: &Path, origin: &Path, dest: &Path, symlinked: bool) {
    use std::io::Write;
    let ts = unsafe { libc::time(std::ptr::null_mut()) };
    let line = format!(
        "{{\"origin\":{},\"dest\":{},\"moved_at\":{},\"symlinked\":{}}}\n",
        json_str(&origin.to_string_lossy()),
        json_str(&dest.to_string_lossy()),
        ts,
        symlinked
    );
    if let Some(parent) = ledger_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Minimal JSON string escaping for the two path fields.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) <= 0x1F => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub struct OffloadJob {
    pub src: PathBuf,
    pub mount_path: PathBuf,
    pub leave_symlink: bool,
}

pub enum OffloadEvent {
    Started {
        name: String,
        total: i64,
    },
    Done {
        reclaimed: i64,
        dest: PathBuf,
        symlinked: bool,
    },
    Failed {
        error: String,
    },
}

/// Run one offload on a background thread, streaming events to the UI.
pub fn run_offload(job: OffloadJob, tx: Sender<OffloadEvent>) {
    std::thread::Builder::new()
        .name("offload".into())
        .spawn(move || {
            let name = job
                .src
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let _ = tx.send(OffloadEvent::Started {
                name,
                total: quick_du(&job.src),
            });
            let dest = dest_for(&job.src, &job.mount_path);
            let ledger = job
                .mount_path
                .join("DiskDeck Offload")
                .join(".diskdeck-offload.json");
            match perform_move(&job.src, &dest, &ledger, job.leave_symlink) {
                Ok(o) => {
                    let _ = tx.send(OffloadEvent::Done {
                        reclaimed: o.reclaimed,
                        dest: o.dest,
                        symlinked: o.symlinked,
                    });
                }
                Err(e) => {
                    let _ = tx.send(OffloadEvent::Failed { error: e });
                }
            }
        })
        .expect("spawn offload thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn dest_mirrors_absolute_path_under_offload_root() {
        let d = dest_for(
            Path::new("/Users/<user>/Movies/Big.mov"),
            Path::new("/Volumes/<external>"),
        );
        assert_eq!(
            d,
            PathBuf::from("/Volumes/<external>/DiskDeck Offload/Users/<user>/Movies/Big.mov")
        );
    }

    #[test]
    fn movable_policy_allows_normal_and_custom_home_folders() {
        let home = Path::new("/Users/<user>");
        assert_eq!(
            classify_movable(Path::new("/Users/<user>/Movies/Big.mov"), home),
            Ok(())
        );
        assert_eq!(
            classify_movable(Path::new("/Users/<user>/Projects/DiskDeck"), home),
            Ok(())
        );
    }

    #[test]
    fn movable_policy_blocks_protected_home_roots() {
        let home = Path::new("/Users/<user>");
        for path in [
            "/Users/<user>",
            "/Users/<user>/Library/Caches/App",
            "/Users/<user>/.ssh",
            "/Users/<user>/Applications/Tool.app",
            "/Users/<user>/Public",
            "/Users/<user>/.Trash/file",
        ] {
            assert!(classify_movable(Path::new(path), home).is_err(), "{path}");
        }
    }

    #[test]
    fn movable_policy_blocks_cloud_roots_and_managed_bundles() {
        let home = Path::new("/Users/<user>");
        for path in [
            "/Users/<user>/Dropbox/archive",
            "/Users/<user>/OneDrive - Example/archive",
            "/Users/<user>/Google Drive/archive",
            "/Users/<user>/Pictures/Library.photoslibrary",
            "/Users/<user>/Movies/Edit.fcpbundle",
        ] {
            assert!(classify_movable(Path::new(path), home).is_err(), "{path}");
        }
    }

    #[test]
    fn movable_policy_blocks_non_normalized_external_and_outside_paths() {
        let home = Path::new("/Users/<user>");
        assert_eq!(
            classify_movable(Path::new("relative/file"), home),
            Err(OffloadBlock::NotAbsolute)
        );
        assert_eq!(
            classify_movable(Path::new("/Users/<user>/Movies/../Library"), home),
            Err(OffloadBlock::NotNormalized)
        );
        assert_eq!(
            classify_movable(Path::new("/Volumes/<external>/file"), home),
            Err(OffloadBlock::AlreadyExternal)
        );
        assert_eq!(
            classify_movable(Path::new("/System/Library/file"), home),
            Err(OffloadBlock::OutsideHome)
        );
    }

    #[test]
    fn movable_filesystem_accepts_a_regular_home_file_and_rejects_a_source_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let regular = home.join("Movies/clip.mov");
        let link = home.join("Movies/link.mov");
        fs::create_dir_all(regular.parent().unwrap()).unwrap();
        fs::write(&regular, b"clip").unwrap();
        std::os::unix::fs::symlink(&regular, &link).unwrap();
        assert_eq!(check_movable(&regular, &home), Ok(()));
        assert_eq!(check_movable(&link, &home), Err(OffloadBlock::Symlink));
    }

    #[test]
    fn movable_filesystem_rejects_a_symlinked_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let real = tmp.path().join("real");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("clip.mov"), b"clip").unwrap();
        std::os::unix::fs::symlink(&real, home.join("Movies")).unwrap();
        assert_eq!(
            check_movable(&home.join("Movies/clip.mov"), &home),
            Err(OffloadBlock::SymlinkAncestor)
        );
    }

    #[test]
    fn room_requires_size_plus_margin() {
        assert!(!has_room(1_000, 1_000 + MARGIN_BYTES - 1));
        assert!(has_room(1_000, 1_000 + MARGIN_BYTES));
    }

    #[test]
    fn confirmation_requires_acknowledgement_room_and_a_movable_source() {
        assert!(can_confirm_offload(true, true, true));
        assert!(!can_confirm_offload(false, true, true));
        assert!(!can_confirm_offload(true, false, true));
        assert!(!can_confirm_offload(true, true, false));
    }

    #[test]
    fn eligible_accepts_writable_local_disk() {
        assert!(eligible_volume("exfat", false, false));
        assert!(eligible_volume("apfs", false, false));
    }

    #[test]
    fn eligible_rejects_readonly_symlink_and_network() {
        assert!(!eligible_volume("apfs", true, false)); // read-only system vol
        assert!(!eligible_volume("apfs", false, true)); // boot alias symlink
        assert!(!eligible_volume("smbfs", false, false)); // network share
    }

    #[test]
    fn external_volumes_does_not_panic() {
        // Hardware-independent: assert real invariants on whatever comes back.
        // An empty result trivially passes on machines with no external drive.
        let vols = external_volumes();
        for v in &vols {
            assert!(
                v.mount_path.starts_with("/Volumes"),
                "mount_path must live under /Volumes, got {:?}",
                v.mount_path
            );
            assert!(!v.name.is_empty(), "volume name must be non-empty");
        }
    }

    #[test]
    fn apparent_size_sums_regular_files() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("dir/a.bin");
        let b = tmp.path().join("dir/sub/b.bin");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        fs::write(&a, vec![0u8; 1000]).unwrap();
        fs::write(&b, vec![0u8; 2500]).unwrap();

        assert_eq!(apparent_size(&tmp.path().join("dir")), 1000 + 2500);

        let single = tmp.path().join("solo.bin");
        fs::write(&single, vec![0u8; 777]).unwrap();
        assert_eq!(apparent_size(&single), 777);
    }

    #[test]
    fn clean_move_relocates_and_removes_original() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src/data.txt");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"hello offload").unwrap();
        let dest = tmp.path().join("vol/DiskDeck Offload/data.txt");
        let ledger = tmp
            .path()
            .join("vol/DiskDeck Offload/.diskdeck-offload.json");

        let out = perform_move(&src, &dest, &ledger, false).unwrap();

        assert!(!src.exists(), "original removed after verified copy");
        assert_eq!(fs::read(&dest).unwrap(), b"hello offload");
        assert!(!out.symlinked);
        assert!(out.reclaimed > 0);
        let log = fs::read_to_string(&ledger).unwrap();
        assert!(log.contains("data.txt"), "ledger records the move");
    }

    #[test]
    fn symlink_move_leaves_a_link_at_the_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src/data.txt");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"payload").unwrap();
        let dest = tmp.path().join("vol/DiskDeck Offload/data.txt");
        let ledger = tmp
            .path()
            .join("vol/DiskDeck Offload/.diskdeck-offload.json");

        let out = perform_move(&src, &dest, &ledger, true).unwrap();

        assert!(out.symlinked);
        let meta = fs::symlink_metadata(&src).unwrap();
        assert!(meta.file_type().is_symlink(), "origin is now a symlink");
        assert_eq!(
            fs::read(&src).unwrap(),
            b"payload",
            "symlink resolves to dest"
        );
    }

    #[test]
    fn run_offload_emits_started_then_done() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Users/me/big.bin");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, vec![7u8; 4096]).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        run_offload(
            OffloadJob {
                src: src.clone(),
                mount_path: tmp.path().join("vol"),
                leave_symlink: false,
            },
            tx,
        );

        let mut saw_started = false;
        let mut saw_done = false;
        while let Ok(ev) = rx.recv() {
            match ev {
                OffloadEvent::Started { .. } => saw_started = true,
                OffloadEvent::Done { .. } => {
                    saw_done = true;
                    break;
                }
                OffloadEvent::Failed { error } => panic!("unexpected failure: {error}"),
            }
        }
        assert!(saw_started && saw_done);
        assert!(!src.exists());
    }
}
