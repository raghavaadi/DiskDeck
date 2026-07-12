//! Local cleanup receipts and verified Trash recovery.
//!
//! Receipt data is evidence only. It never becomes cleanup or command
//! authority, and corrupt history is never overwritten automatically.

use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 5] = b"DDRH1";
const MAX_RECEIPTS: usize = 200;
const MAX_PATH_BYTES: usize = 4096;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
    pub kind: FileKind,
}

impl FileIdentity {
    pub fn at(path: &Path) -> Result<Self, String> {
        let metadata = path
            .symlink_metadata()
            .map_err(|error| format!("read item identity: {error}"))?;
        let kind = if metadata.file_type().is_file() {
            FileKind::File
        } else if metadata.file_type().is_dir() {
            FileKind::Directory
        } else {
            return Err("item is not a regular file or directory".into());
        };
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            kind,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrashEvidence {
    pub path: PathBuf,
    pub identity: FileIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrashOutcome {
    Exact(TrashEvidence),
    FinderManaged,
}

pub fn rename_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiptAction {
    Trash,
    Delete,
    Empty,
    Command,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub event_id: u128,
    pub completed_at_ms: i64,
    pub rec_id: String,
    pub title: String,
    pub origin: PathBuf,
    pub action: ReceiptAction,
    pub freed_bytes: i64,
    pub pending_bytes: i64,
    pub trash: Option<TrashEvidence>,
    pub finder_managed: bool,
    pub restored_at_ms: Option<i64>,
}

pub fn history_path_for_home(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("DiskDeck")
        .join("reclaim-history.ddrh")
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub fn new_event_id() -> u128 {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let process = u128::from(std::process::id());
    let sequence = u128::from(SEQUENCE.fetch_add(1, Ordering::Relaxed));
    (nanos << 48) | (process << 32) | sequence
}

fn put_len(out: &mut Vec<u8>, len: usize, limit: usize, field: &str) -> Result<(), String> {
    if len > limit {
        return Err(format!("{field} is too long"));
    }
    let len = u32::try_from(len).map_err(|_| format!("{field} is too long"))?;
    out.extend_from_slice(&len.to_le_bytes());
    Ok(())
}

fn put_text(out: &mut Vec<u8>, value: &str, field: &str) -> Result<(), String> {
    put_len(out, value.len(), MAX_TEXT_BYTES, field)?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_path(out: &mut Vec<u8>, value: &Path, field: &str) -> Result<(), String> {
    let bytes = value.as_os_str().as_bytes();
    put_len(out, bytes.len(), MAX_PATH_BYTES, field)?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn valid_receipt(receipt: &Receipt) -> Result<(), String> {
    if receipt.completed_at_ms < 0
        || receipt.freed_bytes < 0
        || receipt.pending_bytes < 0
        || receipt.restored_at_ms.is_some_and(|value| value < 0)
    {
        return Err("receipt contains a negative value".into());
    }
    match receipt.action {
        ReceiptAction::Trash => {
            if receipt.trash.is_some() == receipt.finder_managed {
                return Err("Trash receipt must have exactly one destination outcome".into());
            }
        }
        _ if receipt.trash.is_some()
            || receipt.finder_managed
            || receipt.restored_at_ms.is_some() =>
        {
            return Err("non-Trash receipt contains Trash state".into());
        }
        _ => {}
    }
    Ok(())
}

fn encode(receipts: &[Receipt]) -> Result<Vec<u8>, String> {
    if receipts.len() > MAX_RECEIPTS {
        return Err("too many reclaim receipts".into());
    }
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(receipts.len() as u32).to_le_bytes());
    for receipt in receipts {
        valid_receipt(receipt)?;
        out.extend_from_slice(&receipt.event_id.to_le_bytes());
        out.extend_from_slice(&receipt.completed_at_ms.to_le_bytes());
        put_text(&mut out, &receipt.rec_id, "recommendation id")?;
        put_text(&mut out, &receipt.title, "title")?;
        put_path(&mut out, &receipt.origin, "origin path")?;
        out.push(match receipt.action {
            ReceiptAction::Trash => 0,
            ReceiptAction::Delete => 1,
            ReceiptAction::Empty => 2,
            ReceiptAction::Command => 3,
        });
        out.extend_from_slice(&receipt.freed_bytes.to_le_bytes());
        out.extend_from_slice(&receipt.pending_bytes.to_le_bytes());
        match &receipt.trash {
            None => out.push(0),
            Some(evidence) => {
                out.push(1);
                put_path(&mut out, &evidence.path, "Trash path")?;
                out.extend_from_slice(&evidence.identity.dev.to_le_bytes());
                out.extend_from_slice(&evidence.identity.ino.to_le_bytes());
                out.push(match evidence.identity.kind {
                    FileKind::File => 0,
                    FileKind::Directory => 1,
                });
            }
        }
        out.push(u8::from(receipt.finder_managed));
        match receipt.restored_at_ms {
            None => out.push(0),
            Some(value) => {
                out.push(1);
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(out)
}

struct Decoder<'a> {
    cursor: Cursor<&'a [u8]>,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            cursor: Cursor::new(bytes),
        }
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut value = [0; N];
        self.cursor
            .read_exact(&mut value)
            .map_err(|_| "reclaim history is truncated".to_string())?;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }

    fn u128(&mut self) -> Result<u128, String> {
        Ok(u128::from_le_bytes(self.bytes()?))
    }

    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.bytes()?))
    }

    fn blob(&mut self, limit: usize, field: &str) -> Result<Vec<u8>, String> {
        let len = usize::try_from(self.u32()?).map_err(|_| format!("{field} is too long"))?;
        if len > limit {
            return Err(format!("{field} is too long"));
        }
        let mut bytes = vec![0; len];
        self.cursor
            .read_exact(&mut bytes)
            .map_err(|_| "reclaim history is truncated".to_string())?;
        Ok(bytes)
    }

    fn text(&mut self, field: &str) -> Result<String, String> {
        String::from_utf8(self.blob(MAX_TEXT_BYTES, field)?)
            .map_err(|_| format!("{field} is not UTF-8"))
    }

    fn path(&mut self, field: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(OsString::from_vec(
            self.blob(MAX_PATH_BYTES, field)?,
        )))
    }

    fn tag(&mut self, field: &str) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!("reclaim history contains an invalid {field} flag")),
        }
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<Receipt>, String> {
    let mut decoder = Decoder::new(bytes);
    if decoder.bytes::<5>()? != *MAGIC {
        return Err("reclaim history format is not supported".into());
    }
    let count = usize::try_from(decoder.u32()?).map_err(|_| "too many reclaim receipts")?;
    if count > MAX_RECEIPTS {
        return Err("too many reclaim receipts".into());
    }
    let mut receipts = Vec::with_capacity(count);
    for _ in 0..count {
        let event_id = decoder.u128()?;
        let completed_at_ms = decoder.i64()?;
        let rec_id = decoder.text("recommendation id")?;
        let title = decoder.text("title")?;
        let origin = decoder.path("origin path")?;
        let action = match decoder.u8()? {
            0 => ReceiptAction::Trash,
            1 => ReceiptAction::Delete,
            2 => ReceiptAction::Empty,
            3 => ReceiptAction::Command,
            _ => return Err("reclaim history contains an invalid action".into()),
        };
        let freed_bytes = decoder.i64()?;
        let pending_bytes = decoder.i64()?;
        let trash = if decoder.tag("Trash evidence")? {
            let path = decoder.path("Trash path")?;
            let dev = decoder.u64()?;
            let ino = decoder.u64()?;
            let kind = match decoder.u8()? {
                0 => FileKind::File,
                1 => FileKind::Directory,
                _ => return Err("reclaim history contains an invalid file kind".into()),
            };
            Some(TrashEvidence {
                path,
                identity: FileIdentity { dev, ino, kind },
            })
        } else {
            None
        };
        let finder_managed = decoder.tag("Finder-managed")?;
        let restored_at_ms = decoder
            .tag("restored")?
            .then(|| decoder.i64())
            .transpose()?;
        let receipt = Receipt {
            event_id,
            completed_at_ms,
            rec_id,
            title,
            origin,
            action,
            freed_bytes,
            pending_bytes,
            trash,
            finder_managed,
            restored_at_ms,
        };
        valid_receipt(&receipt)?;
        receipts.push(receipt);
    }
    if decoder.cursor.position() != bytes.len() as u64 {
        return Err("reclaim history contains trailing data".into());
    }
    Ok(receipts)
}

