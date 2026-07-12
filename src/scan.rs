//! Concurrent disk scanner building a live, lock-light tree.
//!
//! Sizes are bubbled up the ancestor chain through atomics the moment a file
//! is statted, so the UI can render the tree *while it grows* — the feature
//! the webview edition couldn't do. Structure changes (new child dirs) take a
//! short per-node mutex; size reads are entirely lock-free.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

/// The APFS data volume — everything user-writable. Scanning it directly
/// (instead of "/") avoids firmlink double-counting. Do not "fix" this.
pub const DATA_ROOT: &str = "/System/Volumes/Data";

/// Post-scan compaction thresholds (the live tree keeps everything; compaction
/// folds small directories into parent aggregates once the scan completes).
pub const KEEP_DIR_BYTES: i64 = 10 << 20;
/// Files below this size are never materialized as nodes, only aggregated.
pub const KEEP_FILE_BYTES: i64 = 100 << 20;

pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub bytes: AtomicI64,
    pub files: AtomicI64,
    pub small_bytes: AtomicI64,
    pub small_count: AtomicI64,
    pub denied: AtomicBool,
    pub parent: Weak<Node>,
    pub children: Mutex<Vec<Arc<Node>>>,
}

impl Node {
    fn new(name: String, path: PathBuf, is_dir: bool, parent: &Arc<Node>) -> Arc<Node> {
        Arc::new(Node {
            name,
            path,
            is_dir,
            bytes: AtomicI64::new(0),
            files: AtomicI64::new(0),
            small_bytes: AtomicI64::new(0),
            small_count: AtomicI64::new(0),
            denied: AtomicBool::new(false),
            parent: Arc::downgrade(parent),
            children: Mutex::new(Vec::new()),
        })
    }

    fn new_root(path: PathBuf) -> Arc<Node> {
        Arc::new(Node {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into()),
            path,
            is_dir: true,
            bytes: AtomicI64::new(0),
            files: AtomicI64::new(0),
            small_bytes: AtomicI64::new(0),
            small_count: AtomicI64::new(0),
            denied: AtomicBool::new(false),
            parent: Weak::new(),
            children: Mutex::new(Vec::new()),
        })
    }

    pub fn bytes(&self) -> i64 {
        self.bytes.load(Relaxed)
    }
    pub fn files(&self) -> i64 {
        self.files.load(Relaxed)
    }
    pub fn kids(&self) -> Vec<Arc<Node>> {
        self.children.lock().unwrap().clone()
    }
}

