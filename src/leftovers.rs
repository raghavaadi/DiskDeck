//! Conservative, read-only detection of large orphaned app sandbox containers.

use crate::rules::strip_data_root;
use crate::scan::{lookup, Node, DATA_ROOT};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

const MIN_LEFTOVER_BYTES: i64 = 250 << 20;
const MAX_CANDIDATES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftoverFinding {
    pub bundle_id: String,
    pub path: PathBuf,
    pub bytes: i64,
    pub evidence: String,
}

fn bundle_id_shaped(value: &str) -> bool {
    !value.starts_with("com.apple.")
        && value.len() <= 255
        && value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn candidate(bundle_id: &str, bytes: i64, installed: Result<bool, String>) -> bool {
    bundle_id_shaped(bundle_id) && bytes >= MIN_LEFTOVER_BYTES && matches!(installed, Ok(false))
}

fn standard_bundle_ids(home: &Path) -> HashSet<String> {
    fn visit(root: &Path, depth: usize, ids: &mut HashSet<String>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_app = path.extension().and_then(|value| value.to_str()) == Some("app");
            if is_app {
                let info = path.join("Contents/Info.plist");
                if let Ok(output) = Command::new("/usr/libexec/PlistBuddy")
                    .args(["-c", "Print :CFBundleIdentifier"])
                    .arg(info)
                    .stdin(Stdio::null())
                    .output()
                {
                    if output.status.success() {
                        if let Ok(value) = String::from_utf8(output.stdout) {
                            let value = value.trim();
                            if bundle_id_shaped(value) || value.starts_with("com.apple.") {
                                ids.insert(value.to_owned());
                            }
                        }
                    }
                }
            } else if depth > 0 && path.is_dir() {
                visit(&path, depth - 1, ids);
            }
        }
    }
    let mut ids = HashSet::new();
    for root in [
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        home.join("Applications"),
    ] {
        visit(&root, 1, &mut ids);
    }
    ids
}

fn exact_bundle_installed(bundle_id: &str, installed: &HashSet<String>) -> Result<bool, String> {
    if !bundle_id_shaped(bundle_id) {
        return Ok(true);
    }
    if installed.contains(bundle_id) {
        return Ok(true);
    }
    let query = format!("kMDItemCFBundleIdentifier == '{bundle_id}'c");
    let output = Command::new("/usr/bin/mdfind")
        .arg(query)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("query installed apps: {error}"))?;
    if !output.status.success() {
        return Err("Spotlight could not verify installed apps".into());
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "Spotlight returned invalid app paths".to_string())?;
    Ok(text.lines().any(|line| {
        line.ends_with(".app") || line.contains(".app/Contents/") || line.contains(".app/PlugIns/")
    }))
}

pub fn detect(root: &Arc<Node>, home: &Path) -> Result<Vec<LeftoverFinding>, String> {
    let data_home = PathBuf::from(format!("{DATA_ROOT}{}", home.to_string_lossy()));
    let containers_path = data_home.join("Library/Containers");
    let Some(containers) = lookup(root, &containers_path) else {
        return Ok(Vec::new());
    };
    let mut measured: Vec<(String, PathBuf, i64)> = containers
        .kids()
        .into_iter()
        .filter_map(|node| {
            let bundle_id = node.path.file_name()?.to_str()?.to_owned();
            let bytes = node.bytes();
            (bundle_id_shaped(&bundle_id) && bytes >= MIN_LEFTOVER_BYTES)
                .then(|| (bundle_id, strip_data_root(&node.path), bytes))
        })
        .collect();
    measured.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
    measured.truncate(MAX_CANDIDATES);

    let installed = standard_bundle_ids(home);
    let mut findings = Vec::new();
    for (bundle_id, path, bytes) in measured {
        let installed_match = exact_bundle_installed(&bundle_id, &installed);
        if candidate(&bundle_id, bytes, installed_match) {
            findings.push(LeftoverFinding {
                evidence: format!(
                    "Sandbox container is named for bundle ID {bundle_id}; no matching installed app or extension was found."
                ),
                bundle_id,
                path,
                bytes,
            });
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_identifier_policy_is_conservative() {
        assert!(bundle_id_shaped("com.example.Editor"));
        assert!(bundle_id_shaped("io.vendor.tool-helper"));
        assert!(!bundle_id_shaped("com.apple.Safari"));
        assert!(!bundle_id_shaped("NoDots"));
        assert!(!bundle_id_shaped("com.example.bad_name"));
        assert!(!bundle_id_shaped("com..broken"));
    }

    #[test]
    fn candidate_requires_size_and_a_successful_absence_proof() {
        assert!(candidate(
            "com.example.Editor",
            MIN_LEFTOVER_BYTES,
            Ok(false)
        ));
        assert!(!candidate(
            "com.example.Editor",
            MIN_LEFTOVER_BYTES - 1,
            Ok(false)
        ));
        assert!(!candidate(
            "com.example.Editor",
            MIN_LEFTOVER_BYTES,
            Ok(true)
        ));
        assert!(!candidate(
            "com.example.Editor",
            MIN_LEFTOVER_BYTES,
            Err("lookup failed".into())
        ));
    }
}