pub fn load_receipts(path: &Path) -> Result<Vec<Receipt>, String> {
    match fs::read(path) {
        Ok(bytes) => decode(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("read reclaim history: {error}")),
    }
}

fn write_receipts(path: &Path, receipts: &[Receipt]) -> Result<(), String> {
    let bytes = encode(receipts)?;
    let parent = path
        .parent()
        .ok_or_else(|| "reclaim history path has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create reclaim history folder: {error}"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "reclaim history path has no file name".to_string())?
        .to_string_lossy();
    let temp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        new_event_id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("create reclaim history: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("write reclaim history: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync reclaim history: {error}"))?;
        fs::rename(&temp, path).map_err(|error| format!("install reclaim history: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync reclaim history folder: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn append_receipt(path: &Path, receipt: Receipt) -> Result<(), String> {
    valid_receipt(&receipt)?;
    let mut receipts = load_receipts(path)?;
    receipts.push(receipt);
    if receipts.len() > MAX_RECEIPTS {
        let remove = receipts.len() - MAX_RECEIPTS;
        receipts.drain(..remove);
    }
    write_receipts(path, &receipts)
}

pub fn mark_restored(path: &Path, event_id: u128, restored_at_ms: i64) -> Result<(), String> {
    if restored_at_ms < 0 {
        return Err("restored timestamp is negative".into());
    }
    let mut receipts = load_receipts(path)?;
    let receipt = receipts
        .iter_mut()
        .find(|receipt| receipt.event_id == event_id)
        .ok_or_else(|| "reclaim receipt no longer exists".to_string())?;
    if receipt.action != ReceiptAction::Trash {
        return Err("only a Trash receipt can be marked restored".into());
    }
    receipt.restored_at_ms = Some(restored_at_ms);
    write_receipts(path, &receipts)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptState {
    Ready,
    Missing,
    OriginOccupied,
    Changed,
    ManualOnly,
    UnsafeOrigin,
    SymlinkAncestor,
    CrossDevice,
    Unavailable,
    Restored,
    Permanent,
}

#[derive(Clone, Debug)]
pub struct ReceiptItem {
    pub receipt: Receipt,
    pub state: ReceiptState,
}

#[derive(Clone, Debug, Default)]
pub struct ReclaimHistory {
    pub items: Vec<ReceiptItem>,
    pub freed_bytes: i64,
    pub pending_bytes: i64,
    pub recoverable_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreBlock {
    NotTrash,
    Missing,
    OriginOccupied,
    Changed,
    ManualOnly,
    UnsafeOrigin,
    SymlinkAncestor,
    CrossDevice,
    Unavailable,
    Restored,
}

impl RestoreBlock {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotTrash => "This cleanup was permanent and cannot be restored.",
            Self::Missing => "The recorded item is no longer in Trash.",
            Self::OriginOccupied => {
                "The original path is occupied; DiskDeck will not overwrite it."
            }
            Self::Changed => "The item in Trash changed and no longer matches this receipt.",
            Self::ManualOnly => "Finder chose the Trash destination; restore this item manually.",
            Self::UnsafeOrigin => {
                "The recorded original path is outside the safe restore boundary."
            }
            Self::SymlinkAncestor => "A parent of the original path is a symlink.",
            Self::CrossDevice => "Trash and the original folder are no longer on one volume.",
            Self::Unavailable => "DiskDeck cannot verify this restore path right now.",
            Self::Restored => "This receipt is already marked restored.",
        }
    }
}

fn normalized_absolute(path: &Path) -> bool {
    let mut components = path.components();
    if components.next() != Some(std::path::Component::RootDir) {
        return false;
    }
    components.all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn safe_origin(origin: &Path, home: &Path) -> bool {
    if !normalized_absolute(origin) || !normalized_absolute(home) {
        return false;
    }
    if origin == Path::new("/")
        || origin == Path::new("/Library")
        || origin == Path::new("/Users")
        || origin.starts_with("/System")
        || origin.starts_with("/Applications")
        || origin == home
        || origin == home.join("Library")
        || origin.starts_with(home.join(".Trash"))
    {
        return false;
    }
    true
}

fn exact_trash_path(path: &Path, home: &Path) -> bool {
    normalized_absolute(path)
        && path.file_name().is_some()
        && path.parent() == Some(home.join(".Trash").as_path())
}

fn ancestor_state(parent: &Path) -> Result<u64, ReceiptState> {
    let mut current = PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        let metadata = current.symlink_metadata().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ReceiptState::Unavailable
            } else {
                ReceiptState::Unavailable
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ReceiptState::SymlinkAncestor);
        }
        if !metadata.is_dir() {
            return Err(ReceiptState::Unavailable);
        }
    }
    parent
        .symlink_metadata()
        .map(|metadata| metadata.dev())
        .map_err(|_| ReceiptState::Unavailable)
}

fn device_state(trash_dev: u64, parent_dev: u64) -> Option<ReceiptState> {
    (trash_dev != parent_dev).then_some(ReceiptState::CrossDevice)
}

pub fn classify(receipt: &Receipt, home: &Path) -> ReceiptState {
    if receipt.action != ReceiptAction::Trash {
        return ReceiptState::Permanent;
    }
    if receipt.restored_at_ms.is_some() {
        return ReceiptState::Restored;
    }
    if receipt.finder_managed {
        return ReceiptState::ManualOnly;
    }
    let Some(evidence) = &receipt.trash else {
        return ReceiptState::Unavailable;
    };
    if !exact_trash_path(&evidence.path, home) || !safe_origin(&receipt.origin, home) {
        return ReceiptState::UnsafeOrigin;
    }
    let actual = match FileIdentity::at(&evidence.path) {
        Ok(identity) => identity,
        Err(_) => match evidence.path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ReceiptState::Missing;
            }
            Ok(_) => return ReceiptState::Changed,
            Err(_) => return ReceiptState::Unavailable,
        },
    };
    if actual != evidence.identity {
        return ReceiptState::Changed;
    }
    match receipt.origin.symlink_metadata() {
        Ok(_) => return ReceiptState::OriginOccupied,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return ReceiptState::Unavailable,
    }
    let Some(parent) = receipt.origin.parent() else {
        return ReceiptState::UnsafeOrigin;
    };
    let parent_dev = match ancestor_state(parent) {
        Ok(dev) => dev,
        Err(state) => return state,
    };
    if let Some(state) = device_state(actual.dev, parent_dev) {
        return state;
    }
    ReceiptState::Ready
}

