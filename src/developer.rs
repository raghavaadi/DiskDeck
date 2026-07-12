//! Deterministic, local-only grouping for the opt-in Developer Lens.

use crate::rules::{Rec, Tier};
use crate::scan::Node;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeveloperSection {
    Docker,
    Xcode,
    Projects,
    PackageStores,
    BuildTooling,
    Ungrouped,
}

impl DeveloperSection {
    pub fn title(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Xcode => "Xcode",
            Self::Projects => "Projects",
            Self::PackageStores => "Package stores",
            Self::BuildTooling => "Build tooling",
            Self::Ungrouped => "Ungrouped output",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::Docker => "Measured on disk; inside-VM categories are explanatory and uncounted.",
            Self::Xcode => "Build data, archives, device support, runtimes, and simulator devices.",
            Self::Projects => {
                "Outputs grouped by an existing rule or a nearby standard project marker."
            }
            Self::PackageStores => {
                "Downloaded dependencies retained by language and package managers."
            }
            Self::BuildTooling => "Compiled intermediates that tools can regenerate.",
            Self::Ungrouped => {
                "Measured output without enough evidence to assign project ownership."
            }
        }
    }
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
    pub project_root: Option<PathBuf>,
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

fn deep_section(rec: &Rec) -> Option<DeveloperSection> {
    let id = rec.id.as_str();
    if id == "docker-prune" {
        Some(DeveloperSection::Docker)
    } else if id.starts_with("xcode-") || id == "sim-unavailable" || id == "ios-devicesupport" {
        Some(DeveloperSection::Xcode)
    } else if id.starts_with("nm-") {
        if rec
            .path
            .components()
            .any(|component| component.as_os_str() == ".npm")
        {
            Some(DeveloperSection::PackageStores)
        } else {
            Some(DeveloperSection::Projects)
        }
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

fn candidates_from_recs(recs: &[Rec]) -> Vec<(DeveloperSection, DeepFinding)> {
    recs.iter()
        .filter(|rec| rec.bytes > 0)
        .filter_map(|rec| {
            let section = deep_section(rec)?;
            Some((
                section,
                DeepFinding {
                    title: rec.title.clone(),
                    bytes: rec.bytes,
                    rebuild_cost: rebuild_cost(&rec.id),
                    counted: true,
                    project_root: if section == DeveloperSection::Projects {
                        rec.path.parent().map(PathBuf::from)
                    } else {
                        None
                    },
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
        .collect()
}

fn assemble_report(
    mut candidates: Vec<(DeveloperSection, DeepFinding)>,
    mut docker: DockerBreakdown,
) -> DeveloperReport {
    candidates.sort_by(|(left_section, left), (right_section, right)| {
        left.evidence
            .measured_path
            .components()
            .count()
            .cmp(&right.evidence.measured_path.components().count())
            .then_with(|| {
                left.evidence
                    .source_rec_id
                    .is_none()
                    .cmp(&right.evidence.source_rec_id.is_none())
            })
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

#[cfg(test)]
fn build_report(recs: &[Rec], docker: DockerBreakdown) -> DeveloperReport {
    assemble_report(candidates_from_recs(recs), docker)
}

const PROJECT_MIN_BYTES: i64 = 20_000_000;
const PROJECT_CANDIDATE_CAP: usize = 200;

fn is_project_candidate(path: &std::path::Path, home: &std::path::Path, name: &str) -> bool {
    if !path.starts_with(home) || !matches!(name, "target" | ".venv" | "venv" | "build" | "dist") {
        return false;
    }
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let components: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    for (index, component) in components.iter().enumerate() {
        let last = index + 1 == components.len();
        if component == "Library"
            || matches!(component.as_str(), "CloudStorage" | "Mobile Documents")
            || component.ends_with(".app")
            || component.ends_with(".photoslibrary")
            || component.ends_with(".musiclibrary")
            || (component.starts_with('.') && !(last && component == ".venv"))
        {
            return false;
        }
    }
    true
}

fn project_markers(name: &str) -> &'static [&'static str] {
    match name {
        "target" => &["Cargo.toml"],
        ".venv" | "venv" => &["pyproject.toml", "requirements.txt", "Pipfile"],
        "build" | "dist" => &[
            "package.json",
            "Cargo.toml",
            "pyproject.toml",
            "requirements.txt",
            "Pipfile",
        ],
        _ => &[],
    }
}

fn inventory_finding(
    node: &Arc<Node>,
    project_root: Option<PathBuf>,
    marker: Option<&str>,
) -> DeepFinding {
    let name = node.name.as_str();
    let owned = project_root.is_some();
    let project_name = project_root
        .as_deref()
        .and_then(std::path::Path::file_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unowned project".into());
    let rebuild_cost = if !owned {
        RebuildCost::ManualSetup
    } else if matches!(name, ".venv" | "venv") {
        RebuildCost::LargeDownload
    } else {
        RebuildCost::QuickRegeneration
    };
    let title = if owned {
        format!("{name} — {project_name}")
    } else {
        format!("{name} — ungrouped output")
    };
    let explanation = marker.map_or_else(
        || {
            "Measured in the retained scan, but no nearby standard project marker proves ownership. Reveal-only; never a cleanup rule."
                .into()
        },
        |marker| {
            format!(
                "Measured in the retained scan and grouped by nearby {marker}. This discovered path is reveal-only."
            )
        },
    );
    DeepFinding {
        title,
        bytes: node.bytes(),
        rebuild_cost,
        counted: true,
        project_root,
        evidence: Evidence {
            source_rec_id: None,
            measured_path: node.path.clone(),
            display_path: crate::rules::display(&node.path),
            tier: None,
            estimated: false,
            command: None,
            explanation,
            recovery: if rebuild_cost == RebuildCost::QuickRegeneration {
                "The owning build tool should regenerate this output; DiskDeck does not remove it from Developer Lens."
                    .into()
            } else if rebuild_cost == RebuildCost::LargeDownload {
                "The owning project can recreate this environment, potentially with a large dependency download."
                    .into()
            } else {
                "Ownership is ambiguous, so inspect it in Finder and preserve it unless you understand the project."
                    .into()
            },
            overlap: None,
        },
    }
}

fn xcode_inventory_finding(
    node: &Arc<Node>,
    title: &str,
    rebuild_cost: RebuildCost,
    recovery: &str,
) -> DeepFinding {
    DeepFinding {
        title: title.into(),
        bytes: node.bytes(),
        rebuild_cost,
        counted: true,
        project_root: None,
        evidence: Evidence {
            source_rec_id: None,
            measured_path: node.path.clone(),
            display_path: crate::rules::display(&node.path),
            tier: None,
            estimated: false,
            command: None,
            explanation:
                "Measured at a fixed Xcode location in the retained scan; shown read-only unless a vetted rule exists."
                    .into(),
            recovery: recovery.into(),
            overlap: None,
        },
    }
}

pub fn build_report_with_inventory<F>(
    recs: &[Rec],
    docker: DockerBreakdown,
    root: &Arc<Node>,
    home: &std::path::Path,
    mut marker_exists: F,
) -> DeveloperReport
where
    F: FnMut(&std::path::Path, &str) -> bool,
{
    let mut candidates = candidates_from_recs(recs);
    let rec_paths: std::collections::BTreeSet<PathBuf> = candidates
        .iter()
        .map(|(_, finding)| finding.evidence.measured_path.clone())
        .collect();
    let fixed_xcode = [
        (
            home.join("Library/Developer/Xcode/DerivedData"),
            "Xcode DerivedData",
            RebuildCost::QuickRegeneration,
            "Xcode rebuilds indexes and intermediates on the next project build.",
        ),
        (
            home.join("Library/Developer/Xcode/Archives"),
            "Xcode Archives",
            RebuildCost::ManualSetup,
            "Archives may be required for symbolication or distribution; keep them unless reviewed.",
        ),
        (
            home.join("Library/Developer/Xcode/iOS DeviceSupport"),
            "iOS Device Support",
            RebuildCost::LargeDownload,
            "Xcode recopies support data when the matching device reconnects.",
        ),
        (
            home.join("Library/Developer/CoreSimulator/Profiles/Runtimes"),
            "Simulator runtimes",
            RebuildCost::LargeDownload,
            "Simulator runtimes require a large Xcode download to restore.",
        ),
        (
            home.join("Library/Developer/CoreSimulator/Devices"),
            "Simulator devices",
            RebuildCost::ManualSetup,
            "Simulator devices may contain app state and remain review-only without a vetted rule.",
        ),
    ];

    let mut project_nodes = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        for child in node.kids() {
            stack.push(child);
        }
        if !node.is_dir || node.bytes() <= 0 {
            continue;
        }
        if !rec_paths.contains(&node.path) {
            if let Some((_, title, cost, recovery)) = fixed_xcode
                .iter()
                .find(|(path, _, _, _)| *path == node.path)
            {
                candidates.push((
                    DeveloperSection::Xcode,
                    xcode_inventory_finding(&node, title, *cost, recovery),
                ));
            }
        }
        if node.bytes() >= PROJECT_MIN_BYTES && is_project_candidate(&node.path, home, &node.name) {
            project_nodes.push(node);
        }
    }

    project_nodes.sort_by(|left, right| {
        right
            .bytes()
            .cmp(&left.bytes())
            .then_with(|| left.path.cmp(&right.path))
    });
    project_nodes.truncate(PROJECT_CANDIDATE_CAP);
    for node in project_nodes {
        let Some(project) = node.path.parent() else {
            continue;
        };
        let marker = project_markers(&node.name)
            .iter()
            .copied()
            .find(|marker| marker_exists(project, marker));
        let (section, project_root) = if marker.is_some() {
            (DeveloperSection::Projects, Some(project.to_path_buf()))
        } else {
            (DeveloperSection::Ungrouped, None)
        };
        candidates.push((section, inventory_finding(&node, project_root, marker)));
    }
    assemble_report(candidates, docker)
}

const DOCKER_ARGS: [&str; 4] = [
    "system",
    "df",
    "--format",
    "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
];

fn docker_command_args() -> &'static [&'static str] {
    &DOCKER_ARGS
}

fn parse_docker_size(value: &str) -> Option<i64> {
    let value = value.trim();
    let unit_start = value.find(|character: char| character.is_ascii_alphabetic())?;
    let number: f64 = value[..unit_start].trim().parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let factor = match &value[unit_start..] {
        "B" => 1_f64,
        "kB" | "KB" => 1_000_f64,
        "MB" => 1_000_000_f64,
        "GB" => 1_000_000_000_f64,
        "TB" => 1_000_000_000_000_f64,
        "KiB" => 1_024_f64,
        "MiB" => 1_048_576_f64,
        "GiB" => 1_073_741_824_f64,
        "TiB" => 1_099_511_627_776_f64,
        _ => return None,
    };
    let bytes = number * factor;
    if !bytes.is_finite() || bytes > i64::MAX as f64 {
        None
    } else {
        Some(bytes.round() as i64)
    }
}

fn parse_docker_df(output: &str) -> Vec<DockerDetail> {
    let mut details: Vec<(usize, DockerDetail)> = output
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().splitn(3, '\t');
            let category = fields.next()?.trim();
            let bytes = parse_docker_size(fields.next()?.trim())?;
            let reclaimable = fields.next()?.split_whitespace().next()?;
            let reclaimable_bytes = parse_docker_size(reclaimable)?;
            if bytes <= 0 && reclaimable_bytes <= 0 {
                return None;
            }
            let (order, title, rebuild_cost) = match category {
                "Images" => (0, "Images", RebuildCost::LargeDownload),
                "Containers" => (1, "Containers", RebuildCost::ManualSetup),
                "Local Volumes" => (2, "Local volumes", RebuildCost::ManualSetup),
                "Build Cache" => (3, "Build cache", RebuildCost::QuickRegeneration),
                _ => return None,
            };
            Some((
                order,
                DockerDetail {
                    title: title.into(),
                    bytes,
                    reclaimable_bytes,
                    rebuild_cost,
                    counted: false,
                    explanation: "Inside Docker; not added to the measured filesystem footprint."
                        .into(),
                },
            ))
        })
        .collect();
    details.sort_by_key(|(order, _)| *order);
    details.into_iter().map(|(_, detail)| detail).collect()
}

fn load_docker_with<F>(run: F) -> DockerBreakdown
where
    F: FnOnce() -> Result<Vec<u8>, String>,
{
    let bytes = match run() {
        Ok(bytes) => bytes,
        Err(error) => {
            return DockerBreakdown {
                details: Vec::new(),
                unavailable: Some(format!("Docker breakdown unavailable: {error}")),
            }
        }
    };
    let output = match String::from_utf8(bytes) {
        Ok(output) => output,
        Err(_) => {
            return DockerBreakdown {
                details: Vec::new(),
                unavailable: Some("Docker returned non-UTF-8 output.".into()),
            }
        }
    };
    let details = parse_docker_df(&output);
    DockerBreakdown {
        unavailable: details
            .is_empty()
            .then(|| "Docker reported no measurable storage categories.".into()),
        details,
    }
}

pub fn load_docker_breakdown() -> DockerBreakdown {
    load_docker_with(run_fixed_docker)
}

const DOCKER_OUTPUT_CAP: usize = 64 * 1024;

fn docker_binary() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join("docker"))
            .find(|path| path.is_file())
    });
    from_path.or_else(|| {
        [
            "/usr/local/bin/docker",
            "/opt/homebrew/bin/docker",
            "/Applications/Docker.app/Contents/Resources/bin/docker",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
    })
}

fn capped_reader<R>(reader: R) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut output = Vec::new();
        reader
            .take((DOCKER_OUTPUT_CAP + 1) as u64)
            .read_to_end(&mut output)?;
        Ok(output)
    })
}

