//! Deterministic, local-only grouping for the opt-in Developer Lens.

use crate::rules::{Rec, Tier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeveloperKind {
    Containers,
    AppleDevelopment,
    JavaScriptProjects,
    PackageStores,
    BuildTooling,
}

impl DeveloperKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Containers => "Containers",
            Self::AppleDevelopment => "Xcode & simulators",
            Self::JavaScriptProjects => "JavaScript projects",
            Self::PackageStores => "Package stores",
            Self::BuildTooling => "Build tooling",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::Containers => "Unused images and builder layers inside Docker's VM.",
            Self::AppleDevelopment => {
                "Xcode indexes, archives, device support, and unavailable simulators."
            }
            Self::JavaScriptProjects => {
                "Installed node_modules, npm data, browser test binaries, and JS tooling caches."
            }
            Self::PackageStores => {
                "Downloaded dependencies retained by language and package managers."
            }
            Self::BuildTooling => "Compiled intermediates that tools can regenerate.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeveloperFinding {
    pub title: String,
    pub display: String,
    pub bytes: i64,
    pub caution: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeveloperGroup {
    pub kind: DeveloperKind,
    pub bytes: i64,
    pub caution_count: usize,
    pub findings: Vec<DeveloperFinding>,
}

fn classify(id: &str) -> Option<DeveloperKind> {
    if id == "docker-prune" {
        Some(DeveloperKind::Containers)
    } else if id.starts_with("xcode-") || id == "sim-unavailable" || id == "ios-devicesupport" {
        Some(DeveloperKind::AppleDevelopment)
    } else if id.starts_with("nm-")
        || matches!(id, "npm-cache" | "playwright")
        || id.starts_with("cache-Yarn")
        || id.starts_with("cache-typescript")
        || id.starts_with("cache-node-gyp")
        || id.starts_with("cache-electron")
    {
        Some(DeveloperKind::JavaScriptProjects)
    } else if matches!(
        id,
        "go-modcache" | "pip-cache" | "cargo" | "gradle" | "maven" | "cocoapods" | "brew-cleanup"
    ) {
        Some(DeveloperKind::PackageStores)
    } else if id == "go-buildcache" {
        Some(DeveloperKind::BuildTooling)
    } else {
        None
    }
}

pub fn analyze(recs: &[Rec]) -> Vec<DeveloperGroup> {
    let order = [
        DeveloperKind::Containers,
        DeveloperKind::AppleDevelopment,
        DeveloperKind::JavaScriptProjects,
        DeveloperKind::PackageStores,
        DeveloperKind::BuildTooling,
    ];
    order
        .into_iter()
        .filter_map(|kind| {
            let mut findings: Vec<DeveloperFinding> = recs
                .iter()
                .filter(|rec| classify(&rec.id) == Some(kind))
                .map(|rec| DeveloperFinding {
                    title: rec.title.clone(),
                    display: rec.display.clone(),
                    bytes: rec.bytes,
                    caution: rec.tier == Tier::Caution,
                })
                .collect();
            if findings.is_empty() {
                return None;
            }
            findings.sort_by(|left, right| {
                right
                    .bytes
                    .cmp(&left.bytes)
                    .then_with(|| left.title.cmp(&right.title))
            });
            Some(DeveloperGroup {
                kind,
                bytes: findings.iter().map(|finding| finding.bytes).sum(),
                caution_count: findings.iter().filter(|finding| finding.caution).count(),
                findings,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Action, Rec};
    use std::path::PathBuf;

    fn rec(id: &str, bytes: i64, tier: Tier) -> Rec {
        Rec {
            id: id.into(),
            title: id.into(),
            path: PathBuf::from("fixture"),
            display: "~/fixture".into(),
            bytes,
            tier,
            desc: "fixture",
            restore: "fixture",
            action: Action::Trash,
            command: None,
            allow_trash: true,
            allow_delete: true,
            note: String::new(),
            estimate: false,
        }
    }

    #[test]
    fn groups_developer_findings_without_ordinary_cleanup_rows() {
        let groups = analyze(&[
            rec("docker-prune", 500, Tier::Safe),
            rec("xcode-derived", 300, Tier::Safe),
            rec("sim-unavailable", 200, Tier::Safe),
            rec("nm-0", 100, Tier::Caution),
            rec("npm-cache", 50, Tier::Safe),
            rec("cargo", 40, Tier::Caution),
            rec("go-buildcache", 30, Tier::Safe),
            rec("cache-Google", 999, Tier::Safe),
            rec("trash", 999, Tier::Caution),
        ]);
        assert_eq!(groups.len(), 5);
        assert_eq!(groups[0].kind, DeveloperKind::Containers);
        assert_eq!(groups[1].kind, DeveloperKind::AppleDevelopment);
        assert_eq!(groups[1].bytes, 500);
        assert_eq!(groups[2].kind, DeveloperKind::JavaScriptProjects);
        assert_eq!(groups[2].bytes, 150);
        assert_eq!(groups[2].caution_count, 1);
        assert_eq!(groups[3].kind, DeveloperKind::PackageStores);
        assert_eq!(groups[4].kind, DeveloperKind::BuildTooling);
        assert_eq!(groups.iter().map(|group| group.bytes).sum::<i64>(), 1_220);
    }

    #[test]
    fn findings_are_largest_first_with_stable_titles() {
        let groups = analyze(&[
            rec("nm-0", 20, Tier::Caution),
            rec("npm-cache", 40, Tier::Safe),
            rec("nm-1", 40, Tier::Caution),
        ]);
        let titles: Vec<&str> = groups[0]
            .findings
            .iter()
            .map(|finding| finding.title.as_str())
            .collect();
        assert_eq!(titles, vec!["nm-1", "npm-cache", "nm-0"]);
    }
}