fn block_for_state(state: &ReceiptState) -> Option<RestoreBlock> {
    Some(match state {
        ReceiptState::Ready => return None,
        ReceiptState::Missing => RestoreBlock::Missing,
        ReceiptState::OriginOccupied => RestoreBlock::OriginOccupied,
        ReceiptState::Changed => RestoreBlock::Changed,
        ReceiptState::ManualOnly => RestoreBlock::ManualOnly,
        ReceiptState::UnsafeOrigin => RestoreBlock::UnsafeOrigin,
        ReceiptState::SymlinkAncestor => RestoreBlock::SymlinkAncestor,
        ReceiptState::CrossDevice => RestoreBlock::CrossDevice,
        ReceiptState::Unavailable => RestoreBlock::Unavailable,
        ReceiptState::Restored => RestoreBlock::Restored,
        ReceiptState::Permanent => RestoreBlock::NotTrash,
    })
}

pub fn can_confirm_restore(acknowledged: bool, state: &ReceiptState) -> bool {
    acknowledged && block_for_state(state).is_none()
}

pub fn refresh_history(path: &Path, home: &Path) -> Result<ReclaimHistory, String> {
    let mut receipts = load_receipts(path)?;
    receipts.sort_by(|a, b| {
        b.completed_at_ms
            .cmp(&a.completed_at_ms)
            .then(b.event_id.cmp(&a.event_id))
    });
    let mut history = ReclaimHistory::default();
    for receipt in receipts {
        let state = classify(&receipt, home);
        if receipt.action != ReceiptAction::Trash {
            history.freed_bytes = history
                .freed_bytes
                .saturating_add(receipt.freed_bytes.max(0));
        }
        if matches!(
            state,
            ReceiptState::Ready
                | ReceiptState::OriginOccupied
                | ReceiptState::SymlinkAncestor
                | ReceiptState::CrossDevice
        ) {
            history.pending_bytes = history
                .pending_bytes
                .saturating_add(receipt.pending_bytes.max(0));
        }
        if state == ReceiptState::Ready {
            history.recoverable_count = history.recoverable_count.saturating_add(1);
        }
        history.items.push(ReceiptItem { receipt, state });
    }
    Ok(history)
}

