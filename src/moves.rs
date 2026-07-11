//! Durable move records, state reconciliation, and verified restore.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"DDMOVE1\0";
const MAX_DECODE_RECORDS: usize = 4096;
const MAX_RECORDS: usize = 512;
const MAX_PATH_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveRecord {
    pub origin: PathBuf,
    pub dest: PathBuf,
    pub moved_at: i64,
    pub bytes: i64,
    pub symlinked: bool,
    pub restored_at: Option<i64>,
}

impl MoveRecord {
    pub fn new(origin: PathBuf, dest: PathBuf, moved_at: i64, bytes: i64, symlinked: bool) -> Self {
        Self {
            origin,
            dest,
            moved_at,
            bytes,
            symlinked,
            restored_at: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveState {
    Ready,
    DriveDisconnected,
    OriginChanged,
    TargetMissing,
    Restored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedItem {
    pub record: MoveRecord,
    pub state: MoveState,
}

pub fn registry_path_for_home(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("DiskDeck")
        .join("Moves")
        .join("index.ddmoves")
}

fn put_path(out: &mut Vec<u8>, path: &Path) -> Result<(), String> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() > MAX_PATH_BYTES {
        return Err("move record path exceeds 1 MiB".into());
    }
    let len = u32::try_from(bytes.len()).map_err(|_| "move record path is too long")?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn encode_registry(records: &[MoveRecord]) -> Result<Vec<u8>, String> {
    if records.len() > MAX_DECODE_RECORDS {
        return Err("move registry has too many records".into());
    }
    let count = u32::try_from(records.len()).map_err(|_| "move registry is too large")?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&count.to_le_bytes());
    for record in records {
        out.extend_from_slice(&record.moved_at.to_le_bytes());
        out.extend_from_slice(&record.bytes.to_le_bytes());
        let mut flags = 0u8;
        if record.symlinked {
            flags |= 1;
        }
        if record.restored_at.is_some() {
            flags |= 2;
        }
        out.push(flags);
        out.extend_from_slice(&record.restored_at.unwrap_or(0).to_le_bytes());
        put_path(&mut out, &record.origin)?;
        put_path(&mut out, &record.dest)?;
    }
    Ok(out)
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| "move registry length overflow".to_string())?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| "move registry is truncated".to_string())?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(bytes))
    }

    fn i64(&mut self) -> Result<i64, String> {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(i64::from_le_bytes(bytes))
    }

    fn path(&mut self) -> Result<PathBuf, String> {
        let len = usize::try_from(self.u32()?).map_err(|_| "invalid path length")?;
        if len > MAX_PATH_BYTES {
            return Err("move record path exceeds 1 MiB".into());
        }
        Ok(PathBuf::from(OsString::from_vec(self.take(len)?.to_vec())))
    }
}

fn decode_registry(bytes: &[u8]) -> Result<Vec<MoveRecord>, String> {
    let mut decoder = Decoder { bytes, offset: 0 };
    if decoder.take(MAGIC.len())? != MAGIC {
        return Err("unsupported move registry format".into());
    }
    let count = usize::try_from(decoder.u32()?).map_err(|_| "invalid move record count")?;
    if count > MAX_DECODE_RECORDS {
        return Err("move registry has too many records".into());
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let moved_at = decoder.i64()?;
        let bytes = decoder.i64()?;
        let flags = decoder.u8()?;
        if flags & !3 != 0 {
            return Err("move registry contains unknown flags".into());
        }
        let restored_value = decoder.i64()?;
        let origin = decoder.path()?;
        let dest = decoder.path()?;
        records.push(MoveRecord {
            origin,
            dest,
            moved_at,
            bytes,
            symlinked: flags & 1 != 0,
            restored_at: (flags & 2 != 0).then_some(restored_value),
        });
    }
    if decoder.offset != bytes.len() {
        return Err("move registry contains trailing data".into());
    }
    Ok(records)
}

pub fn load_records(path: &Path) -> Result<Vec<MoveRecord>, String> {
    match std::fs::read(path) {
        Ok(bytes) => decode_registry(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("read move registry: {error}")),
    }
}

