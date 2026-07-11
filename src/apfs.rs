//! Read-only APFS accounting from fixed macOS system commands.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApfsAccounting {
    pub container_size: i64,
    pub container_free: i64,
    pub snapshot_count: Option<usize>,
    pub snapshot_bytes: Option<i64>,
    pub purgeable_bytes: Option<i64>,
}

fn plist_integer(xml: &str, key: &str) -> Option<i64> {
    let key = format!("<key>{key}</key>");
    let tail = xml.split_once(&key)?.1;
    let start = tail.find("<integer>")? + "<integer>".len();
    let end = tail[start..].find("</integer>")? + start;
    tail[start..end].trim().parse().ok()
}

fn snapshot_count(xml: &str) -> Option<usize> {
    let tail = xml.split_once("<key>Snapshots</key>")?.1;
    if tail.trim_start().starts_with("<array/>") {
        return Some(0);
    }
    let start = tail.find("<array>")? + "<array>".len();
    let end = tail[start..].find("</array>")? + start;
    Some(tail[start..end].matches("<dict>").count())
}

fn fixed_command(args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("/usr/sbin/diskutil")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start diskutil: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("diskutil timed out".into());
            }
            Err(error) => return Err(format!("wait for diskutil: {error}")),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("read diskutil output: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("diskutil failed: {}", error.trim()));
    }
    String::from_utf8(output.stdout).map_err(|_| "diskutil returned non-UTF-8 plist".into())
}

pub fn load() -> Result<ApfsAccounting, String> {
    let info = fixed_command(&["info", "-plist", "/System/Volumes/Data"])?;
    let container_size = plist_integer(&info, "APFSContainerSize")
        .filter(|value| *value > 0)
        .ok_or("APFS container size is unavailable")?;
    let container_free = plist_integer(&info, "APFSContainerFree")
        .filter(|value| *value >= 0 && *value <= container_size)
        .ok_or("APFS container free space is unavailable")?;
    let snapshots = fixed_command(&["apfs", "listSnapshots", "/System/Volumes/Data", "-plist"])
        .ok()
        .and_then(|xml| snapshot_count(&xml));
    Ok(ApfsAccounting {
        container_size,
        container_free,
        snapshot_count: snapshots,
        snapshot_bytes: None,
        purgeable_bytes: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_container_values() {
        let xml = "<dict><key>APFSContainerSize</key><integer>245000</integer><key>APFSContainerFree</key><integer>82000</integer></dict>";
        assert_eq!(plist_integer(xml, "APFSContainerSize"), Some(245000));
        assert_eq!(plist_integer(xml, "APFSContainerFree"), Some(82000));
        assert_eq!(plist_integer(xml, "Missing"), None);
    }

    #[test]
    fn counts_only_snapshot_dicts_inside_the_snapshot_array() {
        let xml = "<dict><key>Other</key><dict></dict><key>Snapshots</key><array><dict></dict><dict></dict></array><key>Tail</key><dict></dict></dict>";
        assert_eq!(snapshot_count(xml), Some(2));
        assert_eq!(
            snapshot_count("<dict><key>Snapshots</key><array/></dict>"),
            Some(0)
        );
        assert_eq!(snapshot_count("<dict></dict>"), None);
    }
}