struct RestorePlan {
    trash: PathBuf,
    origin: PathBuf,
    identity: FileIdentity,
    bytes: i64,
}

fn preflight_restore(receipt: &Receipt, home: &Path) -> Result<RestorePlan, RestoreBlock> {
    let state = classify(receipt, home);
    if let Some(block) = block_for_state(&state) {
        return Err(block);
    }
    let evidence = receipt.trash.as_ref().ok_or(RestoreBlock::Unavailable)?;
    Ok(RestorePlan {
        trash: evidence.path.clone(),
        origin: receipt.origin.clone(),
        identity: evidence.identity,
        bytes: receipt.pending_bytes.max(0),
    })
}

#[derive(Clone, Debug)]
pub struct RestoreJob {
    pub receipt: Receipt,
    pub history_path: PathBuf,
    pub home: PathBuf,
}

#[derive(Debug)]
pub enum RestoreEvent {
    Started {
        title: String,
        bytes: i64,
    },
    Done {
        bytes: i64,
        origin: PathBuf,
        warning: Option<String>,
    },
    Failed {
        error: String,
    },
}

struct RestoreOutcome {
    bytes: i64,
    origin: PathBuf,
    warning: Option<String>,
}

fn perform_restore(job: &RestoreJob) -> Result<RestoreOutcome, String> {
    let plan =
        preflight_restore(&job.receipt, &job.home).map_err(|block| block.message().to_string())?;
    rename_exclusive(&plan.trash, &plan.origin)
        .map_err(|error| format!("restore item to original path: {error}"))?;
    let actual = FileIdentity::at(&plan.origin);
    if actual.as_ref() != Ok(&plan.identity) {
        let rollback = rename_exclusive(&plan.origin, &plan.trash);
        return Err(match rollback {
            Ok(()) => "restored item identity changed; item returned to Trash".into(),
            Err(error) => format!(
                "restored item identity changed; return it to Trash manually from {} ({error})",
                plan.origin.display()
            ),
        });
    }
    let restored_at_ms = now_ms();
    let warning = mark_restored(&job.history_path, job.receipt.event_id, restored_at_ms)
        .err()
        .map(|error| format!("restored, but reclaim history could not be updated — {error}"));
    Ok(RestoreOutcome {
        bytes: plan.bytes,
        origin: plan.origin,
        warning,
    })
}