fn write_records(path: &Path, records: &[MoveRecord]) -> Result<(), String> {
    let bytes = encode_registry(records)?;
    let parent = path
        .parent()
        .ok_or_else(|| "move registry has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("prepare move registry: {error}"))?;
    let mut temp = None;
    for index in 0..100u32 {
        let candidate = parent.join(format!(".index.ddmoves.tmp-{}-{index}", std::process::id()));
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => {
                temp = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create move registry update: {error}")),
        }
    }
    let (temp_path, mut file) =
        temp.ok_or_else(|| "no move registry temp name available".to_string())?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|error| format!("write move registry: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync move registry: {error}"))?;
        drop(file);
        std::fs::rename(&temp_path, path)
            .map_err(|error| format!("install move registry: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

pub fn upsert_record(path: &Path, record: MoveRecord) -> Result<(), String> {
    let mut records = load_records(path)?;
    records.retain(|existing| existing.origin != record.origin || existing.dest != record.dest);
    records.push(record);
    records.sort_by(|left, right| right.moved_at.cmp(&left.moved_at));
    records.truncate(MAX_RECORDS);
    write_records(path, &records)
}

pub fn mark_restored(path: &Path, record: &MoveRecord, restored_at: i64) -> Result<(), String> {
    let mut records = load_records(path)?;
    let stored = records
        .iter_mut()
        .find(|stored| stored.origin == record.origin && stored.dest == record.dest)
        .ok_or_else(|| "move record is no longer in the registry".to_string())?;
    stored.restored_at = Some(restored_at);
    write_records(path, &records)
}

fn has_dot_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )
    })
}

fn volume_root(record: &MoveRecord, volumes_root: &Path) -> Option<PathBuf> {
    let relative = record.dest.strip_prefix(volumes_root).ok()?;
    let mut components = relative.components();
    let std::path::Component::Normal(volume) = components.next()? else {
        return None;
    };
    let std::path::Component::Normal(offload) = components.next()? else {
        return None;
    };
    if offload != "DiskDeck Offload" || components.next().is_none() {
        return None;
    }
    Some(volumes_root.join(volume))
}

fn safe_record_paths(record: &MoveRecord, home: &Path, volumes_root: &Path) -> Option<PathBuf> {
    if !record.origin.is_absolute()
        || !record.dest.is_absolute()
        || has_dot_component(&record.origin)
        || has_dot_component(&record.dest)
        || !record.origin.starts_with(home)
        || record.origin == home
    {
        return None;
    }
    volume_root(record, volumes_root)
}

pub fn inspect_record(record: &MoveRecord, home: &Path, volumes_root: &Path) -> MoveState {
    if record.restored_at.is_some() {
        return MoveState::Restored;
    }
    let Some(volume) = safe_record_paths(record, home, volumes_root) else {
        return MoveState::OriginChanged;
    };
    if !volume.is_dir() {
        return MoveState::DriveDisconnected;
    }
    let target = match std::fs::symlink_metadata(&record.dest) {
        Ok(metadata) if !metadata.file_type().is_symlink() => metadata,
        _ => return MoveState::TargetMissing,
    };
    if !target.is_file() && !target.is_dir() {
        return MoveState::TargetMissing;
    }
    match std::fs::symlink_metadata(&record.origin) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => MoveState::Ready,
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if std::fs::read_link(&record.origin).ok().as_deref() == Some(record.dest.as_path()) {
                MoveState::Ready
            } else {
                MoveState::OriginChanged
            }
        }
        _ => MoveState::OriginChanged,
    }
}

pub fn state_reason(state: MoveState) -> &'static str {
    match state {
        MoveState::Ready => "Ready to restore",
        MoveState::DriveDisconnected => "Connect the recorded external drive to restore this item.",
        MoveState::OriginChanged => {
            "The original path is occupied or no longer matches DiskDeck's link."
        }
        MoveState::TargetMissing => "The recorded item is missing or unsafe on the attached drive.",
        MoveState::Restored => "Restored to the Mac",
    }
}

fn json_value_start<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    Some(line[start..].trim_start())
}

