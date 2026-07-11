//! The safety knowledge base: inspects the scanned tree and produces the
//! reclaim plan. Same doctrine as the Wails edition:
//!   safe    = regenerates fully automatically (pre-checked in the UI)
//!   caution = costs a re-download / re-install (never pre-checked)
//! User documents/code/media are NEVER recommended.

use crate::scan::{find_dirs_named, lookup, size_of, Node, DATA_ROOT};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Safe,
    Caution,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Trash,
    Delete,
    Command,
    Empty, // delete contents, keep the folder (~/.Trash)
}

#[derive(Clone)]
pub struct Rec {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    pub display: String,
    pub bytes: i64,
    pub tier: Tier,
    pub desc: &'static str,
    pub restore: &'static str,
    pub action: Action,
    pub command: Option<&'static str>,
    pub allow_trash: bool,
    pub allow_delete: bool,
    pub note: String,
    pub estimate: bool,
}

/// Prettify an absolute data-volume path for humans: strip the firmlink
/// prefix, abbreviate the home dir to `~`.
pub fn display(p: &Path) -> String {
    let s = p.to_string_lossy();
    let d = s.strip_prefix(DATA_ROOT).unwrap_or(&s);
    let d = if d.is_empty() { "/" } else { d };
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().into_owned();
        if let Some(rest) = d.strip_prefix(home.as_str()) {
            return format!("~{rest}");
        }
    }
    d.to_string()
}

/// Is a binary installed? Checks `$PATH` first, then the well-known install
/// dirs. The fallback matters because a GUI-launched `.app` inherits launchd's
/// minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), not the login shell's — so
/// tools in `/usr/local/bin`, Homebrew, or Docker Desktop would otherwise read
/// as "not installed" and their reclaim rows would silently vanish.
fn has(bin: &str) -> bool {
    let on_path = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false);
    if on_path {
        return true;
    }
    const WELL_KNOWN: &[&str] = &[
        "/usr/local/bin",                                  // Intel Homebrew, docker symlink
        "/opt/homebrew/bin",                               // Apple-silicon Homebrew
        "/Applications/Docker.app/Contents/Resources/bin", // Docker Desktop
    ];
    WELL_KNOWN
        .iter()
        .any(|dir| Path::new(dir).join(bin).is_file())
}

fn friendly_cache_name(dir: &str) -> Option<&'static str> {
    Some(match dir {
        "Google" => "Chrome caches",
        "BraveSoftware" => "Brave caches",
        "Mozilla" | "Firefox" => "Firefox caches",
        "com.spotify.client" => "Spotify cache",
        "Yarn" => "Yarn cache",
        "typescript" => "TypeScript server cache",
        "node-gyp" => "node-gyp cache",
        "electron" => "Electron download cache",
        _ => return None,
    })
}

fn is_browser_cache(dir: &str) -> bool {
    matches!(dir, "Google" | "BraveSoftware" | "Mozilla" | "Firefox")
}

