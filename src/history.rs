use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Cursor, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{mpsc::Sender, Arc};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::scan::Node;

const MAGIC_V1: &[u8; 8] = b"DDHIST1\0";
const MAGIC_V2: &[u8; 8] = b"DDHIST2\0";
const MIN_GROWTH_BYTES: i64 = 10 << 20;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_PATH_BYTES: usize = 1 << 20;
const WATCH_MAGIC: &[u8; 8] = b"DDWATCH1";
const MAX_WATCHED: usize = 128;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelinePoint {
    pub captured_at_ms: i64,
    pub total_bytes: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderPoint {
    pub captured_at_ms: i64,
    pub bytes: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderSeries {
    pub path: PathBuf,
    pub points: Vec<FolderPoint>,
    pub bytes_delta: i64,
    pub percent_tenths: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecurringGrowth {
    pub path: PathBuf,
    pub positive_intervals: usize,
    pub bytes_delta: i64,
    pub current_bytes: i64,
    pub percent_tenths: Option<i64>,
    pub watched: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrowthWatch {
    pub timeline: Vec<TimelinePoint>,
    pub capacity: Vec<crate::forecast::CapacityPoint>,
    pub recurring: Vec<RecurringGrowth>,
    pub watched: Vec<FolderSeries>,
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
    volume_total_bytes: Option<i64>,
    volume_free_bytes: Option<i64>,
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
    out.extend_from_slice(MAGIC_V2);
    out.extend_from_slice(&snapshot.captured_at_ms.to_le_bytes());
    out.extend_from_slice(&snapshot.total_bytes.to_le_bytes());
    out.extend_from_slice(&snapshot.volume_total_bytes.unwrap_or(-1).to_le_bytes());
    out.extend_from_slice(&snapshot.volume_free_bytes.unwrap_or(-1).to_le_bytes());
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
    let magic = read_exact::<8>(&mut cursor)?;
    let version = if &magic == MAGIC_V2 {
        2
    } else if &magic == MAGIC_V1 {
        1
    } else {
        return Err("snapshot format is not supported".into());
    };
    let captured_at_ms = read_i64(&mut cursor)?;
    let total_bytes = read_i64(&mut cursor)?;
    let (volume_total_bytes, volume_free_bytes) = if version == 2 {
        let total = read_i64(&mut cursor)?;
        let free = read_i64(&mut cursor)?;
        ((total >= 0).then_some(total), (free >= 0).then_some(free))
    } else {
        (None, None)
    };
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
        volume_total_bytes,
        volume_free_bytes,
        entries,
    })
}

fn capture(root: &Arc<Node>, captured_at_ms: i64, capacity: Option<(i64, i64)>) -> Snapshot {
    fn walk(root_path: &Path, node: &Arc<Node>, entries: &mut Vec<Entry>) {
        for child in node.kids() {
            let Ok(path) = child.path.strip_prefix(root_path) else {
                continue;
            };
            entries.push(Entry {
                path: path.to_path_buf(),
                bytes: child.bytes(),
                files: child.files(),
                is_dir: child.is_dir,
            });
            if child.is_dir {
                walk(root_path, &child, entries);
            }
        }
    }

    let mut entries = Vec::new();
    walk(&root.path, root, &mut entries);
    Snapshot {
        captured_at_ms,
        root: root.path.clone(),
        total_bytes: root.bytes(),
        volume_total_bytes: capacity.map(|value| value.0),
        volume_free_bytes: capacity.map(|value| value.1),
        entries,
    }
}

fn snapshot_key(path: &Path) -> Option<(i64, u32)> {
    let name = path.file_name()?.to_str()?;
    let middle = name.strip_prefix("snapshot-")?.strip_suffix(".ddhist")?;
    let (captured, process) = middle.rsplit_once('-')?;
    if captured.is_empty()
        || process.is_empty()
        || !captured.bytes().all(|byte| byte.is_ascii_digit())
        || !process.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((captured.parse().ok()?, process.parse().ok()?))
}

fn matching_snapshot_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|error| format!("read history: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read history entry: {error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("inspect history entry: {error}"))?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        if snapshot_key(&path).is_some() {
            paths.push(path);
        }
    }
    paths.sort_by_key(|path| std::cmp::Reverse(snapshot_key(path).unwrap()));
    Ok(paths)
}

