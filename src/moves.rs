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
}