pub fn build_recommendations(root: &Arc<Node>) -> Vec<Rec> {
    let mut recs: Vec<Rec> = Vec::new();
    let Some(home) = std::env::var_os("HOME") else {
        return recs;
    };
    let h = PathBuf::from(format!("{DATA_ROOT}{}", home.to_string_lossy()));

    let mut add = |mut r: Rec| {
        if r.bytes > 0 {
            r.display = display(&r.path);
            recs.push(r);
        }
    };

    // ── Docker: prune, never trash — the space lives inside the VM image ──
    let docker_dir = h.join("Library/Containers/com.docker.docker");
    let docker_sz = size_of(root, &docker_dir);
    if docker_sz > 500 << 20 && has("docker") {
        add(Rec {
            id: "docker-prune".into(),
            title: "Docker — unused images & build cache".into(),
            path: docker_dir,
            display: String::new(),
            bytes: docker_sz,
            tier: Tier::Safe,
            desc: "Prunes every unused image and the builder cache inside Docker's VM. Running containers and their images are untouched. The Docker.raw disk image shrinks back via TRIM after pruning.",
            restore: "Images re-pull and layers rebuild automatically on your next docker build/run.",
            action: Action::Command,
            command: Some("docker image prune -a -f && docker builder prune -a -f"),
            allow_trash: false,
            allow_delete: false,
            note: "This is reclaimed by Docker itself — it can't be moved to Trash.".into(),
            estimate: true,
        });
    }

    // ── macOS app code-sign clones — the per-user container has a random name,
    // so derive it from $TMPDIR (.../T/) whose sibling `X` holds the clones.
    // Frequent auto-updaters (Chrome) leave stale multi-GB copies behind. ──
    if let Some(tmp) = std::env::var_os("TMPDIR") {
        if let Some(x_dir) = Path::new(&tmp).parent().map(|c| c.join("X")) {
            let x_sz = crate::clean::quick_du(&x_dir);
            if x_sz > 500 << 20 {
                add(Rec {
                    id: "codesign-clones".into(),
                    title: "macOS app code-sign clones".into(),
                    path: x_dir,
                    display: String::new(),
                    bytes: x_sz,
                    tier: Tier::Safe,
                    desc: "Read-only copies macOS makes of an app so it can keep verifying the app's signature while it runs. Frequent auto-updaters (Chrome especially) leave stale multi-GB copies behind.",
                    restore: "Nothing to restore — macOS recreates a fresh clone the next time each app launches.",
                    action: Action::Empty,
                    command: None,
                    allow_trash: false,
                    allow_delete: true,
                    note: "Running apps keep working and simply re-clone on next launch.".into(),
                    estimate: false,
                });
            }
        }
    }

    // ── Dev toolchain caches (command-based where files are write-protected) ──
    struct CmdRule {
        id: &'static str,
        rel: &'static str,
        title: &'static str,
        desc: &'static str,
        restore: &'static str,
        bin: &'static str,
        command: &'static str,
        estimate: bool,
    }
    let cmd_rules = [
        CmdRule { id: "go-modcache", rel: "go/pkg/mod", title: "Go module cache",
            desc: "Downloaded Go module sources. Files are write-protected, so the go tool itself clears them.",
            restore: "Modules re-download on your next go build.",
            bin: "go", command: "go clean -modcache", estimate: false },
        CmdRule { id: "go-buildcache", rel: "Library/Caches/go-build", title: "Go build cache",
            desc: "Compiled package artifacts. Cleared by the go tool.",
            restore: "Rebuilds on next compile (first build slower).",
            bin: "go", command: "go clean -cache", estimate: false },
        CmdRule { id: "npm-cache", rel: ".npm", title: "npm cache",
            desc: "Tarballs npm keeps for offline installs.",
            restore: "Packages re-download on next npm install.",
            bin: "npm", command: "npm cache clean --force", estimate: false },
        CmdRule { id: "brew-cleanup", rel: "Library/Caches/Homebrew", title: "Homebrew downloads & old versions",
            desc: "Old bottles, downloads and outdated formula versions.",
            restore: "Nothing to restore — current installs are untouched.",
            bin: "brew", command: "brew cleanup -s --prune=all", estimate: true },
        CmdRule { id: "sim-unavailable", rel: "Library/Developer/CoreSimulator/Devices", title: "Orphaned iOS simulators",
            desc: "Deletes only simulators orphaned by Xcode upgrades (marked unavailable). Current simulators stay.",
            restore: "Nothing — these can't be booted anyway.",
            bin: "xcrun", command: "xcrun simctl delete unavailable", estimate: true },
    ];
    for r in cmd_rules {
        let p = h.join(r.rel);
        let sz = size_of(root, &p);
        if sz > 0 && has(r.bin) {
            add(Rec {
                id: r.id.into(),
                title: r.title.into(),
                path: p,
                display: String::new(),
                bytes: sz,
                tier: Tier::Safe,
                desc: r.desc,
                restore: r.restore,
                action: Action::Command,
                command: Some(r.command),
                allow_trash: false,
                allow_delete: false,
                note: String::new(),
                estimate: r.estimate,
            });
        }
    }

    // ── Simple cache directories: trash or delete ──
    struct SimpleRule {
        id: &'static str,
        rel: &'static str,
        title: &'static str,
        desc: &'static str,
        restore: &'static str,
        tier: Tier,
        note: &'static str,
    }
    let simple = [
        SimpleRule {
            id: "xcode-derived",
            rel: "Library/Developer/Xcode/DerivedData",
            title: "Xcode DerivedData",
            desc: "Build intermediates and indexes for every Xcode project you've opened.",
            restore: "Xcode rebuilds them on next open (first build slower).",
            tier: Tier::Safe,
            note: "",
        },
        SimpleRule {
            id: "xcode-archives",
            rel: "Library/Developer/Xcode/Archives",
            title: "Xcode Archives",
            desc: "App archives from Product → Archive. Old ones are rarely needed.",
            restore: "Gone for good — keep any you still need for App Store symbolication.",
            tier: Tier::Caution,
            note: "",
        },
        SimpleRule {
            id: "ios-devicesupport",
            rel: "Library/Developer/Xcode/iOS DeviceSupport",
            title: "iOS device support files",
            desc: "Debug symbols copied from every iPhone/iPad version you've ever plugged in.",
            restore: "Re-copied automatically next time you connect a device.",
            tier: Tier::Safe,
            note: "",
        },
        SimpleRule {
            id: "pip-cache",
            rel: "Library/Caches/pip",
            title: "pip cache",
            desc: "Downloaded Python wheels.",
            restore: "Re-download on next pip install.",
            tier: Tier::Safe,
            note: "",
        },
        SimpleRule {
            id: "playwright",
            rel: "Library/Caches/ms-playwright",
            title: "Playwright browsers",
            desc: "Chromium/Firefox/WebKit binaries Playwright tests run against.",
            restore: "npx playwright install brings them back (~1 GB download).",
            tier: Tier::Caution,
            note: "You said you actively use Playwright — left unchecked on purpose.",
        },
        SimpleRule {
            id: "cargo",
            rel: ".cargo/registry",
            title: "Rust cargo registry",
            desc: "Downloaded crate sources.",
            restore: "Re-download on next cargo build.",
            tier: Tier::Safe,
            note: "",
        },
        SimpleRule {
            id: "gradle",
            rel: ".gradle/caches",
            title: "Gradle caches",
            desc: "Dependency jars and build caches.",
            restore: "Re-download on next gradle build.",
            tier: Tier::Safe,
            note: "",
        },
        SimpleRule {
            id: "maven",
            rel: ".m2/repository",
            title: "Maven repository",
            desc: "Downloaded Java dependencies.",
            restore: "Re-download on next mvn build.",
            tier: Tier::Caution,
            note: "",
        },
        SimpleRule {
            id: "cocoapods",
            rel: "Library/Caches/CocoaPods",
            title: "CocoaPods cache",
            desc: "Downloaded pod specs and sources.",
            restore: "Re-download on next pod install.",
            tier: Tier::Safe,
            note: "",
        },
        SimpleRule {
            id: "user-logs",
            rel: "Library/Logs",
            title: "App log files",
            desc: "Diagnostic logs apps have written over time.",
            restore: "Apps just start new logs.",
            tier: Tier::Safe,
            note: "",
        },
        SimpleRule {
            id: "trash",
            rel: ".Trash",
            title: "Trash",
            desc: "Files already deleted, waiting for the bin to be emptied.",
            restore: "Gone for good once emptied.",
            tier: Tier::Safe,
            note: "",
        },
    ];
    for r in simple {
        let p = h.join(r.rel);
        let mut sz = size_of(root, &p);
        if sz <= 0 && r.id == "trash" {
            // ~/.Trash is often unreadable to the scanner without Full Disk
            // Access; measure directly in case the app has it
            sz = crate::clean::quick_du(&strip_data_root(&p));
        }
        if sz <= 0 {
            continue;
        }
        let (action, allow_trash) = if r.id == "trash" {
            (Action::Empty, false)
        } else {
            (Action::Trash, true)
        };
        add(Rec {
            id: r.id.into(),
            title: r.title.into(),
            path: p,
            display: String::new(),
            bytes: sz,
            tier: r.tier,
            desc: r.desc,
            restore: r.restore,
            action,
            command: None,
            allow_trash,
            allow_delete: true,
            note: r.note.into(),
            estimate: false,
        });
    }

    // ── Generic ~/Library/Caches entries ≥ 50 MB, minus specifically-ruled ──
    let skip = ["pip", "ms-playwright", "Homebrew", "CocoaPods", "go-build"];
    if let Some(caches) = lookup(root, &h.join("Library/Caches")) {
        for c in caches.kids() {
            if !c.is_dir || c.bytes() < 50 << 20 || skip.contains(&c.name.as_str()) {
                continue;
            }
            let title = friendly_cache_name(&c.name)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} cache", c.name));
            let note = if is_browser_cache(&c.name) {
                "Quit the browser first so it doesn't fight the cleanup.".to_string()
            } else {
                String::new()
            };
            add(Rec {
                id: format!("cache-{}", c.name),
                title,
                path: c.path.clone(),
                display: String::new(),
                bytes: c.bytes(),
                tier: Tier::Safe,
                desc: "Application cache — rebuilt automatically as the app runs.",
                restore: "The app recreates it on demand.",
                action: Action::Trash,
                command: None,
                allow_trash: true,
                allow_delete: true,
                note,
                estimate: false,
            });
        }
    }

    // ── node_modules sprawl (top 15 by size, projects only) ──
    let mut count = 0;
    for n in find_dirs_named(root, "node_modules") {
        if n.bytes() < 20 << 20 || count >= 15 {
            continue;
        }
        if n.path.to_string_lossy().contains("/Library/") {
            continue; // app-internal, not your projects
        }
        count += 1;
        let project = n
            .path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        add(Rec {
            id: format!("nm-{count}"),
            title: format!("node_modules — {project}"),
            path: n.path.clone(),
            display: String::new(),
            bytes: n.bytes(),
            tier: Tier::Caution,
            desc: "Installed npm dependencies for this project.",
            restore: "cd into the project and npm install (or pnpm/yarn) to bring it back.",
            action: Action::Delete,
            command: None,
            allow_trash: true,
            allow_delete: true,
            note: "Only clear projects you're not actively running.".into(),
            estimate: false,
        });
    }

    recs.sort_by(|a, b| {
        let tier = |t: Tier| if t == Tier::Safe { 0 } else { 1 };
        tier(a.tier).cmp(&tier(b.tier)).then(b.bytes.cmp(&a.bytes))
    });
    recs
}