pub fn run_restore(
    job: RestoreJob,
    tx: std::sync::mpsc::Sender<RestoreEvent>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("trash-restore".into())
        .spawn(move || {
            let _ = tx.send(RestoreEvent::Started {
                title: job.receipt.title.clone(),
                bytes: job.receipt.pending_bytes.max(0),
            });
            match perform_restore(&job) {
                Ok(outcome) => {
                    let _ = tx.send(RestoreEvent::Done {
                        bytes: outcome.bytes,
                        origin: outcome.origin,
                        warning: outcome.warning,
                    });
                }
                Err(error) => {
                    let _ = tx.send(RestoreEvent::Failed { error });
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("start Trash restore worker: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    struct RestoreFixture {
        _temp: tempfile::TempDir,
        home: PathBuf,
        trash_item: PathBuf,
        origin: PathBuf,
        history: PathBuf,
        receipt: Receipt,
    }

    impl RestoreFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = fs::canonicalize(temp.path()).unwrap().join("home");
            let trash = home.join(".Trash");
            let origin = home.join("Library/Caches/cache");
            let trash_item = trash.join("cache");
            let history = temp.path().join("reclaim-history.ddrh");
            fs::create_dir_all(&trash).unwrap();
            fs::create_dir_all(origin.parent().unwrap()).unwrap();
            fs::write(&trash_item, b"fixture").unwrap();
            let receipt = Receipt {
                event_id: 101,
                completed_at_ms: 42,
                rec_id: "fixture-rule".into(),
                title: "Fixture cache".into(),
                origin: origin.clone(),
                action: ReceiptAction::Trash,
                freed_bytes: 0,
                pending_bytes: 7,
                trash: Some(TrashEvidence {
                    path: trash_item.clone(),
                    identity: FileIdentity::at(&trash_item).unwrap(),
                }),
                finder_managed: false,
                restored_at_ms: None,
            };
            append_receipt(&history, receipt.clone()).unwrap();
            Self {
                _temp: temp,
                home,
                trash_item,
                origin,
                history,
                receipt,
            }
        }

        fn job(&self) -> RestoreJob {
            RestoreJob {
                receipt: self.receipt.clone(),
                history_path: self.history.clone(),
                home: self.home.clone(),
            }
        }

        fn replace_trash_item(&self) {
            fs::remove_file(&self.trash_item).unwrap();
            fs::write(&self.trash_item, b"replacement").unwrap();
        }
    }

    fn fixture_receipt(origin: PathBuf, action: ReceiptAction) -> Receipt {
        Receipt {
            event_id: 7,
            completed_at_ms: 42,
            rec_id: "fixture-rule".into(),
            title: "Fixture cache".into(),
            origin,
            action,
            freed_bytes: 11,
            pending_bytes: 13,
            trash: (action == ReceiptAction::Trash).then(|| TrashEvidence {
                path: PathBuf::from("/tmp/home/.Trash/cache"),
                identity: FileIdentity {
                    dev: 17,
                    ino: 19,
                    kind: FileKind::Directory,
                },
            }),
            finder_managed: false,
            restored_at_ms: None,
        }
    }

    fn fixture_receipt_with_id(id: u128) -> Receipt {
        let mut receipt = fixture_receipt(
            PathBuf::from(format!("/tmp/cache-{id}")),
            ReceiptAction::Delete,
        );
        receipt.event_id = id;
        receipt
    }

    #[test]
    fn ddrh1_round_trips_raw_paths_and_all_actions() {
        let raw = std::ffi::OsString::from_vec(b"/tmp/cache-\xff".to_vec());
        let receipts = vec![
            fixture_receipt(PathBuf::from(raw), ReceiptAction::Trash),
            fixture_receipt(PathBuf::from("/tmp/delete"), ReceiptAction::Delete),
            fixture_receipt(PathBuf::from("/tmp/empty"), ReceiptAction::Empty),
            fixture_receipt(PathBuf::from("/tmp/command"), ReceiptAction::Command),
        ];
        assert_eq!(decode(&encode(&receipts).unwrap()).unwrap(), receipts);
    }

    #[test]
    fn codec_rejects_wrong_truncated_invalid_and_trailing_payloads() {
        let bytes = encode(&[fixture_receipt(
            PathBuf::from("/tmp/a"),
            ReceiptAction::Delete,
        )])
        .unwrap();
        assert!(decode(b"NOPE1").is_err());
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());

        let mut invalid_action = bytes.clone();
        let action_offset = invalid_action
            .windows(b"Fixture cache".len())
            .position(|window| window == b"Fixture cache")
            .unwrap()
            + b"Fixture cache".len()
            + 4
            + b"/tmp/a".len();
        invalid_action[action_offset] = 99;
        assert!(decode(&invalid_action).is_err());

        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
    }

    #[test]
    fn append_is_bounded_atomic_and_refuses_corrupt_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reclaim-history.ddrh");
        for index in 0..205 {
            append_receipt(&path, fixture_receipt_with_id(index)).unwrap();
        }
        let stored = load_receipts(&path).unwrap();
        assert_eq!(stored.len(), 200);
        assert_eq!(stored.first().unwrap().event_id, 5);

        std::fs::write(&path, b"corrupt").unwrap();
        assert!(append_receipt(&path, fixture_receipt_with_id(999)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
    }

    #[test]
    fn mark_restored_updates_only_the_matching_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reclaim-history.ddrh");
        let mut receipt = fixture_receipt(PathBuf::from("/tmp/cache-7"), ReceiptAction::Trash);
        receipt.event_id = 7;
        append_receipt(&path, receipt).unwrap();
        mark_restored(&path, 7, 42).unwrap();
        assert_eq!(load_receipts(&path).unwrap()[0].restored_at_ms, Some(42));
        assert!(mark_restored(&path, 999, 84).is_err());
    }

    #[test]
    fn history_path_uses_diskdeck_application_support() {
        assert_eq!(
            history_path_for_home(Path::new("/fixture/home")),
            PathBuf::from(
                "/fixture/home/Library/Application Support/DiskDeck/reclaim-history.ddrh"
            )
        );
    }

    #[test]
    fn classification_distinguishes_ready_missing_occupied_changed_manual_and_restored() {
        let ready = RestoreFixture::new();
        assert_eq!(classify(&ready.receipt, &ready.home), ReceiptState::Ready);

        let missing = RestoreFixture::new();
        fs::remove_file(&missing.trash_item).unwrap();
        assert_eq!(
            classify(&missing.receipt, &missing.home),
            ReceiptState::Missing
        );

        let occupied = RestoreFixture::new();
        fs::write(&occupied.origin, b"occupied").unwrap();
        assert_eq!(
            classify(&occupied.receipt, &occupied.home),
            ReceiptState::OriginOccupied
        );

        let changed = RestoreFixture::new();
        changed.replace_trash_item();
        assert_eq!(
            classify(&changed.receipt, &changed.home),
            ReceiptState::Changed
        );

        let manual = RestoreFixture::new();
        let mut manual_receipt = manual.receipt.clone();
        manual_receipt.trash = None;
        manual_receipt.finder_managed = true;
        assert_eq!(
            classify(&manual_receipt, &manual.home),
            ReceiptState::ManualOnly
        );

        let restored = RestoreFixture::new();
        let mut restored_receipt = restored.receipt.clone();
        restored_receipt.restored_at_ms = Some(84);
        assert_eq!(
            classify(&restored_receipt, &restored.home),
            ReceiptState::Restored
        );

        let permanent = RestoreFixture::new();
        let mut permanent_receipt = fixture_receipt(
            permanent.home.join("Library/Caches/permanent"),
            ReceiptAction::Delete,
        );
        permanent_receipt.trash = None;
        assert_eq!(
            classify(&permanent_receipt, &permanent.home),
            ReceiptState::Permanent
        );
    }

    #[test]
    fn classification_blocks_symlink_ancestors_protected_origins_and_cross_device() {
        let symlinked = RestoreFixture::new();
        let real_parent = symlinked.home.join("real-parent");
        let linked_parent = symlinked.home.join("linked-parent");
        fs::create_dir_all(&real_parent).unwrap();
        symlink(&real_parent, &linked_parent).unwrap();
        let mut linked_receipt = symlinked.receipt.clone();
        linked_receipt.origin = linked_parent.join("cache");
        assert_eq!(
            classify(&linked_receipt, &symlinked.home),
            ReceiptState::SymlinkAncestor
        );

        let protected = RestoreFixture::new();
        let mut protected_receipt = protected.receipt.clone();
        protected_receipt.origin = PathBuf::from("/System/cache");
        assert_eq!(
            classify(&protected_receipt, &protected.home),
            ReceiptState::UnsafeOrigin
        );
        assert_eq!(device_state(1, 2), Some(ReceiptState::CrossDevice));
        assert_eq!(device_state(1, 1), None);
    }

    #[test]
    fn refresh_summarizes_only_current_receipt_evidence() {
        let fixture = RestoreFixture::new();
        let mut permanent = fixture_receipt(
            fixture.home.join("Library/Caches/deleted"),
            ReceiptAction::Delete,
        );
        permanent.event_id = 102;
        permanent.freed_bytes = 11;
        permanent.pending_bytes = 0;
        append_receipt(&fixture.history, permanent).unwrap();

        let history = refresh_history(&fixture.history, &fixture.home).unwrap();
        assert_eq!(history.items.len(), 2);
        assert_eq!(history.freed_bytes, 11);
        assert_eq!(history.pending_bytes, 7);
        assert_eq!(history.recoverable_count, 1);
        assert_eq!(history.items[0].receipt.event_id, 102);
    }

    #[test]
    fn restore_moves_exact_item_back_and_marks_only_its_receipt() {
        let fixture = RestoreFixture::new();
        let outcome = perform_restore(&fixture.job()).unwrap();
        assert_eq!(fs::read(&fixture.origin).unwrap(), b"fixture");
        assert!(!fixture.trash_item.exists());
        assert_eq!(outcome.bytes, 7);
        assert!(load_receipts(&fixture.history).unwrap()[0]
            .restored_at_ms
            .is_some());
    }

    #[test]
    fn restore_refuses_occupied_or_replaced_paths_before_mutation() {
        let occupied = RestoreFixture::new();
        fs::write(&occupied.origin, b"occupied").unwrap();
        assert!(perform_restore(&occupied.job()).is_err());
        assert_eq!(fs::read(&occupied.origin).unwrap(), b"occupied");
        assert!(occupied.trash_item.exists());

        let changed = RestoreFixture::new();
        assert_eq!(
            classify(&changed.receipt, &changed.home),
            ReceiptState::Ready
        );
        changed.replace_trash_item();
        assert!(perform_restore(&changed.job()).is_err());
        assert!(!changed.origin.exists());
        assert!(changed.trash_item.exists());
    }

    #[test]
    fn restore_success_survives_history_write_failure_as_a_warning() {
        let fixture = RestoreFixture::new();
        fs::write(&fixture.history, b"corrupt").unwrap();
        let outcome = perform_restore(&fixture.job()).unwrap();
        assert!(outcome.warning.unwrap().contains("history"));
        assert_eq!(fs::read(&fixture.origin).unwrap(), b"fixture");
    }

    #[test]
    fn restore_confirmation_and_worker_events_are_fail_closed() {
        assert!(can_confirm_restore(true, &ReceiptState::Ready));
        assert!(!can_confirm_restore(false, &ReceiptState::Ready));
        assert!(!can_confirm_restore(true, &ReceiptState::Changed));

        let fixture = RestoreFixture::new();
        let (tx, rx) = std::sync::mpsc::channel();
        run_restore(fixture.job(), tx).unwrap();
        let events: Vec<_> = rx.into_iter().collect();
        assert!(matches!(events.first(), Some(RestoreEvent::Started { .. })));
        assert!(matches!(events.last(), Some(RestoreEvent::Done { .. })));
    }

    #[test]
    #[ignore = "writes a reversible fixture for signed visual QA"]
    fn seed_signed_visual_fixture() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is required"));
        let history = PathBuf::from(
            std::env::var_os("DISKDECK_QA_HISTORY").expect("DISKDECK_QA_HISTORY is required"),
        );
        assert_eq!(history, history_path_for_home(&home));
        assert!(
            !history.exists(),
            "back up and remove existing history first"
        );

        let trash_item = home.join(".Trash/DiskDeck-QA-Reclaim-History");
        let origin = home.join("Library/Caches/DiskDeck-QA-Reclaim-History");
        assert!(!trash_item.exists(), "QA Trash fixture already exists");
        assert!(!origin.exists(), "QA origin fixture already exists");
        fs::create_dir(&trash_item).unwrap();
        fs::write(trash_item.join("fixture.txt"), b"DiskDeck visual QA").unwrap();
        let evidence = TrashEvidence {
            path: trash_item.clone(),
            identity: FileIdentity::at(&trash_item).unwrap(),
        };
        let receipt = |event_id, title: &str| Receipt {
            event_id,
            completed_at_ms: 1_700_000_000_000 + i64::try_from(event_id).unwrap(),
            rec_id: format!("qa-{event_id}"),
            title: title.into(),
            origin: origin.clone(),
            action: ReceiptAction::Trash,
            freed_bytes: 0,
            pending_bytes: 24_000_000,
            trash: Some(evidence.clone()),
            finder_managed: false,
            restored_at_ms: None,
        };

        let ready = receipt(1, "Ready cache fixture");
        let mut missing = receipt(2, "Missing Trash fixture");
        missing.trash.as_mut().unwrap().path = home.join(".Trash/DiskDeck-QA-Missing");
        let mut changed = receipt(3, "Changed Trash fixture");
        changed.trash.as_mut().unwrap().identity.ino = changed
            .trash
            .as_ref()
            .unwrap()
            .identity
            .ino
            .saturating_add(1);
        let mut manual = receipt(4, "Finder-managed fixture");
        manual.trash = None;
        manual.finder_managed = true;
        let mut permanent = receipt(5, "Permanent cleanup fixture");
        permanent.action = ReceiptAction::Delete;
        permanent.trash = None;
        permanent.freed_bytes = 12_000_000;
        permanent.pending_bytes = 0;
        let mut restored = receipt(6, "Restored cache fixture");
        restored.restored_at_ms = Some(1_700_000_000_100);

        for item in [ready, missing, changed, manual, permanent, restored] {
            append_receipt(&history, item).unwrap();
        }
    }
}