fn load_latest_compatible(dir: &Path, root: &Path) -> Result<Option<Snapshot>, String> {
    for path in matching_snapshot_paths(dir)? {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(snapshot) = decode(&bytes) else {
            continue;
        };
        if snapshot.root == root {
            return Ok(Some(snapshot));
        }
    }
    Ok(None)
}

fn normalized_relative_path(path: &Path) -> bool {
    let mut saw_component = false;
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => saw_component = true,
            _ => return false,
        }
    }
    saw_component && path.as_os_str().as_bytes().len() <= MAX_PATH_BYTES
}

fn watchlist_path(dir: &Path) -> PathBuf {
    dir.join("watchlist.ddwatch")
}

fn encode_watchlist(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    if paths.len() > MAX_WATCHED {
        return Err("Growth Watch has too many watched folders".into());
    }
    let mut out = Vec::new();
    out.extend_from_slice(WATCH_MAGIC);
    push_u32(&mut out, paths.len())?;
    for path in paths {
        if !normalized_relative_path(path) {
            return Err("Growth Watch paths must be normalized scan-relative folders".into());
        }
        push_path(&mut out, path)?;
    }
    Ok(out)
}

fn decode_watchlist(bytes: &[u8]) -> Result<Vec<PathBuf>, String> {
    let mut cursor = Cursor::new(bytes);
    if &read_exact::<8>(&mut cursor)? != WATCH_MAGIC {
        return Err("Growth Watch format is not supported".into());
    }
    let count = read_u32(&mut cursor)?;
    if count > MAX_WATCHED {
        return Err("Growth Watch has too many watched folders".into());
    }
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let path = read_path(&mut cursor)?;
        if !normalized_relative_path(&path) {
            return Err("Growth Watch contains an unsafe path".into());
        }
        paths.push(path);
    }
    if cursor.position() != bytes.len() as u64 {
        return Err("Growth Watch contains trailing data".into());
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn load_watched_paths(path: &Path) -> Result<Vec<PathBuf>, String> {
    match std::fs::read(path) {
        Ok(bytes) => decode_watchlist(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("read Growth Watch: {error}")),
    }
}

fn set_watched_paths(path: &Path, paths: &[PathBuf]) -> Result<(), String> {
    if path.exists() {
        load_watched_paths(path)?;
    }
    let mut normalized = paths.to_vec();
    normalized.sort();
    normalized.dedup();
    let bytes = encode_watchlist(&normalized)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Growth Watch path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("create Growth Watch: {error}"))?;
    let pid = std::process::id();
    let mut temp = None;
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(".watchlist.{pid}.{attempt}.tmp"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temp = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create Growth Watch update: {error}")),
        }
    }
    let (temp_path, mut file) = temp.ok_or("reserve Growth Watch update")?;
    let result = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temp_path, path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("write Growth Watch: {error}"));
    }
    Ok(())
}

fn percentage_tenths(before: i64, delta: i64) -> Option<i64> {
    (before > 0).then(|| {
        ((delta as i128 * 1_000) / before as i128).clamp(i64::MIN as i128, i64::MAX as i128) as i64
    })
}

