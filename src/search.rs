//! Instant, read-only search over a completed compact scan tree.

use crate::scan::Node;
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;

pub const DEFAULT_RESULT_LIMIT: usize = 80;
pub const DEFAULT_LARGEST_FILE_LIMIT: usize = 80;
pub const DEFAULT_COVERAGE_LIMIT: usize = 60;

#[derive(Clone)]
pub struct SearchResult {
    pub node: Arc<Node>,
    pub display_path: String,
    rank: MatchRank,
}

#[derive(Clone, Default)]
pub struct SearchSummary {
    pub total_matches: usize,
    pub results: Vec<SearchResult>,
}

pub struct LargestFile {
    pub node: Arc<Node>,
    pub display_path: String,
}

#[derive(Default)]
pub struct LargestFilesSummary {
    pub total_files: usize,
    pub results: Vec<LargestFile>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CoverageKind {
    Personal,
    System,
}

pub struct DeniedLocation {
    pub path: std::path::PathBuf,
    pub display_path: String,
    pub kind: CoverageKind,
}

#[derive(Default)]
pub struct ScanCoverageSummary {
    pub total_denied: usize,
    pub personal_denied: usize,
    pub system_denied: usize,
    pub results: Vec<DeniedLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MatchRank {
    ExactName,
    NamePrefix,
    NameContains,
    PathOnly,
}

fn match_rank(name: &str, path: &str, query: &str) -> Option<MatchRank> {
    let terms: Vec<_> = query.split_whitespace().collect();
    if terms.is_empty() || !terms.iter().all(|term| path.contains(term)) {
        return None;
    }
    if name == query {
        Some(MatchRank::ExactName)
    } else if name.starts_with(query) {
        Some(MatchRank::NamePrefix)
    } else if name.contains(query) {
        Some(MatchRank::NameContains)
    } else {
        Some(MatchRank::PathOnly)
    }
}

pub fn search_tree(root: &Arc<Node>, query: &str, limit: usize) -> SearchSummary {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 2 {
        return SearchSummary::default();
    }

    let mut matches = Vec::new();
    let mut stack = root.kids();
    while let Some(node) = stack.pop() {
        if node.is_dir {
            stack.extend(node.kids());
        }
        let display_path = node.path.to_string_lossy().into_owned();
        let lower_path = display_path.to_lowercase();
        let lower_name = node.name.to_lowercase();
        if let Some(rank) = match_rank(&lower_name, &lower_path, &query) {
            matches.push(SearchResult {
                node,
                display_path,
                rank,
            });
        }
    }

    matches.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| right.node.bytes().cmp(&left.node.bytes()))
            .then_with(|| {
                left.node
                    .path
                    .as_os_str()
                    .as_bytes()
                    .cmp(right.node.path.as_os_str().as_bytes())
            })
    });
    let total_matches = matches.len();
    matches.truncate(limit.min(DEFAULT_RESULT_LIMIT));
    SearchSummary {
        total_matches,
        results: matches,
    }
}

pub fn largest_files(root: &Arc<Node>, limit: usize) -> LargestFilesSummary {
    let mut files = Vec::new();
    let mut stack = root.kids();
    while let Some(node) = stack.pop() {
        if node.is_dir {
            stack.extend(node.kids());
        } else {
            files.push(LargestFile {
                display_path: node.path.to_string_lossy().into_owned(),
                node,
            });
        }
    }

    files.sort_by(|left, right| {
        right.node.bytes().cmp(&left.node.bytes()).then_with(|| {
            left.node
                .path
                .as_os_str()
                .as_bytes()
                .cmp(right.node.path.as_os_str().as_bytes())
        })
    });
    let total_files = files.len();
    files.truncate(limit.min(DEFAULT_LARGEST_FILE_LIMIT));
    LargestFilesSummary {
        total_files,
        results: files,
    }
}

fn coverage_kind(root: &std::path::Path, path: &std::path::Path) -> CoverageKind {
    let Ok(relative) = path.strip_prefix(root) else {
        return CoverageKind::System;
    };
    let mut components = relative.components();
    let users = components.next().map(|part| part.as_os_str());
    let account = components.next().map(|part| part.as_os_str());
    if users == Some(std::ffi::OsStr::new("Users"))
        && account.is_some()
        && account != Some(std::ffi::OsStr::new("Shared"))
    {
        CoverageKind::Personal
    } else {
        CoverageKind::System
    }
}

