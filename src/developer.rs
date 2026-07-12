//! Deterministic, local-only grouping for the opt-in Developer Lens.

use crate::rules::{Rec, Tier};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeveloperSection {
    Docker,
    Xcode,
    Projects,
    PackageStores,
    BuildTooling,
    Ungrouped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildCost {
    QuickRegeneration,
    LargeDownload,
    ManualSetup,
}

impl RebuildCost {
    pub fn label(self) -> &'static str {
        match self {
            Self::QuickRegeneration => "Quick regeneration",
            Self::LargeDownload => "Large download",
            Self::ManualSetup => "Manual setup",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceTier {
    Safe,
    Caution,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub source_rec_id: Option<String>,
    pub measured_path: PathBuf,
    pub display_path: String,
    pub tier: Option<EvidenceTier>,
    pub estimated: bool,
    pub command: Option<&'static str>,
    pub explanation: String,
    pub recovery: String,
    pub overlap: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepFinding {
    pub title: String,
    pub bytes: i64,
    pub rebuild_cost: RebuildCost,
    pub counted: bool,
    pub evidence: Evidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeveloperSectionReport {
    pub section: DeveloperSection,
    pub measured_bytes: i64,
    pub findings: Vec<DeepFinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerDetail {
    pub title: String,
    pub bytes: i64,
    pub reclaimable_bytes: i64,
    pub rebuild_cost: RebuildCost,
    pub counted: bool,
    pub explanation: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerBreakdown {
    pub details: Vec<DockerDetail>,
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeveloperReport {
    pub measured_bytes: i64,
    pub sections: Vec<DeveloperSectionReport>,
    pub docker: DockerBreakdown,
}

fn deep_section(id: &str) -> Option<DeveloperSection> {
    if id == "docker-prune" {
        Some(DeveloperSection::Docker)
    } else if id.starts_with("xcode-") || id == "sim-unavailable" || id == "ios-devicesupport" {
        Some(DeveloperSection::Xcode)
    } else if id.starts_with("nm-") {
        Some(DeveloperSection::Projects)
    } else if matches!(
        id,
        "go-modcache"
            | "pip-cache"
            | "cargo"
            | "gradle"
            | "maven"
            | "cocoapods"
            | "brew-cleanup"
            | "npm-cache"
            | "playwright"
    ) || id.starts_with("cache-Yarn")
        || id.starts_with("cache-typescript")
        || id.starts_with("cache-node-gyp")
        || id.starts_with("cache-electron")
    {
        Some(DeveloperSection::PackageStores)
    } else if id == "go-buildcache" {
        Some(DeveloperSection::BuildTooling)
    } else {
        None
    }
}

fn rebuild_cost(id: &str) -> RebuildCost {
    if matches!(id, "xcode-derived" | "go-buildcache") {
        RebuildCost::QuickRegeneration
    } else if id == "xcode-archives" {
        RebuildCost::ManualSetup
    } else {
        RebuildCost::LargeDownload
    }
}

fn evidence_tier(tier: Tier) -> EvidenceTier {
    match tier {
        Tier::Safe => EvidenceTier::Safe,
        Tier::Caution => EvidenceTier::Caution,
    }
}

fn section_order() -> [DeveloperSection; 6] {
    [
        DeveloperSection::Docker,
        DeveloperSection::Xcode,
        DeveloperSection::Projects,
        DeveloperSection::PackageStores,
        DeveloperSection::BuildTooling,
        DeveloperSection::Ungrouped,
    ]
}

pub fn build_report(recs: &[Rec], mut docker: DockerBreakdown) -> DeveloperReport {
    let mut candidates: Vec<(DeveloperSection, DeepFinding)> = recs
        .iter()
        .filter(|rec| rec.bytes > 0)
        .filter_map(|rec| {
            let section = deep_section(&rec.id)?;
            Some((
                section,
                DeepFinding {
                    title: rec.title.clone(),
                    bytes: rec.bytes,
                    rebuild_cost: rebuild_cost(&rec.id),
                    counted: true,
                    evidence: Evidence {
                        source_rec_id: Some(rec.id.clone()),
                        measured_path: rec.path.clone(),
                        display_path: rec.display.clone(),
                        tier: Some(evidence_tier(rec.tier)),
                        estimated: rec.estimate,
                        command: rec.command,
                        explanation: rec.desc.into(),
                        recovery: rec.restore.into(),
                        overlap: None,
                    },
                },
            ))
        })
        .collect();

    candidates.sort_by(|(left_section, left), (right_section, right)| {
        left.evidence
            .measured_path
            .components()
            .count()
            .cmp(&right.evidence.measured_path.components().count())
            .then_with(|| left_section.cmp(right_section))
            .then_with(|| {
                left.evidence
                    .source_rec_id
                    .cmp(&right.evidence.source_rec_id)
            })
    });

    let mut counted_paths: Vec<(PathBuf, String)> = Vec::new();
    for (_, finding) in &mut candidates {
        if let Some((_, owner_display)) = counted_paths.iter().find(|(owner, _)| {
            finding.evidence.measured_path == *owner
                || finding.evidence.measured_path.starts_with(owner)
        }) {
            finding.counted = false;
            finding.evidence.overlap = Some(format!("Included in {owner_display}"));
        } else {
            counted_paths.push((
                finding.evidence.measured_path.clone(),
                finding.evidence.display_path.clone(),
            ));
        }
    }

    for detail in &mut docker.details {
        detail.counted = false;
        if !detail.explanation.contains("not added") {
            detail.explanation =
                "Inside Docker; not added to the measured filesystem footprint.".into();
        }
    }

    let mut sections = Vec::new();
    for section in section_order() {
        let mut findings: Vec<DeepFinding> = candidates
            .iter()
            .filter(|(candidate_section, _)| *candidate_section == section)
            .map(|(_, finding)| finding.clone())
            .collect();
        if findings.is_empty() {
            continue;
        }
        findings.sort_by(|left, right| {
            right
                .bytes
                .cmp(&left.bytes)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| {
                    left.evidence
                        .source_rec_id
                        .cmp(&right.evidence.source_rec_id)
                })
        });
        let measured_bytes = findings
            .iter()
            .filter(|finding| finding.counted)
            .fold(0_i64, |total, finding| total.saturating_add(finding.bytes));
        sections.push(DeveloperSectionReport {
            section,
            measured_bytes,
            findings,
        });
    }
    let measured_bytes = sections.iter().fold(0_i64, |total, section| {
        total.saturating_add(section.measured_bytes)
    });
    DeveloperReport {
        measured_bytes,
        sections,
        docker,
    }
}

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

    fn rec_at(
        id: &str,
        path: &str,
        bytes: i64,
        tier: Tier,
        command: Option<&'static str>,
        estimate: bool,
    ) -> Rec {
        let mut value = rec(id, bytes, tier);
        value.path = PathBuf::from(path);
        value.display = path.into();
        value.command = command;
        value.estimate = estimate;
        value
    }

    #[test]
    fn rebuild_cost_labels_are_visible_words() {
        assert_eq!(RebuildCost::QuickRegeneration.label(), "Quick regeneration");
        assert_eq!(RebuildCost::LargeDownload.label(), "Large download");
        assert_eq!(RebuildCost::ManualSetup.label(), "Manual setup");
    }

    #[test]
    fn report_preserves_rule_evidence_and_section_order() {
        let report = build_report(
            &[
                rec_at(
                    "go-buildcache",
                    "/fixture/go-build",
                    30,
                    Tier::Safe,
                    Some("go clean -cache"),
                    false,
                ),
                rec_at(
                    "npm-cache",
                    "/fixture/npm",
                    50,
                    Tier::Safe,
                    Some("npm cache clean --force"),
                    false,
                ),
                rec_at(
                    "nm-1",
                    "/fixture/project/node_modules",
                    100,
                    Tier::Caution,
                    None,
                    false,
                ),
                rec_at(
                    "xcode-archives",
                    "/fixture/Archives",
                    200,
                    Tier::Caution,
                    None,
                    false,
                ),
                rec_at(
                    "docker-prune",
                    "/fixture/Docker",
                    500,
                    Tier::Safe,
                    Some("docker image prune -a -f && docker builder prune -a -f"),
                    true,
                ),
            ],
            DockerBreakdown::default(),
        );
        let sections: Vec<DeveloperSection> = report
            .sections
            .iter()
            .map(|section| section.section)
            .collect();
        assert_eq!(
            sections,
            vec![
                DeveloperSection::Docker,
                DeveloperSection::Xcode,
                DeveloperSection::Projects,
                DeveloperSection::PackageStores,
                DeveloperSection::BuildTooling,
            ]
        );
        assert_eq!(report.measured_bytes, 880);

        let docker = &report.sections[0].findings[0];
        assert_eq!(docker.rebuild_cost, RebuildCost::LargeDownload);
        assert_eq!(
            docker.evidence.source_rec_id.as_deref(),
            Some("docker-prune")
        );
        assert_eq!(docker.evidence.tier, Some(EvidenceTier::Safe));
        assert!(docker.evidence.estimated);
        assert_eq!(
            docker.evidence.command,
            Some("docker image prune -a -f && docker builder prune -a -f")
        );

        let archive = &report.sections[1].findings[0];
        assert_eq!(archive.rebuild_cost, RebuildCost::ManualSetup);
        assert_eq!(archive.evidence.tier, Some(EvidenceTier::Caution));
        assert!(!archive.evidence.explanation.is_empty());
        assert!(!archive.evidence.recovery.is_empty());
    }

    #[test]
    fn exact_and_nested_paths_are_visible_but_counted_once() {
        let report = build_report(
            &[
                rec_at(
                    "xcode-derived",
                    "/fixture/project",
                    100,
                    Tier::Safe,
                    None,
                    false,
                ),
                rec_at(
                    "nm-1",
                    "/fixture/project/node_modules",
                    40,
                    Tier::Caution,
                    None,
                    false,
                ),
                rec_at(
                    "nm-2",
                    "/fixture/project/node_modules",
                    40,
                    Tier::Caution,
                    None,
                    false,
                ),
            ],
            DockerBreakdown::default(),
        );
        assert_eq!(report.measured_bytes, 100);
        let nested: Vec<&DeepFinding> = report
            .sections
            .iter()
            .flat_map(|section| &section.findings)
            .filter(|finding| !finding.counted)
            .collect();
        assert_eq!(nested.len(), 2);
        assert!(nested
            .iter()
            .all(|finding| finding.evidence.overlap.is_some()));
    }

    #[test]
    fn docker_inside_vm_details_never_inflate_measured_total() {
        let docker = DockerBreakdown {
            details: vec![DockerDetail {
                title: "Images".into(),
                bytes: 900,
                reclaimable_bytes: 500,
                rebuild_cost: RebuildCost::LargeDownload,
                counted: false,
                explanation: "Inside Docker; not added to the measured footprint.".into(),
            }],
            unavailable: None,
        };
        let report = build_report(
            &[rec_at(
                "docker-prune",
                "/fixture/Docker",
                1_000,
                Tier::Safe,
                None,
                true,
            )],
            docker,
        );
        assert_eq!(report.measured_bytes, 1_000);
        assert_eq!(report.docker.details[0].bytes, 900);
        assert!(!report.docker.details[0].counted);
        assert!(report.docker.details[0].explanation.contains("not added"));
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