fn json_string_field(line: &str, key: &str) -> Option<String> {
    let value = json_value_start(line, key)?;
    let bytes = value.as_bytes();
    if bytes.first().copied()? != b'"' {
        return None;
    }
    let mut out = String::new();
    let mut index = 1usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some(out),
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hex = std::str::from_utf8(bytes.get(index + 1..index + 5)?).ok()?;
                        let scalar = u32::from_str_radix(hex, 16).ok()?;
                        out.push(char::from_u32(scalar)?);
                        index += 4;
                    }
                    _ => return None,
                }
            }
            byte if byte < 0x80 => out.push(byte as char),
            _ => {
                let tail = std::str::from_utf8(bytes.get(index..)?).ok()?;
                let ch = tail.chars().next()?;
                out.push(ch);
                index += ch.len_utf8() - 1;
            }
        }
        index += 1;
    }
    None
}

fn json_i64_field(line: &str, key: &str) -> Option<i64> {
    let value = json_value_start(line, key)?;
    let end = value.find([',', '}']).unwrap_or(value.len());
    value[..end].trim().parse().ok()
}

fn json_bool_field(line: &str, key: &str) -> Option<bool> {
    let value = json_value_start(line, key)?;
    if value.starts_with("true") {
        Some(true)
    } else if value.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn legacy_records(home: &Path, volumes_root: &Path) -> Vec<MoveRecord> {
    let Ok(volumes) = std::fs::read_dir(volumes_root) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for volume in volumes.flatten() {
        if !volume
            .file_type()
            .map(|kind| kind.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let ledger = volume
            .path()
            .join("DiskDeck Offload")
            .join(".diskdeck-offload.json");
        let Ok(contents) = std::fs::read_to_string(ledger) else {
            continue;
        };
        for line in contents.lines() {
            let Some(origin) = json_string_field(line, "origin").map(PathBuf::from) else {
                continue;
            };
            let Some(dest) = json_string_field(line, "dest").map(PathBuf::from) else {
                continue;
            };
            let Some(moved_at) = json_i64_field(line, "moved_at") else {
                continue;
            };
            let Some(symlinked) = json_bool_field(line, "symlinked") else {
                continue;
            };
            let bytes = crate::transfer::apparent_size(&dest);
            let record = MoveRecord::new(origin, dest, moved_at, bytes, symlinked);
            if safe_record_paths(&record, home, volumes_root).is_some() {
                records.push(record);
            }
        }
    }
    records
}

pub fn refresh_records(
    registry: &Path,
    home: &Path,
    volumes_root: &Path,
) -> Result<Vec<MovedItem>, String> {
    let mut records = load_records(registry)?;
    let mut changed = false;
    for imported in legacy_records(home, volumes_root) {
        if records
            .iter()
            .any(|record| record.origin == imported.origin && record.dest == imported.dest)
        {
            continue;
        }
        records.push(imported);
        changed = true;
    }
    records.sort_by(|left, right| right.moved_at.cmp(&left.moved_at));
    records.truncate(MAX_RECORDS);
    if changed {
        write_records(registry, &records)?;
    }
    Ok(records
        .into_iter()
        .map(|record| MovedItem {
            state: inspect_record(&record, home, volumes_root),
            record,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    fn record(index: i64) -> MoveRecord {
        MoveRecord {
            origin: PathBuf::from(format!("/Users/<user>/item-{index}")),
            dest: PathBuf::from(format!("/Volumes/<external>/DiskDeck Offload/item-{index}")),
            moved_at: index,
            bytes: index,
            symlinked: false,
            restored_at: None,
        }
    }

    #[test]
    fn registry_round_trips_raw_paths_and_restore_state() {
        let origin = PathBuf::from(OsString::from_vec(b"/Users/<user>/clip-\xff".to_vec()));
        let record = MoveRecord {
            origin,
            dest: PathBuf::from("/Volumes/<external>/DiskDeck Offload/Users/<user>/clip"),
            moved_at: 42,
            bytes: 7,
            symlinked: true,
            restored_at: Some(84),
        };

        let encoded = encode_registry(&[record.clone()]).unwrap();
        assert_eq!(decode_registry(&encoded).unwrap(), vec![record]);
    }

    #[test]
    fn registry_rejects_wrong_truncated_and_trailing_payloads() {
        let encoded = encode_registry(&[record(1)]).unwrap();
        let mut wrong_magic = encoded.clone();
        wrong_magic[0] = b'X';
        assert!(decode_registry(&wrong_magic).is_err());
        assert!(decode_registry(&encoded[..encoded.len() - 1]).is_err());
        let mut trailing = encoded;
        trailing.push(0);
        assert!(decode_registry(&trailing).is_err());
    }

    #[test]
    fn upsert_is_atomic_deduplicated_and_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.ddmoves");
        for i in 0..520 {
            upsert_record(&path, record(i)).unwrap();
        }
        let mut newest = record(519);
        newest.moved_at = 999;
        newest.bytes = 123;
        newest.symlinked = true;
        upsert_record(&path, newest).unwrap();

        let records = load_records(&path).unwrap();
        assert_eq!(records.len(), MAX_RECORDS);
        assert_eq!(records[0].moved_at, 999);
        assert_eq!(records[0].bytes, 123);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn exact_origin_symlink_is_ready_but_a_different_link_blocks_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let volumes = tmp.path().join("Volumes");
        let origin = home.join("Movies/clip.mov");
        let dest = volumes.join("<external>/DiskDeck Offload/Users/<user>/Movies/clip.mov");
        std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"clip").unwrap();
        std::os::unix::fs::symlink(&dest, &origin).unwrap();
        let record = MoveRecord {
            origin: origin.clone(),
            dest: dest.clone(),
            moved_at: 42,
            bytes: 4,
            symlinked: true,
            restored_at: None,
        };

        assert_eq!(inspect_record(&record, &home, &volumes), MoveState::Ready);

        std::fs::remove_file(&origin).unwrap();
        std::os::unix::fs::symlink(dest.with_file_name("other.mov"), &origin).unwrap();
        assert_eq!(
            inspect_record(&record, &home, &volumes),
            MoveState::OriginChanged
        );
    }

    #[test]
    fn detached_drive_is_not_misreported_as_a_missing_target() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let volumes = tmp.path().join("Volumes");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&volumes).unwrap();
        let record = MoveRecord {
            origin: home.join("Movies/clip.mov"),
            dest: volumes.join("<external>/DiskDeck Offload/Users/<user>/Movies/clip.mov"),
            moved_at: 42,
            bytes: 4,
            symlinked: false,
            restored_at: None,
        };

        assert_eq!(
            inspect_record(&record, &home, &volumes),
            MoveState::DriveDisconnected
        );
    }

    #[test]
    fn attached_missing_target_and_restored_record_have_distinct_states() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let volumes = tmp.path().join("Volumes");
        let drive = volumes.join("<external>");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&drive).unwrap();
        let mut record = MoveRecord {
            origin: home.join("Movies/clip.mov"),
            dest: drive.join("DiskDeck Offload/Users/<user>/Movies/clip.mov"),
            moved_at: 42,
            bytes: 4,
            symlinked: false,
            restored_at: None,
        };

        assert_eq!(
            inspect_record(&record, &home, &volumes),
            MoveState::TargetMissing
        );
        record.restored_at = Some(84);
        assert_eq!(
            inspect_record(&record, &home, &volumes),
            MoveState::Restored
        );
    }

    #[test]
    fn legacy_import_accepts_only_normalized_paths_under_diskdeck_offload() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let volumes = tmp.path().join("Volumes");
        let drive = volumes.join("<external>");
        let offload = drive.join("DiskDeck Offload");
        std::fs::create_dir_all(&offload).unwrap();
        let valid_origin = home.join("Movies/clip.mov");
        let valid_dest = offload.join("Users/<user>/Movies/clip.mov");
        let ledger = format!(
            "{{\"origin\":\"{}\",\"dest\":\"{}\",\"moved_at\":42,\"symlinked\":false}}\n\
             {{\"origin\":\"{}\",\"dest\":\"{}\",\"moved_at\":43,\"symlinked\":false}}\n",
            valid_origin.display(),
            valid_dest.display(),
            valid_origin.display(),
            offload.join("../escape").display(),
        );
        std::fs::write(offload.join(".diskdeck-offload.json"), ledger).unwrap();
        let registry = tmp.path().join("index.ddmoves");

        let items = refresh_records(&registry, &home, &volumes).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].record.dest, valid_dest);
    }
}
