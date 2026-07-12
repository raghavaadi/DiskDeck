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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::path::{Path, PathBuf};

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
}