fn build_growth_watch(snapshots: &[Snapshot], watched_paths: &[PathBuf]) -> GrowthWatch {
    let timeline = snapshots
        .iter()
        .map(|snapshot| TimelinePoint {
            captured_at_ms: snapshot.captured_at_ms,
            total_bytes: snapshot.total_bytes,
        })
        .collect();
    let capacity = snapshots
        .iter()
        .filter_map(|snapshot| {
            Some(crate::forecast::CapacityPoint {
                captured_at_ms: snapshot.captured_at_ms,
                total_bytes: snapshot.volume_total_bytes?,
                free_bytes: snapshot.volume_free_bytes?,
            })
        })
        .collect();

    #[derive(Clone, Copy)]
    struct Aggregate {
        positive_intervals: usize,
        first_before: i64,
    }
    let mut aggregates: HashMap<PathBuf, Aggregate> = HashMap::new();
    for pair in snapshots.windows(2) {
        let previous: HashMap<&Path, i64> = pair[0]
            .entries
            .iter()
            .filter(|entry| entry.is_dir)
            .map(|entry| (entry.path.as_path(), entry.bytes))
            .collect();
        for entry in pair[1].entries.iter().filter(|entry| entry.is_dir) {
            let before = previous.get(entry.path.as_path()).copied().unwrap_or(0);
            let delta = entry.bytes.saturating_sub(before);
            if delta < MIN_GROWTH_BYTES {
                continue;
            }
            aggregates
                .entry(entry.path.clone())
                .and_modify(|aggregate| {
                    aggregate.positive_intervals += 1;
                })
                .or_insert(Aggregate {
                    positive_intervals: 1,
                    first_before: before,
                });
        }
    }
    let latest: HashMap<&Path, i64> = snapshots
        .last()
        .map(|snapshot| {
            snapshot
                .entries
                .iter()
                .filter(|entry| entry.is_dir)
                .map(|entry| (entry.path.as_path(), entry.bytes))
                .collect()
        })
        .unwrap_or_default();
    let mut recurring: Vec<RecurringGrowth> = aggregates
        .into_iter()
        .map(|(path, aggregate)| {
            let current_bytes = latest.get(path.as_path()).copied().unwrap_or(0);
            let bytes_delta = current_bytes.saturating_sub(aggregate.first_before);
            RecurringGrowth {
                watched: watched_paths.contains(&path),
                path,
                positive_intervals: aggregate.positive_intervals,
                bytes_delta,
                current_bytes,
                percent_tenths: percentage_tenths(aggregate.first_before, bytes_delta),
            }
        })
        .collect();
    recurring.retain(|growth| growth.bytes_delta > 0 && growth.positive_intervals >= 2);
    let candidates = recurring.clone();
    recurring.retain(|candidate| {
        !candidates.iter().any(|other| {
            other.path != candidate.path
                && other.path.starts_with(&candidate.path)
                && other.positive_intervals >= candidate.positive_intervals
                && (other.bytes_delta as i128 * 100) >= (candidate.bytes_delta as i128 * 80)
        })
    });
    recurring.sort_by(|left, right| {
        right
            .positive_intervals
            .cmp(&left.positive_intervals)
            .then_with(|| right.bytes_delta.cmp(&left.bytes_delta))
            .then_with(|| left.path.cmp(&right.path))
    });
    recurring.truncate(12);

    let watched = watched_paths
        .iter()
        .map(|path| {
            let points: Vec<FolderPoint> = snapshots
                .iter()
                .map(|snapshot| FolderPoint {
                    captured_at_ms: snapshot.captured_at_ms,
                    bytes: snapshot
                        .entries
                        .iter()
                        .find(|entry| entry.is_dir && entry.path == *path)
                        .map(|entry| entry.bytes)
                        .unwrap_or(0),
                })
                .collect();
            let before = points.first().map(|point| point.bytes).unwrap_or(0);
            let current = points.last().map(|point| point.bytes).unwrap_or(0);
            let bytes_delta = current.saturating_sub(before);
            FolderSeries {
                path: path.clone(),
                points,
                bytes_delta,
                percent_tenths: percentage_tenths(before, bytes_delta),
            }
        })
        .collect();
    GrowthWatch {
        timeline,
        capacity,
        recurring,
        watched,
    }
}

pub fn load_growth_watch(dir: &Path, root: &Path) -> Result<GrowthWatch, String> {
    if !dir.exists() {
        return Ok(GrowthWatch::default());
    }
    let watched = load_watched_paths(&watchlist_path(dir))?;
    let mut snapshots = Vec::new();
    for path in matching_snapshot_paths(dir)? {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(snapshot) = decode(&bytes) else {
            continue;
        };
        if snapshot.root == root {
            snapshots.push(snapshot);
        }
    }
    snapshots.sort_by_key(|snapshot| snapshot.captured_at_ms);
    Ok(build_growth_watch(&snapshots, &watched))
}

pub fn set_folder_watched(dir: &Path, folder: &Path, watched: bool) -> Result<(), String> {
    if !normalized_relative_path(folder) {
        return Err("Growth Watch paths must be normalized scan-relative folders".into());
    }
    let path = watchlist_path(dir);
    let mut paths = load_watched_paths(&path)?;
    paths.retain(|existing| existing != folder);
    if watched {
        paths.push(folder.to_path_buf());
    }
    set_watched_paths(&path, &paths)
}