/// Recs carry data-volume paths (`/System/Volumes/Data/...`); filesystem
/// operations want the firmlinked view (`/Users/...`). Both work, but the
/// short form matches what users see in Finder.
pub fn strip_data_root(p: &Path) -> PathBuf {
    p.strip_prefix(DATA_ROOT)
        .map(|r| PathBuf::from("/").join(r))
        .unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Node;
    use std::sync::atomic::Ordering::Relaxed;

    fn dir(parent: &Arc<Node>, path: &str, bytes: i64) -> Arc<Node> {
        let p = PathBuf::from(path);
        let child = Arc::new(Node {
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            path: p,
            is_dir: true,
            bytes: std::sync::atomic::AtomicI64::new(bytes),
            files: std::sync::atomic::AtomicI64::new(1),
            small_bytes: std::sync::atomic::AtomicI64::new(0),
            small_count: std::sync::atomic::AtomicI64::new(0),
            denied: std::sync::atomic::AtomicBool::new(false),
            parent: Arc::downgrade(parent),
            children: std::sync::Mutex::new(Vec::new()),
        });
        parent.children.lock().unwrap().push(child.clone());
        child
    }

    fn root_node() -> Arc<Node> {
        Arc::new(Node {
            name: "Data".into(),
            path: PathBuf::from(DATA_ROOT),
            is_dir: true,
            bytes: std::sync::atomic::AtomicI64::new(0),
            files: std::sync::atomic::AtomicI64::new(0),
            small_bytes: std::sync::atomic::AtomicI64::new(0),
            small_count: std::sync::atomic::AtomicI64::new(0),
            denied: std::sync::atomic::AtomicBool::new(false),
            parent: std::sync::Weak::new(),
            children: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Build a synthetic tree mirroring a home dir. Every path segment needs
    /// its own node — lookup() walks segment by segment.
    fn fake_tree() -> Arc<Node> {
        let home = std::env::var("HOME").unwrap();
        let h = format!("{DATA_ROOT}{home}");
        let root = root_node();
        let users = dir(&root, &format!("{DATA_ROOT}/Users"), 0);
        let hd = dir(&users, &h, 0);
        let lib = dir(&hd, &format!("{h}/Library"), 0);
        let caches = dir(&lib, &format!("{h}/Library/Caches"), 0);
        dir(&caches, &format!("{h}/Library/Caches/pip"), 200 << 20);
        dir(
            &caches,
            &format!("{h}/Library/Caches/ms-playwright"),
            1 << 30,
        );
        dir(
            &caches,
            &format!("{h}/Library/Caches/BigToolCache"),
            60 << 20,
        );
        dir(&caches, &format!("{h}/Library/Caches/SmallCache"), 10 << 20);
        let devx = dir(&lib, &format!("{h}/Library/Developer"), 0);
        let xc = dir(&devx, &format!("{h}/Library/Developer/Xcode"), 0);
        dir(
            &xc,
            &format!("{h}/Library/Developer/Xcode/DerivedData"),
            2 << 30,
        );
        let some_app = dir(&lib, &format!("{h}/Library/SomeApp"), 0);
        dir(
            &some_app,
            &format!("{h}/Library/SomeApp/node_modules"),
            100 << 20,
        );
        dir(&hd, &format!("{h}/.Trash"), 500 << 20);
        let work = dir(&hd, &format!("{h}/work"), 0);
        let pa = dir(&work, &format!("{h}/work/projA"), 0);
        dir(&pa, &format!("{h}/work/projA/node_modules"), 300 << 20);
        let pb = dir(&work, &format!("{h}/work/projB"), 0);
        dir(&pb, &format!("{h}/work/projB/node_modules"), 30 << 20);
        root
    }

    fn by_id<'a>(recs: &'a [Rec], id: &str) -> Option<&'a Rec> {
        recs.iter().find(|r| r.id == id)
    }

    #[test]
    fn knowledge_base_doctrine() {
        let root = fake_tree();
        let recs = build_recommendations(&root);

        let dd = by_id(&recs, "xcode-derived").expect("DerivedData rec");
        assert!(dd.tier == Tier::Safe && dd.action == Action::Trash && dd.allow_delete);

        let pw = by_id(&recs, "playwright").expect("playwright rec");
        assert!(pw.tier == Tier::Caution && !pw.note.is_empty());

        let tr = by_id(&recs, "trash").expect("trash rec");
        assert!(tr.action == Action::Empty && !tr.allow_trash);

        assert!(by_id(&recs, "cache-BigToolCache").is_some());
        assert!(
            by_id(&recs, "cache-pip").is_none(),
            "pip must not double-report"
        );
        assert!(by_id(&recs, "cache-SmallCache").is_none(), "<50MB excluded");

        let nm: Vec<_> = recs.iter().filter(|r| r.id.starts_with("nm-")).collect();
        assert_eq!(nm.len(), 2, "Library node_modules excluded, projects kept");
        for r in &nm {
            assert!(r.tier == Tier::Caution);
            assert!(!r.path.to_string_lossy().contains("/Library/"));
        }

        // safe recs sort before caution recs
        let first_caution = recs.iter().position(|r| r.tier == Tier::Caution);
        let last_safe = recs.iter().rposition(|r| r.tier == Tier::Safe);
        if let (Some(fc), Some(ls)) = (first_caution, last_safe) {
            assert!(ls < fc, "safe before caution");
        }

        for r in &recs {
            assert!(!r.desc.is_empty() && !r.restore.is_empty() && !r.display.is_empty());
        }
        let _ = root.bytes.load(Relaxed);
    }

    #[test]
    fn display_prettifies() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(display(Path::new(DATA_ROOT)), "/");
        assert_eq!(
            display(&PathBuf::from(format!("{DATA_ROOT}{home}/go/pkg"))),
            "~/go/pkg"
        );
        assert_eq!(
            display(&PathBuf::from(format!("{DATA_ROOT}/private/var/log"))),
            "/private/var/log"
        );
    }
}
