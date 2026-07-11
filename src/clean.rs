//! Cleanup executors. Same hard-won semantics as the Wails edition:
//! trash = same-volume rename into ~/.Trash FIRST (instant, no Automation
//! permission — Finder-osascript hangs silently without the TCC grant and is
//! fallback only), delete clears write-protected dirs, commands run in a
//! login shell and are only ever the vetted strings stored on a Rec.

use crate::rules::{strip_data_root, Action, Rec};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Move a file/dir to the Trash. Rename first; Finder as fallback.
pub fn trash_path(p: &Path) -> Result<(), String> {
    if let Some(home) = std::env::var_os("HOME") {
        let trash = PathBuf::from(home).join(".Trash");
        if let Some(name) = p.file_name() {
            let mut target = trash.join(name);
            if target.symlink_metadata().is_ok() {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                target = trash.join(format!("{} {ts}", name.to_string_lossy()));
            }
            if fs::rename(p, &target).is_ok() {
                return Ok(());
            }
        }
    }
    // fallback: Finder (requires the Automation permission; may prompt once)
    let script = r#"on run argv
	tell application "Finder" to delete (POSIX file (item 1 of argv) as alias)
end run"#;
    let out = Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .arg(p)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "finder: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Permanently remove, clearing read-only bits if needed (go-modcache style).
pub fn delete_path(p: &Path) -> Result<(), String> {
    let try_remove = |p: &Path| -> std::io::Result<()> {
        let meta = p.symlink_metadata()?;
        if meta.is_dir() {
            fs::remove_dir_all(p)
        } else {
            fs::remove_file(p)
        }
    };
    if try_remove(p).is_ok() {
        return Ok(());
    }
    chmod_dirs_recursive(p);
    try_remove(p).map_err(|e| e.to_string())
}

fn chmod_dirs_recursive(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = p.symlink_metadata() {
        if meta.is_dir() {
            let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o755));
            if let Ok(entries) = fs::read_dir(p) {
                for e in entries.flatten() {
                    chmod_dirs_recursive(&e.path());
                }
            }
        }
    }
}

/// Delete the contents of a directory but keep the directory itself.
pub fn empty_dir(p: &Path) -> Result<(), String> {
    let entries = fs::read_dir(p).map_err(|e| e.to_string())?;
    let mut first_err = None;
    for e in entries.flatten() {
        if let Err(err) = delete_path(&e.path()) {
            first_err.get_or_insert(err);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Run a vetted cleanup command in a login shell, capturing output, with a
/// hard timeout.
pub fn run_command(command: &str, timeout: Duration) -> (String, bool) {
    let child = Command::new("/bin/zsh")
        .args(["-lc", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (e.to_string(), false),
    };

    // drain pipes on threads so the child never blocks on full buffers
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        if let Some(ref mut r) = stdout {
            let _ = r.read_to_string(&mut s);
        }
        s
    });
    let err_t = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        if let Some(ref mut r) = stderr {
            let _ = r.read_to_string(&mut s);
        }
        s
    });

    let deadline = Instant::now() + timeout;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                break false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(150)),
            Err(_) => break false,
        }
    };
    let mut out = out_t.join().unwrap_or_default();
    out.push('\n');
    out.push_str(&err_t.join().unwrap_or_default());
    (out, ok)
}

/// Actual on-disk usage of a path right now (sparse-aware, like the scanner).
pub fn quick_du(p: &Path) -> i64 {
    let mut total = 0i64;
    if let Ok(meta) = p.symlink_metadata() {
        total += meta.blocks() as i64 * 512;
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(p) {
                for e in entries.flatten() {
                    total += quick_du(&e.path());
                }
            }
        }
    }
    total
}

/// Last n non-empty lines of command output.
pub fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

// ───────────────────────── clean orchestration ─────────────────────────

pub struct CleanJob {
    pub rec: Rec,
    pub action: Action,
}

pub enum CleanEvent {
    Started {
        id: String,
        title: String,
    },
    Result {
        id: String,
        title: String,
        ok: bool,
        freed: i64,
        pending: i64,
        message: String,
    },
    Done {
        freed: i64,
        pending: i64,
    },
}