pub fn scan_coverage(root: &Arc<Node>, limit: usize) -> ScanCoverageSummary {
    let mut denied = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        if !node.is_dir {
            continue;
        }
        stack.extend(node.kids());
        if node.denied.load(std::sync::atomic::Ordering::Relaxed) {
            let kind = coverage_kind(&root.path, &node.path);
            denied.push(DeniedLocation {
                path: node.path.clone(),
                display_path: node.path.to_string_lossy().into_owned(),
                kind,
            });
        }
    }

    denied.sort_by(|left, right| {
        left.kind.cmp(&right.kind).then_with(|| {
            left.path
                .as_os_str()
                .as_bytes()
                .cmp(right.path.as_os_str().as_bytes())
        })
    });
    let personal_denied = denied
        .iter()
        .filter(|location| location.kind == CoverageKind::Personal)
        .count();
    let system_denied = denied.len().saturating_sub(personal_denied);
    let total_denied = denied.len();
    denied.truncate(limit.min(DEFAULT_COVERAGE_LIMIT));
    ScanCoverageSummary {
        total_denied,
        personal_denied,
        system_denied,
        results: denied,
    }
}

pub fn crumbs_for(root: &Arc<Node>, target: &Arc<Node>) -> Option<Vec<Arc<Node>>> {
    let mut current = target.clone();
    let mut reversed = Vec::new();
    for _ in 0..4096 {
        if Arc::ptr_eq(&current, root) {
            reversed.reverse();
            return Some(reversed);
        }
        reversed.push(current.clone());
        current = current.parent.upgrade()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Node;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicI64};
    use std::sync::{Arc, Mutex, Weak};

    fn root(path: &str) -> Arc<Node> {
        let path = PathBuf::from(path);
        Arc::new(Node {
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path,
            is_dir: true,
            bytes: AtomicI64::new(90_000_000_000),
            files: AtomicI64::new(12_000),
            small_bytes: AtomicI64::new(0),
            small_count: AtomicI64::new(0),
            denied: AtomicBool::new(false),
            parent: Weak::new(),
            children: Mutex::new(Vec::new()),
        })
    }

    fn child(parent: &Arc<Node>, name: &str, is_dir: bool, bytes: i64, denied: bool) -> Arc<Node> {
        let node = Arc::new(Node {
            name: name.into(),
            path: parent.path.join(name),
            is_dir,
            bytes: AtomicI64::new(bytes),
            files: AtomicI64::new(if is_dir { 20 } else { 1 }),
            small_bytes: AtomicI64::new(0),
            small_count: AtomicI64::new(0),
            denied: AtomicBool::new(denied),
            parent: Arc::downgrade(parent),
            children: Mutex::new(Vec::new()),
        });
        parent.children.lock().unwrap().push(node.clone());
        node
    }

    struct Fixture {
        root: Arc<Node>,
        users: Arc<Node>,
        project: Arc<Node>,
        exact: Arc<Node>,
        prefix: Arc<Node>,
        nested: Arc<Node>,
        path_only: Arc<Node>,
    }

    fn fixture() -> Fixture {
        let root = root("/System/Volumes/Data");
        let users = child(&root, "Users", true, 40_000_000_000, false);
        let project = child(&users, "WardenUI", true, 5_000_000_000, false);
        let exact = child(&project, "node_modules", true, 2_000_000_000, false);
        let prefix = child(&project, "node_modules-old", true, 3_000_000_000, false);
        let nested = child(
            &users,
            "archive-node_modules-copy",
            true,
            4_000_000_000,
            false,
        );
        let path_only = child(&exact, "cache", true, 1_000_000_000, false);
        Fixture {
            root,
            users,
            project,
            exact,
            prefix,
            nested,
            path_only,
        }
    }

    #[test]
    fn storage_search_ranks_exact_prefix_name_and_path_matches() {
        let fixture = fixture();
        let summary = search_tree(&fixture.root, "node_modules", DEFAULT_RESULT_LIMIT);
        let paths: Vec<_> = summary
            .results
            .iter()
            .map(|result| result.node.path.clone())
            .collect();
        assert_eq!(summary.total_matches, 4);
        assert_eq!(
            paths,
            vec![
                fixture.exact.path.clone(),
                fixture.prefix.path.clone(),
                fixture.nested.path.clone(),
                fixture.path_only.path.clone(),
            ]
        );
        assert!(summary.results[0].display_path.ends_with("node_modules"));
    }

    #[test]
    fn storage_search_requires_all_terms_and_ignores_short_queries() {
        let fixture = fixture();
        let summary = search_tree(&fixture.root, "  WARDEN   node  ", DEFAULT_RESULT_LIMIT);
        assert_eq!(summary.total_matches, 3);
        assert!(summary.results.iter().all(|result| {
            result.display_path.to_lowercase().contains("warden")
                && result.display_path.to_lowercase().contains("node")
        }));
        assert_eq!(search_tree(&fixture.root, "n", 80).total_matches, 0);
        assert_eq!(search_tree(&fixture.root, "   ", 80).total_matches, 0);
    }

    #[test]
    fn storage_search_caps_rows_but_reports_every_match() {
        let fixture = fixture();
        let summary = search_tree(&fixture.root, "node_modules", 2);
        assert_eq!(summary.total_matches, 4);
        assert_eq!(summary.results.len(), 2);
    }

    #[test]
    fn storage_search_is_stable_and_preserves_denied_nodes() {
        let fixture = fixture();
        let denied = child(&fixture.users, "SecretCache", true, 600_000_000, true);
        let denied_summary = search_tree(&fixture.root, "secret", 80);
        assert_eq!(denied_summary.results.len(), 1);
        assert!(Arc::ptr_eq(&denied_summary.results[0].node, &denied));
        assert!(denied_summary.results[0]
            .node
            .denied
            .load(std::sync::atomic::Ordering::Relaxed));

        let alpha_mirror = child(&fixture.users, "alpha-mirror", true, 500, false);
        let alpha_copy = child(&fixture.users, "alpha-copy", true, 500, false);
        let stable = search_tree(&fixture.root, "alpha", 80);
        assert_eq!(stable.results.len(), 2);
        assert!(Arc::ptr_eq(&stable.results[0].node, &alpha_copy));
        assert!(Arc::ptr_eq(&stable.results[1].node, &alpha_mirror));
        assert!(stable
            .results
            .iter()
            .all(|result| !Arc::ptr_eq(&result.node, &fixture.root)));
    }

    #[test]
    fn storage_search_reconstructs_only_attached_breadcrumbs() {
        let fixture = fixture();
        let crumbs = crumbs_for(&fixture.root, &fixture.exact).unwrap();
        assert_eq!(
            crumbs
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Users", "WardenUI", "node_modules"]
        );
        assert!(crumbs_for(&fixture.root, &fixture.root).unwrap().is_empty());
        let unrelated = root("/fixture/OtherRoot");
        assert!(crumbs_for(&fixture.root, &unrelated).is_none());
        assert_eq!(fixture.project.name, "WardenUI");
    }

    #[test]
    fn storage_search_breadcrumbs_fail_closed_on_a_cycle() {
        let cyclic = Arc::new_cyclic(|weak| Node {
            name: "cycle".into(),
            path: PathBuf::from("/cycle"),
            is_dir: true,
            bytes: AtomicI64::new(1),
            files: AtomicI64::new(0),
            small_bytes: AtomicI64::new(0),
            small_count: AtomicI64::new(0),
            denied: AtomicBool::new(false),
            parent: weak.clone(),
            children: Mutex::new(Vec::new()),
        });
        assert!(crumbs_for(&fixture().root, &cyclic).is_none());
    }

    #[test]
    fn largest_files_keeps_only_files_and_orders_size_then_raw_path() {
        let fixture = fixture();
        let biggest = child(&fixture.project, "movie.mov", false, 900_000_000, false);
        let tie_z = child(&fixture.users, "zeta.dmg", false, 500_000_000, false);
        let tie_a = child(&fixture.users, "alpha.dmg", false, 500_000_000, false);
        let _larger_directory = child(&fixture.users, "not-a-file", true, 9_000_000_000, false);

        let summary = largest_files(&fixture.root, DEFAULT_LARGEST_FILE_LIMIT);

        assert_eq!(summary.total_files, 3);
        assert_eq!(summary.results.len(), 3);
        assert!(Arc::ptr_eq(&summary.results[0].node, &biggest));
        assert!(Arc::ptr_eq(&summary.results[1].node, &tie_a));
        assert!(Arc::ptr_eq(&summary.results[2].node, &tie_z));
        assert!(summary.results[0].display_path.ends_with("movie.mov"));
    }

    #[test]
    fn largest_files_caps_rows_reports_total_and_handles_empty_maps() {
        let fixture = fixture();
        for index in 0..(DEFAULT_LARGEST_FILE_LIMIT + 2) {
            child(
                &fixture.users,
                &format!("file-{index:03}.bin"),
                false,
                200_000_000 + index as i64,
                false,
            );
        }

        let capped = largest_files(&fixture.root, DEFAULT_LARGEST_FILE_LIMIT + 20);
        assert_eq!(capped.total_files, DEFAULT_LARGEST_FILE_LIMIT + 2);
        assert_eq!(capped.results.len(), DEFAULT_LARGEST_FILE_LIMIT);

        let one = largest_files(&fixture.root, 1);
        assert_eq!(one.total_files, DEFAULT_LARGEST_FILE_LIMIT + 2);
        assert_eq!(one.results.len(), 1);

        let empty = largest_files(&root("/empty"), DEFAULT_LARGEST_FILE_LIMIT);
        assert_eq!(empty.total_files, 0);
        assert!(empty.results.is_empty());
    }

    #[test]
    fn scan_coverage_classifies_personal_paths_and_orders_raw_paths() {
        let fixture = fixture();
        let alice = child(&fixture.users, "alice", true, 1, false);
        let personal_z = child(&alice, "Documents", true, 0, true);
        let personal_a = child(&alice, "Desktop", true, 0, true);
        let shared = child(&fixture.users, "Shared", true, 0, true);
        let system = child(&fixture.root, ".Spotlight-V100", true, 0, true);
        let _readable = child(&fixture.root, "Library", true, 1, false);
        let _denied_file = child(&fixture.root, "locked.bin", false, 200_000_000, true);

        let summary = scan_coverage(&fixture.root, DEFAULT_COVERAGE_LIMIT);

        assert_eq!(summary.total_denied, 4);
        assert_eq!(summary.personal_denied, 2);
        assert_eq!(summary.system_denied, 2);
        assert_eq!(summary.results.len(), 4);
        assert_eq!(summary.results[0].kind, CoverageKind::Personal);
        assert_eq!(summary.results[0].path, personal_a.path);
        assert_eq!(summary.results[1].path, personal_z.path);
        assert_eq!(summary.results[2].kind, CoverageKind::System);
        assert_eq!(summary.results[2].path, system.path);
        assert_eq!(summary.results[3].path, shared.path);
        assert!(summary.results[0].display_path.ends_with("Desktop"));
    }

    #[test]
    fn scan_coverage_caps_rows_reports_every_denial_and_handles_empty_maps() {
        let fixture = fixture();
        for index in 0..(DEFAULT_COVERAGE_LIMIT + 2) {
            child(&fixture.root, &format!("locked-{index:03}"), true, 0, true);
        }

        let capped = scan_coverage(&fixture.root, DEFAULT_COVERAGE_LIMIT + 20);
        assert_eq!(capped.total_denied, DEFAULT_COVERAGE_LIMIT + 2);
        assert_eq!(capped.system_denied, DEFAULT_COVERAGE_LIMIT + 2);
        assert_eq!(capped.results.len(), DEFAULT_COVERAGE_LIMIT);

        let empty = scan_coverage(&root("/empty"), DEFAULT_COVERAGE_LIMIT);
        assert_eq!(empty.total_denied, 0);
        assert!(empty.results.is_empty());
    }
}