fn record(dir: &Path, current: &Snapshot) -> Result<Option<GrowthSummary>, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("create history: {error}"))?;
    let previous = load_latest_compatible(dir, &current.root)?;
    let bytes = encode(current)?;
    let process = std::process::id();
    let filename = format!(
        "snapshot-{:020}-{process}.ddhist",
        current.captured_at_ms.max(0)
    );
    let final_path = dir.join(&filename);
    let temp_path = dir.join(format!(".{filename}.tmp"));
    let mut file = std::fs::File::create(&temp_path)
        .map_err(|error| format!("create history snapshot: {error}"))?;
    if let Err(error) = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temp_path, &final_path))
    {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("write history snapshot: {error}"));
    }

    let paths = matching_snapshot_paths(dir)?;
    for old in paths.into_iter().skip(12) {
        std::fs::remove_file(&old).map_err(|error| format!("prune history snapshot: {error}"))?;
    }
    Ok(previous.and_then(|previous| compare(&previous, current)))
}

pub fn default_history_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("DiskDeck")
            .join("History")
    })
}

pub fn record_scan(root: Arc<Node>, dir: PathBuf, tx: Sender<HistoryEvent>) -> Result<(), String> {
    std::thread::Builder::new()
        .name("scan-history".into())
        .spawn(move || {
            let captured_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or(0);
            let stats = crate::scan::disk_stats();
            let capacity = (stats.total > 0 && stats.free >= 0 && stats.free <= stats.total)
                .then_some((stats.total, stats.free));
            let current = capture(&root, captured_at_ms, capacity);
            let event = match record(&dir, &current) {
                Ok(Some(summary)) => HistoryEvent::Compared(summary),
                Ok(None) => HistoryEvent::BaselineSaved,
                Err(error) => HistoryEvent::Failed(error),
            };
            let _ = tx.send(event);
        })
        .map(|_| ())
        .map_err(|error| format!("start scan history: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{start_scan, ScanState};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::time::{Duration, Instant};

    const MB: i64 = 1 << 20;

    fn entry(path: &str, bytes: i64) -> Entry {
        Entry {
            path: PathBuf::from(path),
            bytes,
            files: 1,
            is_dir: true,
        }
    }

    fn snapshot_with_capacity(
        captured_at_ms: i64,
        total_bytes: i64,
        capacity: Option<(i64, i64)>,
    ) -> Snapshot {
        Snapshot {
            captured_at_ms,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes,
            volume_total_bytes: capacity.map(|value| value.0),
            volume_free_bytes: capacity.map(|value| value.1),
            entries: vec![],
        }
    }

    fn encode_v1_for_test(snapshot: &Snapshot) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC_V1);
        out.extend_from_slice(&snapshot.captured_at_ms.to_le_bytes());
        out.extend_from_slice(&snapshot.total_bytes.to_le_bytes());
        push_path(&mut out, &snapshot.root).unwrap();
        push_u32(&mut out, snapshot.entries.len()).unwrap();
        for entry in &snapshot.entries {
            push_path(&mut out, &entry.path).unwrap();
            out.extend_from_slice(&entry.bytes.to_le_bytes());
            out.extend_from_slice(&entry.files.to_le_bytes());
            out.push(u8::from(entry.is_dir));
        }
        out
    }

    #[test]
    fn v2_codec_round_trips_capacity_observation() {
        let snapshot = Snapshot {
            captured_at_ms: 123,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes: 456,
            volume_total_bytes: Some(250_000_000_000),
            volume_free_bytes: Some(80_000_000_000),
            entries: vec![entry("Users", 789)],
        };
        assert_eq!(decode(&encode(&snapshot).unwrap()).unwrap(), snapshot);
    }

    #[test]
    fn v1_codec_remains_readable_without_capacity_evidence() {
        let bytes = encode_v1_for_test(&Snapshot {
            captured_at_ms: 123,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes: 456,
            volume_total_bytes: None,
            volume_free_bytes: None,
            entries: vec![],
        });
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.volume_total_bytes, None);
        assert_eq!(decoded.volume_free_bytes, None);
    }

    #[test]
    fn growth_watch_exposes_only_complete_capacity_pairs() {
        let snapshots = vec![
            snapshot_with_capacity(10, 100, Some((250, 80))),
            snapshot_with_capacity(20, 110, None),
            snapshot_with_capacity(30, 120, Some((250, 70))),
        ];
        let watch = build_growth_watch(&snapshots, &[]);
        assert_eq!(watch.capacity.len(), 2);
        assert_eq!(watch.capacity[0].free_bytes, 80);
        assert_eq!(watch.capacity[1].free_bytes, 70);
    }

    #[test]
    fn compare_orders_large_positive_growers_and_keeps_total_change() {
        let previous = Snapshot {
            captured_at_ms: 100,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes: 200 * MB,
            volume_total_bytes: None,
            volume_free_bytes: None,
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
            volume_total_bytes: None,
            volume_free_bytes: None,
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
            volume_total_bytes: None,
            volume_free_bytes: None,
            entries: vec![],
        };
        let current = Snapshot {
            captured_at_ms: 2,
            root: PathBuf::from("/two"),
            total_bytes: 2,
            volume_total_bytes: None,
            volume_free_bytes: None,
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
            volume_total_bytes: None,
            volume_free_bytes: None,
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
            volume_total_bytes: None,
            volume_free_bytes: None,
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

    #[test]
    fn storage_capture_uses_compacted_relative_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let big = tmp.path().join("Big/data.bin");
        std::fs::create_dir_all(big.parent().unwrap()).unwrap();
        std::fs::write(&big, vec![1u8; 11 << 20]).unwrap();
        let scan = start_scan(tmp.path().to_path_buf());
        let deadline = Instant::now() + Duration::from_secs(5);
        while scan.state() == ScanState::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(scan.state(), ScanState::Done);
        let snapshot = capture(&scan.root, 123, None);
        assert_eq!(snapshot.captured_at_ms, 123);
        assert_eq!(snapshot.root, tmp.path());
        assert_eq!(snapshot.total_bytes, scan.root.bytes());
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.path == PathBuf::from("Big")));
        assert!(snapshot
            .entries
            .iter()
            .all(|entry| !entry.path.is_absolute()));
    }

    #[test]
    fn storage_record_retains_twelve_matching_files_and_unrelated_contents() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("keep.txt"), b"keep").unwrap();
        for captured_at_ms in 1..=14 {
            let snapshot = Snapshot {
                captured_at_ms,
                root: PathBuf::from("/System/Volumes/Data"),
                total_bytes: captured_at_ms,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![],
            };
            record(tmp.path(), &snapshot).unwrap();
        }
        assert_eq!(matching_snapshot_paths(tmp.path()).unwrap().len(), 12);
        assert_eq!(std::fs::read(tmp.path().join("keep.txt")).unwrap(), b"keep");
    }

    #[test]
    fn storage_record_skips_a_corrupt_newest_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let previous = Snapshot {
            captured_at_ms: 10,
            root: PathBuf::from("/System/Volumes/Data"),
            total_bytes: 100,
            volume_total_bytes: None,
            volume_free_bytes: None,
            entries: vec![],
        };
        record(tmp.path(), &previous).unwrap();
        std::fs::write(tmp.path().join("snapshot-999-1.ddhist"), b"broken").unwrap();
        let current = Snapshot {
            captured_at_ms: 1_000,
            root: previous.root.clone(),
            total_bytes: 125,
            volume_total_bytes: None,
            volume_free_bytes: None,
            entries: vec![],
        };
        let summary = record(tmp.path(), &current).unwrap().unwrap();
        assert_eq!(summary.previous_at_ms, 10);
        assert_eq!(summary.total_delta, 25);
    }

    #[test]
    fn growth_watch_orders_timeline_and_ranks_recurring_growth() {
        let snapshots = vec![
            Snapshot {
                captured_at_ms: 10,
                root: PathBuf::from("/System/Volumes/Data"),
                total_bytes: 100 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![entry("Users/project", 20 * MB), entry("Users/new", 0)],
            },
            Snapshot {
                captured_at_ms: 20,
                root: PathBuf::from("/System/Volumes/Data"),
                total_bytes: 140 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![entry("Users/project", 35 * MB), entry("Users/new", 25 * MB)],
            },
            Snapshot {
                captured_at_ms: 30,
                root: PathBuf::from("/System/Volumes/Data"),
                total_bytes: 180 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![entry("Users/project", 50 * MB), entry("Users/new", 30 * MB)],
            },
        ];

        let watch = build_growth_watch(&snapshots, &[PathBuf::from("Users/project")]);
        assert_eq!(
            watch
                .timeline
                .iter()
                .map(|p| p.captured_at_ms)
                .collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
        assert_eq!(watch.recurring[0].path, PathBuf::from("Users/project"));
        assert_eq!(watch.recurring[0].positive_intervals, 2);
        assert_eq!(watch.recurring[0].bytes_delta, 30 * MB);
        assert_eq!(watch.recurring[0].percent_tenths, Some(1500));
        assert!(watch.recurring[0].watched);
        assert_eq!(watch.watched[0].points.len(), 3);
    }

    #[test]
    fn recurring_growth_reports_net_change_after_a_later_shrink() {
        let root = PathBuf::from("/System/Volumes/Data");
        let snapshots = vec![
            Snapshot {
                captured_at_ms: 10,
                root: root.clone(),
                total_bytes: 20 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![entry("Users/project", 20 * MB)],
            },
            Snapshot {
                captured_at_ms: 20,
                root: root.clone(),
                total_bytes: 40 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![entry("Users/project", 40 * MB)],
            },
            Snapshot {
                captured_at_ms: 30,
                root: root.clone(),
                total_bytes: 60 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![entry("Users/project", 60 * MB)],
            },
            Snapshot {
                captured_at_ms: 40,
                root,
                total_bytes: 35 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![entry("Users/project", 35 * MB)],
            },
        ];
        let watch = build_growth_watch(&snapshots, &[]);
        assert_eq!(watch.recurring[0].positive_intervals, 2);
        assert_eq!(watch.recurring[0].bytes_delta, 15 * MB);
        assert_eq!(watch.recurring[0].percent_tenths, Some(750));
    }

    #[test]
    fn recurring_growth_omits_net_negative_and_correlated_ancestor_noise() {
        let root = PathBuf::from("/System/Volumes/Data");
        let snapshots = vec![
            Snapshot {
                captured_at_ms: 10,
                root: root.clone(),
                total_bytes: 100 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![
                    entry("Users", 50 * MB),
                    entry("Users/project", 45 * MB),
                    entry("private", 40 * MB),
                ],
            },
            Snapshot {
                captured_at_ms: 20,
                root: root.clone(),
                total_bytes: 130 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![
                    entry("Users", 80 * MB),
                    entry("Users/project", 73 * MB),
                    entry("private", 20 * MB),
                ],
            },
            Snapshot {
                captured_at_ms: 30,
                root,
                total_bytes: 160 * MB,
                volume_total_bytes: None,
                volume_free_bytes: None,
                entries: vec![
                    entry("Users", 110 * MB),
                    entry("Users/project", 101 * MB),
                    entry("private", 10 * MB),
                ],
            },
        ];
        let watch = build_growth_watch(&snapshots, &[]);
        assert_eq!(watch.recurring.len(), 1);
        assert_eq!(watch.recurring[0].path, PathBuf::from("Users/project"));
        assert_eq!(watch.recurring[0].bytes_delta, 56 * MB);
    }

    #[test]
    fn watchlist_round_trips_raw_paths_and_refuses_corrupt_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("watchlist.ddwatch");
        let raw = PathBuf::from(OsString::from_vec(b"Users/project-\xff".to_vec()));
        set_watched_paths(&path, &[raw.clone()]).unwrap();
        assert_eq!(load_watched_paths(&path).unwrap(), vec![raw]);

        std::fs::write(&path, b"broken").unwrap();
        assert!(set_watched_paths(&path, &[PathBuf::from("Users/other")]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
    }

    #[test]
    fn watchlist_rejects_absolute_and_parent_traversal_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("watchlist.ddwatch");
        assert!(set_watched_paths(&path, &[PathBuf::from("/Users/project")]).is_err());
        assert!(set_watched_paths(&path, &[PathBuf::from("Users/../private")]).is_err());
        assert!(!path.exists());
    }
}