/// Execute jobs sequentially on a background thread, streaming events.
/// Commands only ever run the vetted string stored on the Rec.
pub fn run_clean(jobs: Vec<CleanJob>, tx: Sender<CleanEvent>) {
    std::thread::Builder::new()
        .name("clean".into())
        .spawn(move || {
            let mut total_freed = 0i64;
            let mut total_pending = 0i64;
            for job in jobs {
                let rec = job.rec;
                let fs_path = strip_data_root(&rec.path);
                let _ = tx.send(CleanEvent::Started {
                    id: rec.id.clone(),
                    title: rec.title.clone(),
                });
                let before = quick_du(&fs_path);
                // command recs are locked to their command, whatever the UI says
                let action = if rec.action == Action::Command {
                    Action::Command
                } else {
                    job.action
                };

                let (ok, freed, pending, message) = match action {
                    Action::Command => {
                        let cmd = rec.command.unwrap_or("");
                        let (out, ok) = run_command(cmd, Duration::from_secs(15 * 60));
                        let after = quick_du(&fs_path);
                        let freed = (before - after).max(0);
                        (ok, freed, 0, tail_lines(&out, if ok { 2 } else { 3 }))
                    }
                    Action::Trash if rec.allow_trash => match trash_path(&fs_path) {
                        Ok(()) => (
                            true,
                            0,
                            before,
                            "moved to Trash — empty it to free the space".into(),
                        ),
                        Err(e) => (false, 0, 0, e),
                    },
                    Action::Delete if rec.allow_delete => match delete_path(&fs_path) {
                        Ok(()) => (true, before, 0, String::new()),
                        Err(e) => (false, 0, 0, e),
                    },
                    Action::Empty => match empty_dir(&fs_path) {
                        Ok(()) => (true, before - quick_du(&fs_path), 0, String::new()),
                        Err(e) => (false, 0, 0, e),
                    },
                    _ => (false, 0, 0, "action not allowed for this item".into()),
                };

                total_freed += freed;
                total_pending += pending;
                let _ = tx.send(CleanEvent::Result {
                    id: rec.id,
                    title: rec.title,
                    ok,
                    freed,
                    pending,
                    message,
                });
            }
            let _ = tx.send(CleanEvent::Done {
                freed: total_freed,
                pending: total_pending,
            });
        })
        .expect("spawn clean thread");
}

// ───────────────────────── misc helpers ─────────────────────────

pub fn reveal_in_finder(p: &Path) {
    let _ = Command::new("/usr/bin/open")
        .arg("-R")
        .arg(strip_data_root(p))
        .spawn();
}

pub fn open_full_disk_access() {
    let _ = Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn();
}

pub fn fmt_bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 || i == 0 {
        format!("{:.0} {}", v, UNITS[i])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

pub fn fmt_count(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
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

    #[test]
    fn quick_du_measures() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(&tmp.path().join("a/x.bin"), 8192);
        write_file(&tmp.path().join("b.bin"), 4096);
        assert!(quick_du(tmp.path()) >= 8192 + 4096);
        assert_eq!(quick_du(&tmp.path().join("missing")), 0);
    }

    #[test]
    fn delete_clears_write_protected() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        write_file(&locked.join("f.bin"), 1024);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

        delete_path(&locked).expect("chmod-and-retry should succeed");
        assert!(!locked.exists());
    }

    #[test]
    fn empty_keeps_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("trashlike");
        write_file(&target.join("old1.bin"), 1024);
        write_file(&target.join("sub/old2.bin"), 1024);

        empty_dir(&target).unwrap();
        assert!(target.exists());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    }

    #[test]
    fn tail_lines_cases() {
        assert_eq!(tail_lines("one\n\ntwo\nthree\nfour\n", 2), "three\nfour");
        assert_eq!(tail_lines("solo", 5), "solo");
        assert_eq!(tail_lines("", 3), "");
    }

    #[test]
    fn fmt_bytes_decimal() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1_500_000), "1.5 MB");
        assert_eq!(fmt_bytes(56_200_000_000), "56.2 GB");
    }

    #[test]
    fn run_command_captures_and_times_out() {
        let (out, ok) = run_command("echo hello", Duration::from_secs(10));
        assert!(ok && out.contains("hello"));
        let t0 = Instant::now();
        let (_, ok) = run_command("sleep 30", Duration::from_millis(400));
        assert!(!ok && t0.elapsed() < Duration::from_secs(5));
    }
}
