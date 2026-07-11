use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Cursor, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

const MAGIC: &[u8; 8] = b"DDHIST1\0";
const MIN_GROWTH_BYTES: i64 = 10 << 20;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_PATH_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Growth {
    pub path: PathBuf,
    pub bytes_delta: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrowthSummary {
    pub previous_at_ms: i64,
    pub total_delta: i64,
    pub growers: Vec<Growth>,
}

pub enum HistoryEvent {
    BaselineSaved,
    Compared(GrowthSummary),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    bytes: i64,
    files: i64,
    is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    captured_at_ms: i64,
    root: PathBuf,
    total_bytes: i64,
    entries: Vec<Entry>,
}

fn compare(previous: &Snapshot, current: &Snapshot) -> Option<GrowthSummary> {
    if previous.root != current.root {
        return None;
    }
    let previous_bytes: HashMap<&std::path::Path, i64> = previous
        .entries
        .iter()
        .map(|entry| (entry.path.as_path(), entry.bytes))
        .collect();
    let mut growers: Vec<Growth> = current
        .entries
        .iter()
        .filter_map(|entry| {
            let before = previous_bytes
                .get(entry.path.as_path())
                .copied()
                .unwrap_or(0);
            let bytes_delta = entry.bytes - before;
            (bytes_delta >= MIN_GROWTH_BYTES).then(|| Growth {
                path: entry.path.clone(),
                bytes_delta,
            })
        })
        .collect();
    growers.sort_by(|left, right| {
        right
            .bytes_delta
            .cmp(&left.bytes_delta)
            .then_with(|| left.path.cmp(&right.path))
    });
    growers.truncate(5);
    Some(GrowthSummary {
        previous_at_ms: previous.captured_at_ms,
        total_delta: current.total_bytes - previous.total_bytes,
        growers,
    })
}

fn push_u32(out: &mut Vec<u8>, value: usize) -> Result<(), String> {
    let value = u32::try_from(value).map_err(|_| "snapshot value is too large")?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn push_path(out: &mut Vec<u8>, path: &std::path::Path) -> Result<(), String> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() > MAX_PATH_BYTES {
        return Err("snapshot path is too long".into());
    }
    push_u32(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, String> {
    if snapshot.entries.len() > MAX_ENTRIES {
        return Err("snapshot has too many entries".into());
    }
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&snapshot.captured_at_ms.to_le_bytes());
    out.extend_from_slice(&snapshot.total_bytes.to_le_bytes());
    push_path(&mut out, &snapshot.root)?;
    push_u32(&mut out, snapshot.entries.len())?;
    for entry in &snapshot.entries {
        push_path(&mut out, &entry.path)?;
        out.extend_from_slice(&entry.bytes.to_le_bytes());
        out.extend_from_slice(&entry.files.to_le_bytes());
        out.push(u8::from(entry.is_dir));
    }
    Ok(out)
}

fn read_exact<const N: usize>(cursor: &mut Cursor<&[u8]>) -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    cursor
        .read_exact(&mut bytes)
        .map_err(|_| "snapshot is truncated".to_string())?;
    Ok(bytes)
}

fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<usize, String> {
    Ok(u32::from_le_bytes(read_exact(cursor)?) as usize)
}

fn read_i64(cursor: &mut Cursor<&[u8]>) -> Result<i64, String> {
    Ok(i64::from_le_bytes(read_exact(cursor)?))
}

fn read_path(cursor: &mut Cursor<&[u8]>) -> Result<PathBuf, String> {
    let len = read_u32(cursor)?;
    if len > MAX_PATH_BYTES {
        return Err("snapshot path is too long".into());
    }
    let mut bytes = vec![0u8; len];
    cursor
        .read_exact(&mut bytes)
        .map_err(|_| "snapshot is truncated".to_string())?;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn decode(bytes: &[u8]) -> Result<Snapshot, String> {
    let mut cursor = Cursor::new(bytes);
    if &read_exact::<8>(&mut cursor)? != MAGIC {
        return Err("snapshot format is not supported".into());
    }
    let captured_at_ms = read_i64(&mut cursor)?;
    let total_bytes = read_i64(&mut cursor)?;
    let root = read_path(&mut cursor)?;
    let count = read_u32(&mut cursor)?;
    if count > MAX_ENTRIES {
        return Err("snapshot has too many entries".into());
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let path = read_path(&mut cursor)?;
        let bytes = read_i64(&mut cursor)?;
        let files = read_i64(&mut cursor)?;
        let flag = read_exact::<1>(&mut cursor)?[0];
        let is_dir = match flag {
            0 => false,
            1 => true,
            _ => return Err("snapshot contains an invalid entry flag".into()),
        };
        entries.push(Entry {
            path,
            bytes,
            files,
            is_dir,
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err("snapshot contains trailing data".into());
    }
    Ok(Snapshot {
        captured_at_ms,
        root,
        total_bytes,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    const MB: i64 = 1 << 20;

    fn entry(path: &str, bytes: i64) -> Entry {
        Entry {
            path: PathBuf::from(path),
            bytes,
            files: 1,
            is_dir: true,
        }
    }

    #[test]
    fn compare_orders_large_positive_growers_and_keeps_total_change() {
        let previous = Snapshot {
            captured_at_ms: 100,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes: 200 * MB,
            entries: vec![
                entry("a", 50 * MB),
                entry("gone", 100 * MB),
                entry("small", 10 * MB),
            ],
        };
        let current = Snapshot {
            captured_at_ms: 200,
            root: previous.root.clone(),
            total_bytes: 159 * MB,
            entries: vec![
                entry("a", 70 * MB),
                entry("new", 30 * MB),
                entry("small", 19 * MB),
            ],
        };

        let summary = compare(&previous, &current).unwrap();
        assert_eq!(summary.previous_at_ms, 100);
        assert_eq!(summary.total_delta, -41 * MB);
        assert_eq!(summary.growers.len(), 2);
        assert_eq!(summary.growers[0].path, PathBuf::from("new"));
        assert_eq!(summary.growers[0].bytes_delta, 30 * MB);
        assert_eq!(summary.growers[1].path, PathBuf::from("a"));
        assert_eq!(summary.growers[1].bytes_delta, 20 * MB);
    }

    #[test]
    fn compare_rejects_incompatible_roots() {
        let previous = Snapshot {
            captured_at_ms: 1,
            root: PathBuf::from("/one"),
            total_bytes: 1,
            entries: vec![],
        };
        let current = Snapshot {
            captured_at_ms: 2,
            root: PathBuf::from("/two"),
            total_bytes: 2,
            entries: vec![],
        };
        assert_eq!(compare(&previous, &current), None);
    }

    #[test]
    fn codec_round_trips_raw_path_bytes() {
        let raw = std::ffi::OsString::from_vec(vec![b'f', b'o', 0xff]);
        let snapshot = Snapshot {
            captured_at_ms: 123,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes: 456,
            entries: vec![Entry {
                path: PathBuf::from(raw),
                bytes: 789,
                files: 3,
                is_dir: false,
            }],
        };
        let decoded = decode(&encode(&snapshot).unwrap()).unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(
            decoded.entries[0].path.as_os_str().as_bytes(),
            &[b'f', b'o', 0xff]
        );
    }

    #[test]
    fn codec_rejects_wrong_truncated_and_trailing_payloads() {
        let snapshot = Snapshot {
            captured_at_ms: 1,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes: 2,
            entries: vec![entry("Users", 3)],
        };
        let bytes = encode(&snapshot).unwrap();
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        assert!(decode(&wrong).is_err());
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
    }
}