fn join_output(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>, String> {
    let output = reader
        .join()
        .map_err(|_| format!("Docker {stream} reader stopped"))?
        .map_err(|error| format!("read Docker {stream}: {error}"))?;
    if output.len() > DOCKER_OUTPUT_CAP {
        Err(format!("Docker {stream} exceeded 64 KiB"))
    } else {
        Ok(output)
    }
}

fn run_fixed_docker() -> Result<Vec<u8>, String> {
    let binary = docker_binary().ok_or("docker executable was not found")?;
    let mut child = Command::new(binary)
        .args(docker_command_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start docker system df: {error}"))?;
    let Some(stdout_pipe) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Docker stdout was unavailable".into());
    };
    let Some(stderr_pipe) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Docker stderr was unavailable".into());
    };
    let stdout = capped_reader(stdout_pipe);
    let stderr = capped_reader(stderr_pipe);
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_output(stdout, "stdout");
                let _ = join_output(stderr, "stderr");
                return Err("timed out after 3 seconds".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_output(stdout, "stdout");
                let _ = join_output(stderr, "stderr");
                return Err(format!("wait for docker system df: {error}"));
            }
        }
    };
    let stdout_result = join_output(stdout, "stdout");
    let stderr_result = join_output(stderr, "stderr");
    let stdout = stdout_result?;
    let stderr = stderr_result?;
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr);
        let message = message.trim();
        return Err(if message.is_empty() {
            format!("docker system df exited with {status}")
        } else {
            format!("docker system df failed: {message}")
        });
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Action, Rec};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

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

    fn test_root(path: &str) -> Arc<Node> {
        Arc::new(Node {
            name: PathBuf::from(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "root".into()),
            path: PathBuf::from(path),
            is_dir: true,
            bytes: std::sync::atomic::AtomicI64::new(0),
            files: std::sync::atomic::AtomicI64::new(0),
            small_bytes: std::sync::atomic::AtomicI64::new(0),
            small_count: std::sync::atomic::AtomicI64::new(0),
            denied: AtomicBool::new(false),
            parent: std::sync::Weak::new(),
            children: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn test_dir(parent: &Arc<Node>, path: &str, bytes: i64) -> Arc<Node> {
        let path = PathBuf::from(path);
        let child = Arc::new(Node {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            is_dir: true,
            bytes: std::sync::atomic::AtomicI64::new(bytes),
            files: std::sync::atomic::AtomicI64::new(1),
            small_bytes: std::sync::atomic::AtomicI64::new(0),
            small_count: std::sync::atomic::AtomicI64::new(0),
            denied: AtomicBool::new(false),
            parent: Arc::downgrade(parent),
            children: std::sync::Mutex::new(Vec::new()),
        });
        parent.children.lock().unwrap().push(child.clone());
        child
    }

    #[test]
    fn bounded_inventory_groups_owned_projects_and_keeps_ambiguity_read_only() {
        const MB: i64 = 1_000_000;
        let root = test_root("/data");
        test_dir(&root, "/data/home/rust/target", 100 * MB);
        test_dir(&root, "/data/home/python/.venv", 80 * MB);
        test_dir(&root, "/data/home/js/dist", 60 * MB);
        test_dir(&root, "/data/home/ambiguous/build", 50 * MB);
        test_dir(&root, "/data/home/Library/Bad/target", 500 * MB);
        test_dir(&root, "/data/home/small/target", 10 * MB);
        test_dir(&root, "/outside/project/target", 400 * MB);

        let report = build_report_with_inventory(
            &[rec_at(
                "nm-1",
                "/data/home/js/node_modules",
                120 * MB,
                Tier::Caution,
                None,
                false,
            )],
            DockerBreakdown::default(),
            &root,
            std::path::Path::new("/data/home"),
            |project, marker| {
                matches!(
                    (project.to_string_lossy().as_ref(), marker),
                    ("/data/home/rust", "Cargo.toml")
                        | ("/data/home/python", "pyproject.toml")
                        | ("/data/home/js", "package.json")
                )
            },
        );

        let projects = report
            .sections
            .iter()
            .find(|section| section.section == DeveloperSection::Projects)
            .unwrap();
        let project_paths: Vec<&str> = projects
            .findings
            .iter()
            .map(|finding| finding.evidence.measured_path.to_str().unwrap())
            .collect();
        assert_eq!(
            project_paths,
            vec![
                "/data/home/js/node_modules",
                "/data/home/rust/target",
                "/data/home/python/.venv",
                "/data/home/js/dist",
            ]
        );
        assert!(projects
            .findings
            .iter()
            .all(|finding| finding.project_root.is_some()));

        let ungrouped = report
            .sections
            .iter()
            .find(|section| section.section == DeveloperSection::Ungrouped)
            .unwrap();
        assert_eq!(ungrouped.findings.len(), 1);
        assert_eq!(ungrouped.findings[0].rebuild_cost, RebuildCost::ManualSetup);
        assert_eq!(ungrouped.findings[0].evidence.tier, None);
        assert_eq!(ungrouped.findings[0].evidence.command, None);
        assert_eq!(ungrouped.findings[0].project_root, None);
    }

    #[test]
    fn inventory_caps_marker_probes_and_is_deterministic() {
        const MB: i64 = 1_000_000;
        let root = test_root("/data");
        for index in (0..250).rev() {
            test_dir(
                &root,
                &format!("/data/home/project-{index:03}/target"),
                (30 + index) * MB,
            );
        }
        let calls = std::cell::Cell::new(0_usize);
        let report = build_report_with_inventory(
            &[],
            DockerBreakdown::default(),
            &root,
            std::path::Path::new("/data/home"),
            |_, marker| {
                calls.set(calls.get() + 1);
                marker == "Cargo.toml"
            },
        );
        assert!(calls.get() <= 1_000);
        let projects = report
            .sections
            .iter()
            .find(|section| section.section == DeveloperSection::Projects)
            .unwrap();
        assert_eq!(projects.findings.len(), 200);
        assert_eq!(
            projects.findings[0].evidence.measured_path,
            PathBuf::from("/data/home/project-249/target")
        );
        assert_eq!(
            projects.findings.last().unwrap().evidence.measured_path,
            PathBuf::from("/data/home/project-050/target")
        );
    }

    #[test]
    fn fixed_xcode_inventory_prefers_existing_rule_evidence() {
        const MB: i64 = 1_000_000;
        let root = test_root("/data");
        test_dir(
            &root,
            "/data/home/Library/Developer/CoreSimulator/Profiles/Runtimes",
            900 * MB,
        );
        test_dir(
            &root,
            "/data/home/Library/Developer/CoreSimulator/Devices",
            700 * MB,
        );
        let report = build_report_with_inventory(
            &[rec_at(
                "sim-unavailable",
                "/data/home/Library/Developer/CoreSimulator/Devices",
                700 * MB,
                Tier::Safe,
                Some("xcrun simctl delete unavailable"),
                true,
            )],
            DockerBreakdown::default(),
            &root,
            std::path::Path::new("/data/home"),
            |_, _| false,
        );
        let xcode = report
            .sections
            .iter()
            .find(|section| section.section == DeveloperSection::Xcode)
            .unwrap();
        assert_eq!(xcode.findings.len(), 2);
        let devices = xcode
            .findings
            .iter()
            .find(|finding| finding.evidence.measured_path.ends_with("Devices"))
            .unwrap();
        assert_eq!(
            devices.evidence.source_rec_id.as_deref(),
            Some("sim-unavailable")
        );
        assert!(devices.evidence.estimated);
        assert_eq!(
            devices.evidence.command,
            Some("xcrun simctl delete unavailable")
        );
    }

    #[test]
    fn docker_sizes_are_parsed_conservatively() {
        assert_eq!(parse_docker_size("0B"), Some(0));
        assert_eq!(parse_docker_size("3.4kB"), Some(3_400));
        assert_eq!(parse_docker_size("12.5MB"), Some(12_500_000));
        assert_eq!(parse_docker_size("1.2GB"), Some(1_200_000_000));
        assert_eq!(parse_docker_size("1.5GiB"), Some(1_610_612_736));
        assert_eq!(parse_docker_size("-4MB"), None);
        assert_eq!(parse_docker_size("NaNGB"), None);
        assert_eq!(parse_docker_size("999XB"), None);
    }

    #[test]
    fn docker_parser_maps_known_rows_and_omits_malformed_values() {
        let rows = parse_docker_df(
            "Images\t1.2GB\t800MB (66%)\n\
             Containers\t300MB\t20MB (6%)\n\
             Local Volumes\t2GB\t1GB (50%)\n\
             Build Cache\t700MB\t600MB\n\
             Unknown\t9GB\t8GB\n\
             Images\tbad\t2GB\n\
             Containers\t0B\t0B\n",
        );
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Images", "Containers", "Local volumes", "Build cache"]
        );
        assert_eq!(rows[0].bytes, 1_200_000_000);
        assert_eq!(rows[0].reclaimable_bytes, 800_000_000);
        assert_eq!(rows[0].rebuild_cost, RebuildCost::LargeDownload);
        assert_eq!(rows[1].rebuild_cost, RebuildCost::ManualSetup);
        assert_eq!(rows[2].rebuild_cost, RebuildCost::ManualSetup);
        assert_eq!(rows[3].rebuild_cost, RebuildCost::QuickRegeneration);
        assert!(rows.iter().all(|row| !row.counted));
        assert!(rows.iter().all(|row| row.explanation.contains("not added")));
    }

    #[test]
    fn docker_loader_surfaces_failure_timeout_and_non_utf8() {
        let failed = load_docker_with(|| Err("engine unavailable".into()));
        assert!(failed.details.is_empty());
        assert!(failed
            .unavailable
            .as_deref()
            .unwrap()
            .contains("engine unavailable"));

        let timed_out = load_docker_with(|| Err("timed out after 3 seconds".into()));
        assert!(timed_out
            .unavailable
            .as_deref()
            .unwrap()
            .contains("timed out"));

        let non_utf8 = load_docker_with(|| Ok(vec![0xff, 0xfe]));
        assert!(non_utf8
            .unavailable
            .as_deref()
            .unwrap()
            .contains("non-UTF-8"));
    }

    #[test]
    fn docker_command_spec_is_fixed_and_read_only() {
        assert_eq!(
            docker_command_args(),
            [
                "system",
                "df",
                "--format",
                "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
            ]
        );
        assert!(!docker_command_args()
            .iter()
            .any(|arg| matches!(*arg, "prune" | "rm" | "delete")));
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
    fn npm_npx_node_modules_are_package_store_not_a_project() {
        let report = build_report(
            &[rec_at(
                "nm-1",
                "/data/home/.npm/_npx/run/node_modules",
                400,
                Tier::Caution,
                None,
                false,
            )],
            DockerBreakdown::default(),
        );
        assert_eq!(report.sections.len(), 1);
        assert_eq!(report.sections[0].section, DeveloperSection::PackageStores);
        assert_eq!(report.sections[0].findings[0].project_root, None);
    }
}
