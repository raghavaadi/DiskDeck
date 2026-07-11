//! Optional native menu-bar free-space monitor and local settings.

use objc2::rc::Retained;
use objc2_app_kit::{NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::NSString;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"DDMON1\0\0";
const DEFAULT_THRESHOLD_GB: u64 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorSettings {
    pub enabled: bool,
    pub launch_at_login: bool,
    pub threshold_gb: u64,
}

impl Default for MonitorSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            launch_at_login: false,
            threshold_gb: DEFAULT_THRESHOLD_GB,
        }
    }
}

pub fn settings_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/DiskDeck/monitor.ddmon")
}

fn encode(settings: MonitorSettings) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(18);
    bytes.extend_from_slice(MAGIC);
    bytes.push(u8::from(settings.enabled));
    bytes.push(u8::from(settings.launch_at_login));
    bytes.extend_from_slice(&settings.threshold_gb.to_le_bytes());
    bytes
}

fn decode(bytes: &[u8]) -> Result<MonitorSettings, String> {
    if bytes.len() != 18 || &bytes[..8] != MAGIC {
        return Err("menu monitor settings format is not supported".into());
    }
    if bytes[8] > 1 || bytes[9] > 1 {
        return Err("menu monitor settings contain invalid flags".into());
    }
    let threshold_gb = u64::from_le_bytes(bytes[10..18].try_into().unwrap());
    if !(5..=100).contains(&threshold_gb) {
        return Err("menu monitor threshold is outside 5–100 GB".into());
    }
    Ok(MonitorSettings {
        enabled: bytes[8] == 1,
        launch_at_login: bytes[9] == 1,
        threshold_gb,
    })
}

pub fn load(path: &Path) -> Result<MonitorSettings, String> {
    match std::fs::read(path) {
        Ok(bytes) => decode(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(MonitorSettings::default())
        }
        Err(error) => Err(format!("read menu monitor settings: {error}")),
    }
}

pub fn save(path: &Path, settings: MonitorSettings) -> Result<(), String> {
    if path.exists() {
        load(path)?;
    }
    let parent = path
        .parent()
        .ok_or("menu monitor settings have no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("create monitor settings: {error}"))?;
    let pid = std::process::id();
    let mut reserved = None;
    for attempt in 0..32u32 {
        let temp = parent.join(format!(".monitor.{pid}.{attempt}.tmp"));
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
        {
            Ok(file) => {
                reserved = Some((temp, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create monitor update: {error}")),
        }
    }
    let (temp, mut file) = reserved.ok_or("reserve monitor settings update")?;
    let result = file
        .write_all(&encode(settings))
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temp, path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write monitor settings: {error}"));
    }
    Ok(())
}

pub fn is_low(free_bytes: i64, threshold_gb: u64) -> bool {
    free_bytes >= 0 && free_bytes < (threshold_gb as i64) * 1_000_000_000
}

pub struct MenuBarItem {
    bar: Retained<NSStatusBar>,
    item: Retained<NSStatusItem>,
}

impl MenuBarItem {
    pub fn new() -> Self {
        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);
        Self { bar, item }
    }

    #[allow(deprecated)]
    pub fn update(&self, free_bytes: i64, low: bool) {
        let gb = free_bytes.max(0) as f64 / 1_000_000_000.0;
        let title = if low {
            format!("⚠ {gb:.1} GB")
        } else {
            format!("▱ {gb:.1} GB")
        };
        self.item.setTitle(Some(&NSString::from_str(&title)));
    }
}

impl Drop for MenuBarItem {
    fn drop(&mut self) {
        self.bar.removeStatusItem(&self.item);
    }
}

pub fn set_launch_at_login(home: &Path, enabled: bool) -> Result<(), String> {
    let path = home.join("Library/LaunchAgents/com.buddyhq.diskdeck.plist");
    if !enabled {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("disable launch at login: {error}")),
        };
    }
    let parent = path.parent().ok_or("LaunchAgent path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("create LaunchAgents: {error}"))?;
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.buddyhq.diskdeck</string>
<key>ProgramArguments</key><array><string>/usr/bin/open</string><string>-a</string><string>/Applications/DiskDeck.app</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>
"#;
    std::fs::write(path, plist).map_err(|error| format!("enable launch at login: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_corruption_fails_closed() {
        let value = MonitorSettings {
            enabled: true,
            launch_at_login: false,
            threshold_gb: 25,
        };
        assert_eq!(decode(&encode(value)).unwrap(), value);
        assert!(decode(b"broken").is_err());
    }

    #[test]
    fn low_space_threshold_uses_decimal_gigabytes() {
        assert!(is_low(9_999_999_999, 10));
        assert!(!is_low(10_000_000_000, 10));
        assert!(!is_low(-1, 10));
    }

    #[test]
    fn launch_agent_is_explicit_and_user_owned() {
        let tmp = tempfile::tempdir().unwrap();
        set_launch_at_login(tmp.path(), true).unwrap();
        let path = tmp
            .path()
            .join("Library/LaunchAgents/com.buddyhq.diskdeck.plist");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("/Applications/DiskDeck.app"));
        set_launch_at_login(tmp.path(), false).unwrap();
        assert!(!path.exists());
    }
}