/// Add bytes/files to a node and every ancestor — the live-map heartbeat.
fn bubble(start: &Arc<Node>, bytes: i64, files: i64) {
    let mut cur = Some(start.clone());
    while let Some(n) = cur {
        n.bytes.fetch_add(bytes, Relaxed);
        if files != 0 {
            n.files.fetch_add(files, Relaxed);
        }
        cur = n.parent.upgrade();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ScanState {
    Idle = 0,
    Running = 1,
    Done = 2,
    Aborted = 3,
}

pub struct ScanHandle {
    pub root: Arc<Node>,
    pub denied: Arc<AtomicI64>,
    pub current: Arc<Mutex<String>>,
    pub cancel: Arc<AtomicBool>,
    state: Arc<AtomicU8>,
    pub started: Instant,
    pub duration_ms: Arc<AtomicI64>,
}

impl ScanHandle {
    pub fn state(&self) -> ScanState {
        match self.state.load(Relaxed) {
            1 => ScanState::Running,
            2 => ScanState::Done,
            3 => ScanState::Aborted,
            _ => ScanState::Idle,
        }
    }
}

struct Ctx {
    dev: u64,
    denied: Arc<AtomicI64>,
    current: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
    // sharded inode sets so hardlinked content is counted once
    hardlinks: [Mutex<HashSet<u64>>; 16],
}

impl Ctx {
    fn seen_inode(&self, ino: u64) -> bool {
        !self.hardlinks[(ino & 0xF) as usize]
            .lock()
            .unwrap()
            .insert(ino)
    }
}

/// Kick off a scan on background threads. Returns immediately; the UI reads
/// the growing tree through the handle.
pub fn start_scan(root_path: PathBuf) -> ScanHandle {
    let root = Node::new_root(root_path.clone());
    let handle = ScanHandle {
        root: root.clone(),
        denied: Arc::new(AtomicI64::new(0)),
        current: Arc::new(Mutex::new(String::new())),
        cancel: Arc::new(AtomicBool::new(false)),
        state: Arc::new(AtomicU8::new(ScanState::Running as u8)),
        started: Instant::now(),
        duration_ms: Arc::new(AtomicI64::new(0)),
    };

    let ctx = Arc::new(Ctx {
        dev: fs::symlink_metadata(&root_path)
            .map(|m| m.dev())
            .unwrap_or(0),
        denied: handle.denied.clone(),
        current: handle.current.clone(),
        cancel: handle.cancel.clone(),
        hardlinks: Default::default(),
    });
    let state = handle.state.clone();
    let duration = handle.duration_ms.clone();
    let started = handle.started;

    std::thread::Builder::new()
        .name("scan-driver".into())
        .spawn(move || {
            rayon::scope(|s| scan_dir(s, root.clone(), ctx.clone()));
            compact(&root);
            duration.store(started.elapsed().as_millis() as i64, Relaxed);
            let end = if ctx.cancel.load(Relaxed) {
                ScanState::Aborted
            } else {
                ScanState::Done
            };
            state.store(end as u8, Relaxed);
        })
        .expect("spawn scan thread");

    handle
}

fn scan_dir<'s>(scope: &rayon::Scope<'s>, node: Arc<Node>, ctx: Arc<Ctx>) {
    if ctx.cancel.load(Relaxed) {
        return;
    }
    let entries = match fs::read_dir(&node.path) {
        Ok(e) => e,
        Err(_) => {
            node.denied.store(true, Relaxed);
            ctx.denied.fetch_add(1, Relaxed);
            return;
        }
    };
    if let Ok(mut cur) = ctx.current.lock() {
        cur.clear();
        cur.push_str(&node.path.to_string_lossy());
    }
    // the directory entry itself occupies blocks too
    if let Ok(m) = fs::symlink_metadata(&node.path) {
        bubble(&node, (m.blocks() as i64) * 512, 0);
    }

    for entry in entries.flatten() {
        if ctx.cancel.load(Relaxed) {
            return;
        }
        // DirEntry::metadata does not traverse symlinks
        let Ok(m) = entry.metadata() else { continue };
        if m.dev() != ctx.dev {
            continue; // never cross into other volumes / network mounts
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if m.is_dir() {
            let child = Node::new(name, entry.path(), true, &node);
            node.children.lock().unwrap().push(child.clone());
            let ctx = ctx.clone();
            scope.spawn(move |s| scan_dir(s, child, ctx));
            continue;
        }
        let sz = (m.blocks() as i64) * 512; // on-disk usage, sparse-aware
        if m.nlink() > 1 && ctx.seen_inode(m.ino()) {
            continue; // hardlinked content counted once
        }
        bubble(&node, sz, 1);
        if sz >= KEEP_FILE_BYTES {
            let leaf = Node::new(name, entry.path(), false, &node);
            leaf.bytes.store(sz, Relaxed);
            leaf.files.store(1, Relaxed);
            node.children.lock().unwrap().push(leaf);
        } else {
            node.small_bytes.fetch_add(sz, Relaxed);
            node.small_count.fetch_add(1, Relaxed);
        }
    }
}

/// Fold directories below KEEP_DIR_BYTES into their parent's aggregate and
/// sort children by size. Run once after the scan completes.
pub fn compact(node: &Arc<Node>) {
    let mut kids = node.children.lock().unwrap();
    let mut kept = Vec::with_capacity(kids.len());
    for c in kids.drain(..) {
        if c.is_dir && c.bytes() < KEEP_DIR_BYTES && !c.denied.load(Relaxed) {
            node.small_bytes.fetch_add(c.bytes(), Relaxed);
            node.small_count.fetch_add(c.files(), Relaxed);
        } else {
            kept.push(c);
        }
    }
    kept.sort_by_key(|c| -c.bytes());
    *kids = kept;
    let snapshot = kids.clone();
    drop(kids);
    for c in &snapshot {
        if c.is_dir {
            compact(c);
        }
    }
}

/// Find a node by absolute path, walking one segment at a time.
pub fn lookup(root: &Arc<Node>, path: &Path) -> Option<Arc<Node>> {
    if path == root.path {
        return Some(root.clone());
    }
    let rel = path.strip_prefix(&root.path).ok()?;
    let mut cur = root.clone();
    for seg in rel.components() {
        let seg = seg.as_os_str().to_string_lossy();
        let next = cur
            .children
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.name == seg)
            .cloned()?;
        cur = next;
    }
    Some(cur)
}

pub fn size_of(root: &Arc<Node>, path: &Path) -> i64 {
    lookup(root, path).map(|n| n.bytes()).unwrap_or(0)
}

/// Collect kept directories with the given name, skipping nested matches
/// (a node_modules inside another node_modules is not reported twice).
pub fn find_dirs_named(root: &Arc<Node>, name: &str) -> Vec<Arc<Node>> {
    let mut out = Vec::new();
    fn rec(n: &Arc<Node>, name: &str, out: &mut Vec<Arc<Node>>) {
        for c in n.kids() {
            if c.is_dir && c.name == name {
                out.push(c);
            } else if c.is_dir {
                rec(&c, name, out);
            }
        }
    }
    rec(root, name, &mut out);
    out.sort_by_key(|n| -n.bytes());
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DiskStats {
    pub total: i64,
    pub free: i64,
    pub used: i64,
    pub used_pct: f64,
}

pub fn disk_stats() -> DiskStats {
    disk_stats_for(Path::new(DATA_ROOT))
}

pub fn disk_stats_for(path: &Path) -> DiskStats {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return DiskStats::default();
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut st) } != 0 {
        return DiskStats {
            total: 0,
            free: 0,
            used: 0,
            used_pct: 0.0,
        };
    }
    let bsize = st.f_bsize as i64;
    let total = st.f_blocks as i64 * bsize;
    let free = st.f_bavail as i64 * bsize;
    let used = total - free;
    let used_pct = if total > 0 {
        used as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    DiskStats {
        total,
        free,
        used,
        used_pct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(path: &Path, size: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::File::create(path).unwrap();
        f.write_all(&vec![0u8; size]).unwrap();
    }

    fn scan_blocking(root: &Path) -> ScanHandle {
        let h = start_scan(root.to_path_buf());
        while h.state() == ScanState::Running {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        h
    }

    #[test]
    fn counts_and_aggregates() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(&tmp.path().join("a/big.bin"), 20480);
        write_file(&tmp.path().join("a/small.bin"), 1024);
        write_file(&tmp.path().join("b/x.bin"), 4096);

        let h = scan_blocking(tmp.path());
        assert_eq!(h.root.files(), 3);
        assert!(h.root.bytes() >= 20480 + 1024 + 4096);
        assert_eq!(h.state(), ScanState::Done);
    }

    #[test]
    fn compact_folds_small_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(&tmp.path().join("tiny/x.bin"), 1024);
        let h = scan_blocking(tmp.path());
        // tiny is far below KEEP_DIR_BYTES → folded by post-scan compaction
        assert!(lookup(&h.root, &tmp.path().join("tiny")).is_none());
        assert!(h.root.small_count.load(Relaxed) >= 1);
    }

    #[test]
    fn hardlinks_counted_once() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        write_file(&a, 8192);
        fs::hard_link(&a, tmp.path().join("b.bin")).unwrap();

        let h = scan_blocking(tmp.path());
        assert_eq!(h.root.files(), 1, "hardlinked file must count once");
        assert!(h.root.bytes() <= 8192 + 8192, "no double count");
    }

    #[test]
    fn denied_dirs_counted_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        write_file(&tmp.path().join("locked/secret.bin"), 4096);
        write_file(&tmp.path().join("open/ok.bin"), 4096);
        let locked = tmp.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let h = scan_blocking(tmp.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(h.denied.load(Relaxed), 1);
        assert_eq!(h.root.files(), 1, "scan continues past denied dirs");
    }

    #[test]
    fn nested_dirs_named_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(&tmp.path().join("a/node_modules/p/i.js"), 4096);
        write_file(
            &tmp.path().join("a/node_modules/p/node_modules/q/j.js"),
            4096,
        );
        write_file(&tmp.path().join("b/node_modules/r/k.js"), 4096);

        let h = scan_blocking(tmp.path());
        // compaction folds these (small), so search before relying on kept
        // nodes: rebuild expectation from the uncompacted invariant instead.
        // find_dirs_named operates on kept nodes — with tiny fixtures the
        // dirs were folded, so assert against a fresh uncompacted scan:
        let root = Node::new_root(tmp.path().to_path_buf());
        let ctx = Arc::new(Ctx {
            dev: fs::symlink_metadata(tmp.path()).unwrap().dev(),
            denied: Arc::new(AtomicI64::new(0)),
            current: Arc::new(Mutex::new(String::new())),
            cancel: Arc::new(AtomicBool::new(false)),
            hardlinks: Default::default(),
        });
        rayon::scope(|s| scan_dir(s, root.clone(), ctx));
        let found = find_dirs_named(&root, "node_modules");
        assert_eq!(found.len(), 2, "nested node_modules must not double-report");
        let _ = h;
    }

    #[test]
    fn disk_stats_for_reports_the_selected_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        let stats = disk_stats_for(tmp.path());
        assert!(stats.total > 0);
        assert!(stats.total >= stats.free);
        assert_eq!(stats.used, stats.total - stats.free);
        assert!((0.0..=100.0).contains(&stats.used_pct));
    }
}
