//! Opt-in, bounded, read-only duplicate and large-old user-file review.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const DUPLICATE_FLOOR: u64 = 10 * 1024 * 1024;
const LARGE_FLOOR: i64 = 1_000_000_000;
const OLD_DAYS: i64 = 180;
const MAX_VISITED: usize = 500_000;
const MAX_CANDIDATES: usize = 20_000;
const MAX_DUPLICATE_GROUPS: usize = 50;
const MAX_LARGE_OLD: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub paths: Vec<PathBuf>,
    pub bytes_each: i64,
    pub wasted_bytes: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LargeOldFile {
    pub path: PathBuf,
    pub bytes: i64,
    pub accessed_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewResult {
    pub files_visited: usize,
    pub duplicate_groups: Vec<DuplicateGroup>,
    pub large_old: Vec<LargeOldFile>,
}

#[derive(Clone, Copy)]
struct Config {
    duplicate_floor: u64,
    large_floor: i64,
    old_seconds: i64,
    max_visited: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            duplicate_floor: DUPLICATE_FLOOR,
            large_floor: LARGE_FLOOR,
            old_seconds: OLD_DAYS * 24 * 60 * 60,
            max_visited: MAX_VISITED,
        }
    }
}

#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    logical: u64,
    on_disk: i64,
    accessed_at: i64,
}

fn skipped_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    if name.starts_with('.') || name == "node_modules" {
        return true;
    }
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "app"
                | "photoslibrary"
                | "photolibrary"
                | "musiclibrary"
                | "imovielibrary"
                | "fcpbundle"
                | "logicx"
                | "band"
        )
    )
}

fn qualifies_large_old(on_disk: i64, accessed_at: i64, cutoff: i64, floor: i64) -> bool {
    on_disk >= floor && accessed_at > 0 && accessed_at <= cutoff
}

fn fingerprint(path: &Path) -> Result<u64, String> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hash = 0xcbf29ce484222325u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if read == 0 {
            return Ok(hash);
        }
        for byte in &buffer[..read] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let mut left = BufReader::new(File::open(left).map_err(|error| format!("open file: {error}"))?);
    let mut right =
        BufReader::new(File::open(right).map_err(|error| format!("open file: {error}"))?);
    let mut a = [0u8; 64 * 1024];
    let mut b = [0u8; 64 * 1024];
    loop {
        let ar = left
            .read(&mut a)
            .map_err(|error| format!("compare file: {error}"))?;
        let br = right
            .read(&mut b)
            .map_err(|error| format!("compare file: {error}"))?;
        if ar != br || a[..ar] != b[..br] {
            return Ok(false);
        }
        if ar == 0 {
            return Ok(true);
        }
    }
}

fn scan_roots(
    roots: &[PathBuf],
    config: Config,
    cancel: &AtomicBool,
) -> Result<ReviewResult, String> {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let cutoff = now.saturating_sub(config.old_seconds);
    let mut stack: Vec<(PathBuf, u64)> = roots
        .iter()
        .filter_map(|root| {
            let metadata = std::fs::symlink_metadata(root).ok()?;
            metadata.is_dir().then(|| (root.clone(), metadata.dev()))
        })
        .collect();
    let mut candidates = Vec::new();
    let mut large_old = Vec::new();
    let mut visited = 0usize;
    let mut seen_files = HashSet::new();
    while let Some((dir, root_dev)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err("file review cancelled".into());
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if visited >= config.max_visited {
                break;
            }
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.dev() != root_dev || metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if !skipped_dir(&path) {
                    stack.push((path, root_dev));
                }
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            if !seen_files.insert((metadata.dev(), metadata.ino())) {
                continue;
            }
            visited += 1;
            let on_disk = (metadata.blocks() as i64).saturating_mul(512);
            let item = Candidate {
                path,
                logical: metadata.len(),
                on_disk,
                accessed_at: metadata.atime(),
            };
            if qualifies_large_old(item.on_disk, item.accessed_at, cutoff, config.large_floor) {
                large_old.push(LargeOldFile {
                    path: item.path.clone(),
                    bytes: item.on_disk,
                    accessed_at: item.accessed_at,
                });
            }
            if item.logical >= config.duplicate_floor && candidates.len() < MAX_CANDIDATES {
                candidates.push(item);
            }
        }
        if visited >= config.max_visited {
            break;
        }
    }

    let mut by_size: BTreeMap<u64, Vec<Candidate>> = BTreeMap::new();
    for candidate in candidates {
        by_size
            .entry(candidate.logical)
            .or_default()
            .push(candidate);
    }
    let mut duplicate_groups = Vec::new();
    for same_size in by_size.into_values().filter(|items| items.len() > 1) {
        if cancel.load(Ordering::Relaxed) {
            return Err("file review cancelled".into());
        }
        let mut by_fingerprint: HashMap<u64, Vec<Candidate>> = HashMap::new();
        for item in same_size {
            if let Ok(hash) = fingerprint(&item.path) {
                by_fingerprint.entry(hash).or_default().push(item);
            }
        }
        for same_hash in by_fingerprint.into_values().filter(|items| items.len() > 1) {
            let mut exact: Vec<Vec<Candidate>> = Vec::new();
            for item in same_hash {
                let mut placed = false;
                for group in &mut exact {
                    if files_equal(&group[0].path, &item.path).unwrap_or(false) {
                        group.push(item.clone());
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    exact.push(vec![item]);
                }
            }
            for mut group in exact.into_iter().filter(|group| group.len() > 1) {
                group.sort_by(|left, right| left.path.cmp(&right.path));
                let bytes_each = group[0].on_disk;
                duplicate_groups.push(DuplicateGroup {
                    wasted_bytes: bytes_each.saturating_mul(group.len().saturating_sub(1) as i64),
                    bytes_each,
                    paths: group.into_iter().map(|item| item.path).collect(),
                });
            }
        }
    }
    duplicate_groups.sort_by(|left, right| right.wasted_bytes.cmp(&left.wasted_bytes));
    duplicate_groups.truncate(MAX_DUPLICATE_GROUPS);
    large_old.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.path.cmp(&right.path))
    });
    large_old.truncate(MAX_LARGE_OLD);
    Ok(ReviewResult {
        files_visited: visited,
        duplicate_groups,
        large_old,
    })
}

pub fn standard_roots(home: &Path) -> Vec<PathBuf> {
    [
        "Desktop",
        "Documents",
        "Downloads",
        "Movies",
        "Music",
        "Pictures",
    ]
    .into_iter()
    .map(|name| home.join(name))
    .filter(|path| path.is_dir())
    .collect()
}

pub fn run(
    roots: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<Result<ReviewResult, String>>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("file-review".into())
        .spawn(move || {
            let _ = tx.send(scan_roots(&roots, Config::default(), &cancel));
        })
        .map(|_| ())
        .map_err(|error| format!("start file review: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    #[test]
    fn proves_exact_duplicates_and_separates_same_size_content() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        let c = tmp.path().join("c.bin");
        std::fs::write(&a, b"same payload").unwrap();
        std::fs::write(&b, b"same payload").unwrap();
        std::fs::write(&c, b"other bytes!").unwrap();
        let result = scan_roots(
            &[tmp.path().to_path_buf()],
            Config {
                duplicate_floor: 1,
                large_floor: i64::MAX,
                old_seconds: 1,
                max_visited: 100,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.duplicate_groups.len(), 1);
        assert_eq!(result.duplicate_groups[0].paths, vec![a, b]);
    }

    #[test]
    fn skips_hidden_node_modules_and_bundle_directories() {
        let tmp = tempfile::tempdir().unwrap();
        for dir in [".hidden", "node_modules", "Photos.photoslibrary"] {
            let path = tmp.path().join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("copy"), b"same").unwrap();
        }
        let result = scan_roots(
            &[tmp.path().to_path_buf()],
            Config {
                duplicate_floor: 1,
                large_floor: i64::MAX,
                old_seconds: 1,
                max_visited: 100,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.files_visited, 0);
    }

    #[test]
    fn cancellation_stops_before_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(true);
        let result = scan_roots(&[tmp.path().to_path_buf()], Config::default(), &cancel);
        assert_eq!(result.unwrap_err(), "file review cancelled");
    }

    #[test]
    fn sparse_large_file_uses_on_disk_bytes_for_large_old() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sparse");
        let mut file = File::create(&path).unwrap();
        file.seek(SeekFrom::Start(2_000_000_000)).unwrap();
        file.write_all(b"x").unwrap();
        let metadata = std::fs::metadata(path).unwrap();
        assert!((metadata.blocks() as i64 * 512) < LARGE_FLOOR);
        assert!(qualifies_large_old(LARGE_FLOOR, 100, 200, LARGE_FLOOR));
        assert!(!qualifies_large_old(LARGE_FLOOR - 1, 100, 200, LARGE_FLOOR));
        assert!(!qualifies_large_old(LARGE_FLOOR, 201, 200, LARGE_FLOOR));
    }

    #[test]
    fn hardlinks_are_one_physical_file_not_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::write(&a, b"same").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        let result = scan_roots(
            &[tmp.path().to_path_buf()],
            Config {
                duplicate_floor: 1,
                large_floor: i64::MAX,
                old_seconds: 1,
                max_visited: 100,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.files_visited, 1);
        assert!(result.duplicate_groups.is_empty());
    }
}
