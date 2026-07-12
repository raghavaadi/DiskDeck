//! The DiskDeck application: top bar, capacity gauge, scan telemetry,
//! live terrain map, reclaim plan with hold-to-reclaim, ops feed.

use crate::apfs::{self, ApfsAccounting};
use crate::clean::{
    fmt_bytes, fmt_count, open_full_disk_access, open_trash, reveal_in_finder, run_clean,
    CleanEvent, CleanJob,
};
use crate::developer;
use crate::file_review::{self, ReviewResult};
use crate::forecast::{self, Confidence, ForecastState};
use crate::history::{
    default_history_dir, load_growth_watch, record_scan, set_folder_watched, GrowthSummary,
    GrowthWatch, HistoryEvent, TimelinePoint,
};
use crate::leftovers::{self, LeftoverFinding};
use crate::monitor::{self, MenuBarItem, MonitorSettings};
use crate::moves::{
    can_confirm_restore, refresh_records, registry_path_for_home, restore_block, run_restore,
    state_reason, MoveState, MovedItem, RestoreBlock, RestoreEvent, RestoreJob, RestoreRoots,
};
use crate::offload::{
    can_confirm_offload, check_movable, classify_movable, external_volumes, has_room, run_offload,
    OffloadEvent, OffloadJob, Volume,
};
use crate::reclaim_history::{
    can_confirm_restore as receipt_can_confirm_restore, history_path_for_home, now_ms,
    refresh_history, run_restore as run_trash_restore, ReceiptAction, ReceiptItem, ReceiptState,
    ReclaimHistory, RestoreEvent as TrashRestoreEvent, RestoreJob as TrashRestoreJob,
};
use crate::reclaim_plan::{
    build_plan, parse_goal_gb, GoalError, OutcomeTracker, ReclaimOutcome, ReclaimPlan, GB,
};
use crate::rules::{self, strip_data_root, Action, Rec, Tier};
use crate::scan::{disk_stats, start_scan, DiskStats, Node, ScanHandle, ScanState, DATA_ROOT};
use crate::theme;
use crate::treemap;
use egui::{
    pos2, vec2, Align2, Color32, Context, Frame, Label, Margin, Pos2, Rect, RichText, Rounding,
    Sense, Stroke,
};
use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HOLD_SECS: f32 = 0.9;
const ZOOM_SECS: f32 = 0.28;

#[derive(Clone)]
enum RecStatus {
    Idle,
    Running,
    Cleared(i64),
    InTrash(i64),
    Failed(String),
}

struct RecRow {
    rec: Rec,
    checked: bool,
    action: Action,
    expanded: bool,
    status: RecStatus,
}

#[derive(Clone, Copy)]
enum OpsKind {
    Info,
    Ok,
    Err,
    Dim,
    Amber,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeveloperWorkspaceState {
    WaitingForScan,
    Loading,
    Empty,
    Partial,
    DockerUnavailable,
    Populated,
    Failed,
}

fn developer_workspace_state(
    report: Option<&developer::DeveloperReport>,
    loading: bool,
    error: Option<&str>,
    scan_ready: bool,
) -> DeveloperWorkspaceState {
    if !scan_ready {
        DeveloperWorkspaceState::WaitingForScan
    } else if loading {
        DeveloperWorkspaceState::Loading
    } else if error.is_some() {
        DeveloperWorkspaceState::Failed
    } else if let Some(report) = report {
        if report.sections.is_empty() {
            DeveloperWorkspaceState::Empty
        } else if report.docker.unavailable.is_some() {
            DeveloperWorkspaceState::DockerUnavailable
        } else if report.sections.len() >= 3 {
            DeveloperWorkspaceState::Populated
        } else {
            DeveloperWorkspaceState::Partial
        }
    } else {
        DeveloperWorkspaceState::Empty
    }
}

fn developer_workspace_copy(state: DeveloperWorkspaceState) -> &'static str {
    match state {
        DeveloperWorkspaceState::WaitingForScan => {
            "Complete the foreground scan to inspect developer storage."
        }
        DeveloperWorkspaceState::Loading => {
            "Reading retained scan evidence and fixed read-only tool output…"
        }
        DeveloperWorkspaceState::Empty => {
            "No developer storage passed the bounded evidence checks."
        }
        DeveloperWorkspaceState::Partial => {
            "Developer evidence is available from part of this Mac."
        }
        DeveloperWorkspaceState::DockerUnavailable => {
            "Measured storage is available; Docker's inside-VM detail is unavailable."
        }
        DeveloperWorkspaceState::Populated => {
            "Developer storage is grouped by source with overlap-safe totals."
        }
        DeveloperWorkspaceState::Failed => "Developer workspace could not be loaded.",
    }
}

fn should_start_developer_worker(
    opening_developer_lens: bool,
    scan_ready: bool,
    worker_running: bool,
    has_report: bool,
) -> bool {
    opening_developer_lens && scan_ready && !worker_running && !has_report
}

fn invalidate_developer_workspace(
    report: &mut Option<developer::DeveloperReport>,
    error: &mut Option<String>,
) {
    *report = None;
    *error = None;
}

struct OpsLine {
    time: String,
    text: String,
    kind: OpsKind,
}

struct OffloadDialog {
    src: std::path::PathBuf,
    size: i64,
    vols: Vec<Volume>,
    vol_idx: usize,
    leave_symlink: bool,
    acknowledged: bool,
    show_info: bool,
    reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RailView {
    Summary,
    GuidedReclaim,
    Reclaim,
    Insights,
    Moved,
    Growth,
    Developer,
    Apfs,
    Leftovers,
    Monitor,
    FileReview,
    ReclaimHistory,
}

fn rail_back_target(view: RailView) -> Option<RailView> {
    match view {
        RailView::Summary => None,
        RailView::GuidedReclaim | RailView::Reclaim | RailView::Insights => Some(RailView::Summary),
        RailView::Moved
        | RailView::Growth
        | RailView::Developer
        | RailView::Apfs
        | RailView::Leftovers
        | RailView::Monitor
        | RailView::FileReview
        | RailView::ReclaimHistory => Some(RailView::Insights),
    }
}

struct RestoreDialog {
    item: MovedItem,
    acknowledged: bool,
    block: Option<RestoreBlock>,
}

struct TrashRestoreDialog {
    item: ReceiptItem,
    acknowledged: bool,
}

pub struct App {
    scan: Option<ScanHandle>,
    view: Option<Arc<Node>>,
    crumbs: Vec<Arc<Node>>,
    zoom: Option<(Rect, Instant)>,
    recs: Vec<RecRow>,
    recs_built: bool,
    recs_revision: u64,
    guide_goal_bytes: i64,
    guide_custom_gb: String,
    guide_goal_error: Option<GoalError>,
    guide_acknowledged: bool,
    guide_revision: Option<u64>,
    guided_goal_for_review: Option<i64>,
    active_guided_reclaim: Option<OutcomeTracker>,
    guided_outcome: Option<ReclaimOutcome>,
    clean_rx: Option<Receiver<CleanEvent>>,
    cleaning: bool,
    hold: f32,
    ops: Vec<OpsLine>,
    stats: DiskStats,
    stats_at: Instant,
    stamp: Option<(String, Instant)>,
    booted: bool,
    offload_rx: Option<Receiver<OffloadEvent>>,
    offloading: bool,
    dialog: Option<OffloadDialog>,
    dialog_hold: f32,
    rail_view: RailView,
    activity_open: bool,
    history_rx: Option<Receiver<HistoryEvent>>,
    regrowth: Option<GrowthSummary>,
    history_baseline: bool,
    moves_rx: Option<Receiver<Result<Vec<MovedItem>, String>>>,
    moved_items: Vec<MovedItem>,
    moves_error: Option<String>,
    restore_rx: Option<Receiver<RestoreEvent>>,
    restoring: bool,
    restore_dialog: Option<RestoreDialog>,
    restore_hold: f32,
    reclaim_history_rx: Option<Receiver<Result<ReclaimHistory, String>>>,
    reclaim_history: ReclaimHistory,
    reclaim_history_error: Option<String>,
    trash_restore_rx: Option<Receiver<TrashRestoreEvent>>,
    restoring_trash: bool,
    trash_restore_dialog: Option<TrashRestoreDialog>,
    trash_restore_hold: f32,
    growth_watch_rx: Option<Receiver<Result<GrowthWatch, String>>>,
    growth_watch: GrowthWatch,
    growth_watch_error: Option<String>,
    developer_rx: Option<Receiver<Result<developer::DeveloperReport, String>>>,
    developer_report: Option<developer::DeveloperReport>,
    developer_error: Option<String>,
    apfs_rx: Option<Receiver<Result<ApfsAccounting, String>>>,
    apfs: Option<ApfsAccounting>,
    apfs_error: Option<String>,
    leftovers_rx: Option<Receiver<Result<Vec<LeftoverFinding>, String>>>,
    leftovers: Vec<LeftoverFinding>,
    leftovers_error: Option<String>,
    monitor_settings: MonitorSettings,
    menu_bar_item: Option<MenuBarItem>,
    monitor_updated_at: Instant,
    monitor_low: bool,
    monitor_error: Option<String>,
    file_review_rx: Option<Receiver<Result<ReviewResult, String>>>,
    file_review_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    file_review: Option<ReviewResult>,
    file_review_error: Option<String>,
}

fn now_hms() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

fn tail_str(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let tail: String = chars[chars.len() - max..].iter().collect();
    format!("…{tail}")
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn should_record_history(state: ScanState) -> bool {
    state == ScanState::Done
}

fn fmt_delta(bytes: i64) -> String {
    if bytes > 0 {
        format!("+{}", fmt_bytes(bytes))
    } else if bytes < 0 {
        format!("−{}", fmt_bytes(bytes.saturating_abs()))
    } else {
        "no change".to_string()
    }
}

fn fmt_percent_tenths(value: Option<i64>) -> String {
    match value {
        Some(value) => format!("{:+.1}%", value as f64 / 10.0),
        None => "new".into(),
    }
}

fn draw_growth_sparkline(ui: &egui::Ui, rect: Rect, points: &[TimelinePoint]) {
    let palette = theme::palette(ui.ctx());
    ui.painter()
        .rect_filled(rect, Rounding::same(8.0), palette.surface);
    ui.painter().rect_stroke(
        rect,
        Rounding::same(8.0),
        Stroke::new(1.0, palette.edge_soft),
    );
    if points.is_empty() {
        return;
    }
    let plot = Rect::from_min_max(rect.min + vec2(9.0, 9.0), rect.max - vec2(9.0, 22.0));
    let min = points
        .iter()
        .map(|point| point.total_bytes)
        .min()
        .unwrap_or(0);
    let max = points
        .iter()
        .map(|point| point.total_bytes)
        .max()
        .unwrap_or(min);
    let span = (max - min).max(1) as f32;
    let denom = points.len().saturating_sub(1).max(1) as f32;
    let line: Vec<Pos2> = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            pos2(
                egui::lerp(plot.x_range(), index as f32 / denom),
                plot.max.y - (point.total_bytes - min) as f32 / span * plot.height(),
            )
        })
        .collect();
    if line.len() > 1 {
        ui.painter().add(egui::Shape::line(
            line.clone(),
            Stroke::new(2.0, palette.accent),
        ));
    }
    for point in line {
        ui.painter().circle_filled(point, 2.5, palette.accent);
    }
    ui.painter().text(
        rect.min + vec2(9.0, rect.height() - 8.0),
        Align2::LEFT_CENTER,
        format!("oldest {}", fmt_bytes(points[0].total_bytes)),
        theme::mono(8.5),
        palette.faint,
    );
    ui.painter().text(
        rect.max - vec2(9.0, 8.0),
        Align2::RIGHT_CENTER,
        format!("latest {}", fmt_bytes(points.last().unwrap().total_bytes)),
        theme::mono(8.5),
        palette.muted,
    );
}

fn developer_tier_copy(tier: developer::EvidenceTier) -> &'static str {
    match tier {
        developer::EvidenceTier::Safe => "Safe rule",
        developer::EvidenceTier::Caution => "Caution · never preselected",
    }
}

fn docker_footprint_copy(measured: bool) -> &'static str {
    if measured {
        "Measured on disk; inside-VM categories are explanatory and uncounted."
    } else {
        "Docker's on-disk footprint was not measured in this scan; inside-VM categories remain explanatory and uncounted."
    }
}

fn draw_developer_finding(
    ui: &mut egui::Ui,
    finding: &developer::DeepFinding,
) -> Option<std::path::PathBuf> {
    let palette = theme::palette(ui.ctx());
    let mut reveal = None;
    Frame::none()
        .fill(palette.surface_raised)
        .stroke(Stroke::new(1.0, palette.edge_soft))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(9.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(project) = &finding.project_root {
                ui.label(
                    RichText::new(format!(
                        "PROJECT · {}",
                        tail_str(&rules::display(project), 36)
                    ))
                    .font(theme::mono(8.0))
                    .color(palette.accent),
                );
            }
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tail_str(&finding.title, 28))
                        .font(theme::display_md(10.5))
                        .color(palette.ink),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(fmt_bytes(finding.bytes))
                            .font(theme::mono(9.5))
                            .color(palette.muted),
                    );
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(finding.rebuild_cost.label())
                        .font(theme::body(8.5))
                        .color(palette.muted),
                );
                if let Some(tier) = finding.evidence.tier {
                    ui.label(
                        RichText::new(format!("· {}", developer_tier_copy(tier)))
                            .font(theme::body(8.5))
                            .color(if tier == developer::EvidenceTier::Caution {
                                palette.caution
                            } else {
                                palette.safe
                            }),
                    );
                }
                if finding.evidence.estimated {
                    ui.label(
                        RichText::new("· Upper-bound estimate")
                            .font(theme::body(8.5))
                            .color(palette.caution),
                    );
                }
                if !finding.counted {
                    ui.label(
                        RichText::new("· Not added to total")
                            .font(theme::body(8.5))
                            .color(palette.caution),
                    );
                }
            });
            egui::CollapsingHeader::new(
                RichText::new("Evidence")
                    .font(theme::mono(8.5))
                    .color(palette.accent),
            )
            .id_salt((
                "developer-evidence",
                finding.evidence.source_rec_id.as_deref(),
                &finding.evidence.measured_path,
            ))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(&finding.evidence.display_path)
                        .font(theme::mono(8.0))
                        .color(palette.faint),
                );
                ui.label(
                    RichText::new(&finding.evidence.explanation)
                        .font(theme::body(8.5))
                        .color(palette.muted),
                );
                ui.label(
                    RichText::new(format!("Recovery: {}", finding.evidence.recovery))
                        .font(theme::body(8.5))
                        .color(palette.muted),
                );
                if let Some(overlap) = &finding.evidence.overlap {
                    ui.label(
                        RichText::new(format!("Overlap: {overlap}"))
                            .font(theme::body(8.5))
                            .color(palette.caution),
                    );
                }
                if let Some(command) = finding.evidence.command {
                    ui.label(
                        RichText::new(format!("Vetted cleanup command (display only): {command}"))
                            .font(theme::mono(8.0))
                            .color(palette.faint),
                    );
                }
                if ui
                    .button(RichText::new("Reveal in Finder").font(theme::mono(8.5)))
                    .clicked()
                {
                    reveal = Some(strip_data_root(&finding.evidence.measured_path));
                }
            });
        });
    reveal
}

fn draw_docker_detail(ui: &mut egui::Ui, detail: &developer::DockerDetail) {
    let palette = theme::palette(ui.ctx());
    Frame::none()
        .fill(palette.surface_raised)
        .stroke(Stroke::new(1.0, palette.edge_soft))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(9.0, 7.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&detail.title)
                        .font(theme::display_md(10.0))
                        .color(palette.ink),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(fmt_bytes(detail.bytes))
                            .font(theme::mono(9.0))
                            .color(palette.muted),
                    );
                });
            });
            ui.label(
                RichText::new(format!(
                    "{} · Docker reports {} reclaimable",
                    detail.rebuild_cost.label(),
                    fmt_bytes(detail.reclaimable_bytes)
                ))
                .font(theme::body(8.5))
                .color(palette.muted),
            );
            ui.label(
                RichText::new(&detail.explanation)
                    .font(theme::body(8.5))
                    .color(palette.caution),
            );
        });
}

impl App {
    pub fn new() -> Self {
        let stats = disk_stats();
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let (monitor_settings, monitor_error) = match monitor::load(&monitor::settings_path(&home))
        {
            Ok(settings) => (settings, None),
            Err(error) => (MonitorSettings::default(), Some(error)),
        };
        let monitor_low = monitor::is_low(stats.free, monitor_settings.threshold_gb);
        let menu_bar_item = monitor_settings.enabled.then(|| {
            let item = MenuBarItem::new();
            item.update(stats.free, monitor_low);
            item
        });
        App {
            scan: None,
            view: None,
            crumbs: Vec::new(),
            zoom: None,
            recs: Vec::new(),
            recs_built: false,
            recs_revision: 0,
            guide_goal_bytes: 20 * GB,
            guide_custom_gb: String::new(),
            guide_goal_error: None,
            guide_acknowledged: false,
            guide_revision: None,
            guided_goal_for_review: None,
            active_guided_reclaim: None,
            guided_outcome: None,
            clean_rx: None,
            cleaning: false,
            hold: 0.0,
            ops: vec![OpsLine {
                time: now_hms(),
                text: "diskdeck v1.0 — feed online. nothing is ever removed without your explicit selection.".into(),
                kind: OpsKind::Dim,
            }],
            stats,
            stats_at: Instant::now(),
            stamp: None,
            booted: false,
            offload_rx: None,
            offloading: false,
            dialog: None,
            dialog_hold: 0.0,
            rail_view: RailView::Summary,
            activity_open: false,
            history_rx: None,
            regrowth: None,
            history_baseline: false,
            moves_rx: None,
            moved_items: Vec::new(),
            moves_error: None,
            restore_rx: None,
            restoring: false,
            restore_dialog: None,
            restore_hold: 0.0,
            reclaim_history_rx: None,
            reclaim_history: ReclaimHistory::default(),
            reclaim_history_error: None,
            trash_restore_rx: None,
            restoring_trash: false,
            trash_restore_dialog: None,
            trash_restore_hold: 0.0,
            growth_watch_rx: None,
            growth_watch: GrowthWatch::default(),
            growth_watch_error: None,
            developer_rx: None,
            developer_report: None,
            developer_error: None,
            apfs_rx: None,
            apfs: None,
            apfs_error: None,
            leftovers_rx: None,
            leftovers: Vec::new(),
            leftovers_error: None,
            monitor_settings,
            menu_bar_item,
            monitor_updated_at: Instant::now(),
            monitor_low,
            monitor_error,
            file_review_rx: None,
            file_review_cancel: None,
            file_review: None,
            file_review_error: None,
        }
    }

    fn ops(&mut self, kind: OpsKind, text: impl Into<String>) {
        self.ops.push(OpsLine {
            time: now_hms(),
            text: text.into(),
            kind,
        });
        if self.ops.len() > 300 {
            self.ops.remove(0);
        }
    }

    fn begin_scan(&mut self) {
        self.recs_revision = self.recs_revision.wrapping_add(1);
        self.guide_revision = None;
        self.guide_acknowledged = false;
        self.guided_goal_for_review = None;
        self.active_guided_reclaim = None;
        self.guided_outcome = None;
        self.scan = Some(start_scan(DATA_ROOT.into()));
        self.view = None;
        self.crumbs.clear();
        self.zoom = None;
        self.recs.clear();
        self.recs_built = false;
        self.history_rx = None;
        self.regrowth = None;
        self.history_baseline = false;
        self.developer_rx = None;
        invalidate_developer_workspace(&mut self.developer_report, &mut self.developer_error);
        self.ops(
            OpsKind::Info,
            "scan initiated — sweeping /System/Volumes/Data (read-only)",
        );
    }

    fn scanning(&self) -> bool {
        self.scan
            .as_ref()
            .map_or(false, |s| s.state() == ScanState::Running)
    }

    fn scan_done(&self) -> bool {
        self.scan.as_ref().map_or(false, |s| {
            matches!(s.state(), ScanState::Done | ScanState::Aborted)
        })
    }

    fn on_scan_finished(&mut self) {
        let Some(scan) = &self.scan else { return };
        let state = scan.state();
        let (files, bytes, denied, ms, done) = (
            scan.root.files(),
            scan.root.bytes(),
            scan.denied.load(Relaxed),
            scan.duration_ms.load(Relaxed),
            state == ScanState::Done,
        );
        let root = scan.root.clone();
        self.ops(
            if done { OpsKind::Ok } else { OpsKind::Amber },
            format!(
                "scan {} — {} items, {} mapped in {}",
                if done { "complete" } else { "aborted" },
                fmt_count(files),
                fmt_bytes(bytes),
                fmt_elapsed(Duration::from_millis(ms.max(0) as u64))
            ),
        );
        if denied > 0 {
            self.ops(OpsKind::Amber, format!(
                "{denied} locations were off-limits — root-only system dirs are normal; grant Full Disk Access for your gated folders"
            ));
        }
        let recs = rules::build_recommendations(&root);
        let total: i64 = recs.iter().map(|r| r.bytes).sum();
        if !recs.is_empty() {
            self.ops(
                OpsKind::Info,
                format!(
                    "{} reclaim targets identified totalling {}",
                    recs.len(),
                    fmt_bytes(total)
                ),
            );
        }
        self.recs = recs
            .into_iter()
            .map(|rec| RecRow {
                checked: rec.tier == Tier::Safe,
                action: rec.action,
                expanded: false,
                status: RecStatus::Idle,
                rec,
            })
            .collect();
        self.recs_built = true;
        self.guide_goal_bytes = if self.stats.used > 0 {
            (20 * GB).min(self.stats.used)
        } else {
            20 * GB
        };
        self.guide_revision = Some(self.recs_revision);
        self.guide_acknowledged = false;
        self.guide_goal_error = None;
        self.begin_leftovers(root.clone());
        if should_record_history(state) {
            if let Some(dir) = default_history_dir() {
                let (tx, rx) = std::sync::mpsc::channel();
                match record_scan(root, dir, tx) {
                    Ok(()) => self.history_rx = Some(rx),
                    Err(error) => self.ops(
                        OpsKind::Amber,
                        format!("scan history unavailable — {error}"),
                    ),
                }
            }
        }
    }

    fn poll_history(&mut self) {
        let event = match self.history_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(event)) => Some(event),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => Some(HistoryEvent::Failed(
                "history worker stopped before reporting".into(),
            )),
        };
        self.history_rx = None;
        match event.unwrap() {
            HistoryEvent::BaselineSaved => {
                self.history_baseline = true;
                self.regrowth = None;
                self.ops(
                    OpsKind::Dim,
                    "scan baseline saved — future scans will show what grew",
                );
                self.begin_growth_refresh();
            }
            HistoryEvent::Compared(summary) => {
                self.history_baseline = false;
                let largest = summary.growers.first().map(|growth| {
                    format!(
                        "; largest growth: {} {}",
                        growth.path.display(),
                        fmt_delta(growth.bytes_delta)
                    )
                });
                self.ops(
                    OpsKind::Info,
                    format!(
                        "since last scan: {}{}",
                        fmt_delta(summary.total_delta),
                        largest.unwrap_or_default()
                    ),
                );
                self.regrowth = Some(summary);
                self.begin_growth_refresh();
            }
            HistoryEvent::Failed(error) => {
                self.history_baseline = false;
                self.regrowth = None;
                self.ops(
                    OpsKind::Amber,
                    format!("scan history unavailable — {error}"),
                );
            }
        }
    }

    fn begin_growth_refresh(&mut self) {
        if self.growth_watch_rx.is_some() {
            return;
        }
        let Some(dir) = default_history_dir() else {
            self.growth_watch_error = Some("Home folder is unavailable".into());
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("growth-watch".into())
            .spawn(move || {
                let result = load_growth_watch(&dir, Path::new(DATA_ROOT));
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                self.growth_watch_rx = Some(rx);
                self.growth_watch_error = None;
            }
            Err(error) => {
                self.growth_watch_error = Some(format!("start Growth Watch: {error}"));
            }
        }
    }

    fn set_growth_folder(&mut self, folder: std::path::PathBuf, watched: bool) {
        if self.growth_watch_rx.is_some() {
            return;
        }
        let Some(dir) = default_history_dir() else {
            self.growth_watch_error = Some("Home folder is unavailable".into());
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("growth-watch-update".into())
            .spawn(move || {
                let result = set_folder_watched(&dir, &folder, watched)
                    .and_then(|_| load_growth_watch(&dir, Path::new(DATA_ROOT)));
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                self.growth_watch_rx = Some(rx);
                self.growth_watch_error = None;
            }
            Err(error) => {
                self.growth_watch_error = Some(format!("update Growth Watch: {error}"));
            }
        }
    }

    fn poll_growth_watch(&mut self) {
        let result = match self.growth_watch_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Some(Err("Growth Watch worker stopped before reporting".into()))
            }
        };
        self.growth_watch_rx = None;
        match result.unwrap() {
            Ok(watch) => {
                self.growth_watch = watch;
                self.growth_watch_error = None;
            }
            Err(error) => {
                self.growth_watch_error = Some(error.clone());
                self.ops(
                    OpsKind::Amber,
                    format!("Growth Watch unavailable — {error}"),
                );
            }
        }
    }

    fn developer_scan_ready(&self) -> bool {
        self.recs_built
            && self
                .scan
                .as_ref()
                .is_some_and(|scan| scan.state() == ScanState::Done)
    }

    fn begin_developer_refresh(&mut self, force: bool) {
        if !should_start_developer_worker(
            force || self.rail_view == RailView::Developer,
            self.developer_scan_ready(),
            self.developer_rx.is_some(),
            self.developer_report.is_some() && !force,
        ) {
            return;
        }
        let Some(root) = self.scan.as_ref().map(|scan| scan.root.clone()) else {
            self.developer_error = Some("Completed scan evidence is unavailable".into());
            return;
        };
        let Some(home) = std::env::var_os("HOME") else {
            self.developer_error = Some("Home folder is unavailable".into());
            return;
        };
        let home_data = std::path::PathBuf::from(format!(
            "{DATA_ROOT}{}",
            std::path::PathBuf::from(home).to_string_lossy()
        ));
        let recs: Vec<Rec> = self.recs.iter().map(|row| row.rec.clone()).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("developer-deep-dive".into())
            .spawn(move || {
                let docker = developer::load_docker_breakdown();
                let report = developer::build_report_with_inventory(
                    &recs,
                    docker,
                    &root,
                    &home_data,
                    |project, marker| strip_data_root(project).join(marker).is_file(),
                );
                let _ = tx.send(Ok(report));
            }) {
            Ok(_) => {
                self.developer_rx = Some(rx);
                self.developer_error = None;
                if force {
                    self.developer_report = None;
                }
            }
            Err(error) => {
                self.developer_error = Some(format!("start Developer Deep Dive: {error}"));
            }
        }
    }

    fn poll_developer(&mut self) {
        let result = match self.developer_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => Some(Err(
                "Developer Deep Dive worker stopped before reporting".into(),
            )),
        };
        self.developer_rx = None;
        match result.unwrap() {
            Ok(report) => {
                self.developer_report = Some(report);
                self.developer_error = None;
            }
            Err(error) => {
                self.developer_report = None;
                self.developer_error = Some(error.clone());
                self.ops(
                    OpsKind::Amber,
                    format!("Developer Deep Dive unavailable — {error}"),
                );
            }
        }
    }

    fn begin_apfs_refresh(&mut self) {
        if self.apfs_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("apfs-accounting".into())
            .spawn(move || {
                let _ = tx.send(apfs::load());
            }) {
            Ok(_) => {
                self.apfs_rx = Some(rx);
                self.apfs_error = None;
            }
            Err(error) => self.apfs_error = Some(format!("start APFS accounting: {error}")),
        }
    }

    fn poll_apfs(&mut self) {
        let result = match self.apfs_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Some(Err("APFS worker stopped before reporting".into()))
            }
        };
        self.apfs_rx = None;
        match result.unwrap() {
            Ok(accounting) => {
                self.apfs = Some(accounting);
                self.apfs_error = None;
            }
            Err(error) => {
                self.apfs_error = Some(error.clone());
                self.ops(
                    OpsKind::Amber,
                    format!("APFS accounting unavailable — {error}"),
                );
            }
        }
    }

    fn begin_leftovers(&mut self, root: Arc<Node>) {
        if self.leftovers_rx.is_some() {
            return;
        }
        let home = Self::home_dir();
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("app-leftovers".into())
            .spawn(move || {
                let _ = tx.send(leftovers::detect(&root, &home));
            }) {
            Ok(_) => {
                self.leftovers_rx = Some(rx);
                self.leftovers_error = None;
            }
            Err(error) => self.leftovers_error = Some(format!("start app leftovers: {error}")),
        }
    }

    fn poll_leftovers(&mut self) {
        let result = match self.leftovers_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Some(Err("app-leftovers worker stopped before reporting".into()))
            }
        };
        self.leftovers_rx = None;
        match result.unwrap() {
            Ok(findings) => {
                self.leftovers = findings;
                self.leftovers_error = None;
            }
            Err(error) => {
                self.leftovers_error = Some(error.clone());
                self.ops(
                    OpsKind::Amber,
                    format!("app leftovers unavailable — {error}"),
                );
            }
        }
    }

    fn apply_monitor_settings(&mut self, next: MonitorSettings) {
        let home = Self::home_dir();
        let launch_changed = next.launch_at_login != self.monitor_settings.launch_at_login;
        if launch_changed {
            if let Err(error) = monitor::set_launch_at_login(&home, next.launch_at_login) {
                self.monitor_error = Some(error.clone());
                self.ops(
                    OpsKind::Err,
                    format!("menu monitor settings failed — {error}"),
                );
                return;
            }
        }
        if let Err(error) = monitor::save(&monitor::settings_path(&home), next) {
            if launch_changed {
                let _ = monitor::set_launch_at_login(&home, self.monitor_settings.launch_at_login);
            }
            self.monitor_error = Some(error.clone());
            self.ops(
                OpsKind::Err,
                format!("menu monitor settings failed — {error}"),
            );
            return;
        }
        self.monitor_settings = next;
        self.monitor_low = monitor::is_low(self.stats.free, next.threshold_gb);
        if next.enabled {
            if self.menu_bar_item.is_none() {
                self.menu_bar_item = Some(MenuBarItem::new());
            }
            if let Some(item) = &self.menu_bar_item {
                item.update(self.stats.free, self.monitor_low);
            }
        } else {
            self.menu_bar_item = None;
        }
        self.monitor_updated_at = Instant::now();
        self.monitor_error = None;
        self.ops(
            OpsKind::Info,
            format!(
                "menu-bar monitor {}{}",
                if next.enabled { "enabled" } else { "disabled" },
                if next.launch_at_login {
                    " · launch at login enabled"
                } else {
                    ""
                }
            ),
        );
    }

    fn update_menu_monitor(&mut self) {
        if !self.monitor_settings.enabled
            || self.monitor_updated_at.elapsed() < Duration::from_secs(300)
        {
            return;
        }
        let low = monitor::is_low(self.stats.free, self.monitor_settings.threshold_gb);
        if let Some(item) = &self.menu_bar_item {
            item.update(self.stats.free, low);
        }
        if low && !self.monitor_low {
            self.ops(
                OpsKind::Amber,
                format!(
                    "low space — {} free, below the {} GB local warning threshold",
                    fmt_bytes(self.stats.free),
                    self.monitor_settings.threshold_gb
                ),
            );
        }
        self.monitor_low = low;
        self.monitor_updated_at = Instant::now();
    }

    fn begin_file_review(&mut self) {
        if self.file_review_rx.is_some() {
            return;
        }
        let roots = file_review::standard_roots(&Self::home_dir());
        if roots.is_empty() {
            self.file_review_error =
                Some("No standard user folders are available to review".into());
            return;
        }
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        match file_review::run(roots, cancel.clone(), tx) {
            Ok(()) => {
                self.file_review_rx = Some(rx);
                self.file_review_cancel = Some(cancel);
                self.file_review = None;
                self.file_review_error = None;
                self.ops(OpsKind::Info, "opt-in file review started — read-only");
            }
            Err(error) => self.file_review_error = Some(error),
        }
    }

    fn poll_file_review(&mut self) {
        let result = match self.file_review_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Some(Err("file-review worker stopped before reporting".into()))
            }
        };
        self.file_review_rx = None;
        self.file_review_cancel = None;
        match result.unwrap() {
            Ok(result) => {
                self.ops(
                    OpsKind::Info,
                    format!(
                        "file review complete — {} files, {} duplicate groups, {} large-old files",
                        fmt_count(result.files_visited as i64),
                        result.duplicate_groups.len(),
                        result.large_old.len()
                    ),
                );
                self.file_review = Some(result);
                self.file_review_error = None;
            }
            Err(error) if error == "file review cancelled" => {
                self.file_review_error = None;
                self.ops(OpsKind::Dim, "file review cancelled");
            }
            Err(error) => {
                self.file_review_error = Some(error.clone());
                self.ops(OpsKind::Amber, format!("file review unavailable — {error}"));
            }
        }
    }

    fn poll_clean_events(&mut self) {
        let Some(rx) = &self.clean_rx else { return };
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        for ev in events {
            match ev {
                CleanEvent::Started { id, .. } => {
                    if let Some(row) = self.recs.iter_mut().find(|r| r.rec.id == id) {
                        row.status = RecStatus::Running;
                    }
                }
                CleanEvent::Result {
                    id,
                    title,
                    ok,
                    freed,
                    pending,
                    message,
                    history_warning,
                } => {
                    if let Some(tracker) = &mut self.active_guided_reclaim {
                        tracker.record_result(&id, ok);
                    }
                    if let Some(row) = self.recs.iter_mut().find(|r| r.rec.id == id) {
                        row.checked = false;
                        row.status = if !ok {
                            RecStatus::Failed(message.clone())
                        } else if pending > 0 {
                            RecStatus::InTrash(pending)
                        } else {
                            RecStatus::Cleared(freed)
                        };
                    }
                    if ok {
                        if pending > 0 {
                            self.ops(
                                OpsKind::Ok,
                                format!(
                                    "✓ {title} — {} moved to Trash (empty it to free)",
                                    fmt_bytes(pending)
                                ),
                            );
                        } else {
                            self.ops(
                                OpsKind::Ok,
                                format!("✓ {title} — freed {}", fmt_bytes(freed)),
                            );
                        }
                        if !message.is_empty() {
                            self.ops(OpsKind::Dim, format!("  {message}"));
                        }
                        if let Some(warning) = history_warning {
                            self.ops(OpsKind::Amber, warning);
                        }
                    } else {
                        self.ops(OpsKind::Err, format!("✗ {title} — {message}"));
                    }
                }
                CleanEvent::Done { freed, pending } => {
                    self.cleaning = false;
                    self.clean_rx = None;
                    if let Some(tracker) = self.active_guided_reclaim.take() {
                        self.guided_outcome = Some(tracker.finish(freed, pending));
                        self.rail_view = RailView::GuidedReclaim;
                    }
                    self.recs_revision = self.recs_revision.wrapping_add(1);
                    self.guide_revision = None;
                    self.guide_acknowledged = false;
                    self.stats = disk_stats();
                    self.stats_at = Instant::now();
                    if freed > 0 {
                        self.stamp =
                            Some((format!("+{} RECLAIMED", fmt_bytes(freed)), Instant::now()));
                    }
                    self.ops(
                        OpsKind::Ok,
                        format!(
                            "reclaim complete — {} freed{}",
                            fmt_bytes(freed),
                            if pending > 0 {
                                format!(", {} waiting in Trash", fmt_bytes(pending))
                            } else {
                                String::new()
                            }
                        ),
                    );
                    if pending > 0 {
                        self.ops(OpsKind::Amber,
                            "tip: select the Trash target and reclaim again (or empty Trash in Finder) to finish the job");
                    }
                    self.ops(OpsKind::Dim, "rescan to refresh the terrain map");
                    self.force_reclaim_history_refresh();
                    return;
                }
            }
        }
    }

    fn poll_offload(&mut self) {
        let Some(rx) = &self.offload_rx else { return };
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        for ev in events {
            match ev {
                OffloadEvent::Started { name, total } => {
                    self.ops(
                        OpsKind::Info,
                        format!("offloading {name} — {}", fmt_bytes(total)),
                    );
                }
                OffloadEvent::Done {
                    reclaimed,
                    dest,
                    symlinked,
                    registry_warning,
                } => {
                    self.offloading = false;
                    self.offload_rx = None;
                    self.stats = disk_stats();
                    self.stats_at = Instant::now();
                    self.stamp = Some((
                        format!("+{} OFFLOADED", fmt_bytes(reclaimed)),
                        Instant::now(),
                    ));
                    let tail = if symlinked {
                        " (symlink left at the old path)"
                    } else {
                        ""
                    };
                    self.ops(OpsKind::Ok, format!("✓ moved to {}{tail}", dest.display()));
                    if let Some(error) = registry_warning {
                        self.ops(
                            OpsKind::Amber,
                            format!(
                                "move completed, but Restore Center could not record it — {error}"
                            ),
                        );
                    }
                    self.ops(OpsKind::Dim, "rescan to refresh the terrain map");
                    self.begin_move_refresh();
                }
                OffloadEvent::Failed { error } => {
                    self.offloading = false;
                    self.offload_rx = None;
                    self.ops(OpsKind::Err, format!("✗ offload failed — {error}"));
                }
            }
        }
    }

    fn home_dir() -> std::path::PathBuf {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
    }

    fn mutation_busy(&self) -> bool {
        self.cleaning || self.offloading || self.restoring || self.restoring_trash
    }

    fn begin_reclaim_history_refresh(&mut self) {
        if self.reclaim_history_rx.is_some() {
            return;
        }
        let home = Self::home_dir();
        let history = history_path_for_home(&home);
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("reclaim-history-refresh".into())
            .spawn(move || {
                let _ = tx.send(refresh_history(&history, &home));
            }) {
            Ok(_) => {
                self.reclaim_history_rx = Some(rx);
                self.reclaim_history_error = None;
            }
            Err(error) => {
                self.reclaim_history_error =
                    Some(format!("start reclaim-history refresh: {error}"));
            }
        }
    }

    fn force_reclaim_history_refresh(&mut self) {
        self.reclaim_history_rx = None;
        self.begin_reclaim_history_refresh();
    }

    fn poll_reclaim_history(&mut self) {
        let result = match self.reclaim_history_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Some(Err("reclaim-history worker stopped before reporting".into()))
            }
        };
        self.reclaim_history_rx = None;
        match result.unwrap() {
            Ok(history) => {
                self.reclaim_history = history;
                self.reclaim_history_error = None;
            }
            Err(error) => {
                self.reclaim_history_error = Some(error.clone());
                self.ops(
                    OpsKind::Amber,
                    format!("reclaim history unavailable — {error}"),
                );
            }
        }
    }

    fn invalidate_scan_after_trash_restore(&mut self) {
        self.scan = None;
        self.view = None;
        self.crumbs.clear();
        self.zoom = None;
        self.recs.clear();
        self.recs_built = false;
        self.recs_revision = self.recs_revision.wrapping_add(1);
        self.guide_revision = None;
        self.guide_acknowledged = false;
        self.guided_goal_for_review = None;
    }

    fn poll_trash_restore(&mut self) {
        let Some(rx) = &self.trash_restore_rx else {
            return;
        };
        let mut events = Vec::new();
        let disconnected = loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        let terminal = events.iter().any(|event| {
            matches!(
                event,
                TrashRestoreEvent::Done { .. } | TrashRestoreEvent::Failed { .. }
            )
        });
        if disconnected && !terminal {
            events.push(TrashRestoreEvent::Failed {
                error: "Trash restore worker stopped before reporting".into(),
            });
        }
        for event in events {
            match event {
                TrashRestoreEvent::Started { title, bytes } => self.ops(
                    OpsKind::Info,
                    format!("restoring {title} from Trash — {}", fmt_bytes(bytes)),
                ),
                TrashRestoreEvent::Done {
                    bytes,
                    origin,
                    warning,
                } => {
                    self.restoring_trash = false;
                    self.trash_restore_rx = None;
                    self.stats = disk_stats();
                    self.stats_at = Instant::now();
                    self.invalidate_scan_after_trash_restore();
                    self.ops(
                        OpsKind::Ok,
                        format!(
                            "✓ restored {} from Trash to {}",
                            fmt_bytes(bytes),
                            origin.display()
                        ),
                    );
                    if let Some(warning) = warning {
                        self.ops(OpsKind::Amber, warning);
                    }
                    self.ops(OpsKind::Dim, "scan again to refresh the terrain map");
                    self.force_reclaim_history_refresh();
                }
                TrashRestoreEvent::Failed { error } => {
                    self.restoring_trash = false;
                    self.trash_restore_rx = None;
                    self.ops(OpsKind::Err, format!("✗ Trash restore failed — {error}"));
                    self.force_reclaim_history_refresh();
                }
            }
        }
    }

    fn begin_move_refresh(&mut self) {
        if self.moves_rx.is_some() {
            return;
        }
        let home = Self::home_dir();
        let registry = registry_path_for_home(&home);
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("move-refresh".into())
            .spawn(move || {
                let result = refresh_records(&registry, &home, Path::new("/Volumes"));
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                self.moves_rx = Some(rx);
                self.moves_error = None;
            }
            Err(error) => {
                self.moves_error = Some(format!("start moved-items refresh: {error}"));
            }
        }
    }

    fn poll_moves(&mut self) {
        let result = match self.moves_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Some(Err("moved-items worker stopped before reporting".into()))
            }
        };
        self.moves_rx = None;
        match result.unwrap() {
            Ok(items) => {
                self.moved_items = items;
                self.moves_error = None;
            }
            Err(error) => {
                self.moves_error = Some(error.clone());
                self.ops(OpsKind::Amber, format!("moved items unavailable — {error}"));
            }
        }
    }

    fn poll_restore(&mut self) {
        let Some(rx) = &self.restore_rx else { return };
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                RestoreEvent::Started { name, total } => self.ops(
                    OpsKind::Info,
                    format!("restoring {name} — {}", fmt_bytes(total)),
                ),
                RestoreEvent::Done {
                    restored,
                    origin,
                    warning,
                } => {
                    self.restoring = false;
                    self.restore_rx = None;
                    self.stats = disk_stats();
                    self.stats_at = Instant::now();
                    self.ops(
                        OpsKind::Ok,
                        format!("✓ restored {} to {}", fmt_bytes(restored), origin.display()),
                    );
                    if let Some(warning) = warning {
                        self.ops(OpsKind::Amber, warning);
                    }
                    self.begin_move_refresh();
                }
                RestoreEvent::Failed { error } => {
                    self.restoring = false;
                    self.restore_rx = None;
                    self.ops(OpsKind::Err, format!("✗ restore failed — {error}"));
                    self.begin_move_refresh();
                }
            }
        }
    }

    /// Prepare and show the offload confirm dialog for a real (stripped) path.
    fn open_offload_dialog(&mut self, src: std::path::PathBuf, size: i64) {
        if self.mutation_busy() {
            self.ops(
                OpsKind::Amber,
                "another storage operation is already active",
            );
            return;
        }
        let vols = external_volumes();
        if vols.is_empty() {
            self.ops(OpsKind::Amber, "attach an external drive to offload to");
            return;
        }
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let reason = check_movable(&src, &home)
            .err()
            .map(|block| block.message().to_owned());
        self.dialog = Some(OffloadDialog {
            src,
            size,
            vols,
            vol_idx: 0,
            leave_symlink: true,
            acknowledged: false,
            show_info: false,
            reason,
        });
        self.dialog_hold = 0.0;
    }

    fn fire_reclaim(&mut self) {
        if self.mutation_busy() {
            self.ops(
                OpsKind::Amber,
                "another storage operation is already active",
            );
            return;
        }
        let jobs: Vec<CleanJob> = self
            .recs
            .iter()
            .filter(|r| r.checked)
            .map(|r| CleanJob {
                rec: r.rec.clone(),
                action: r.action,
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        if let Some(goal_bytes) = self.guided_goal_for_review.take() {
            let selected: Vec<&RecRow> = self.recs.iter().filter(|row| row.checked).collect();
            self.active_guided_reclaim = Some(OutcomeTracker::new(
                goal_bytes,
                selected.iter().map(|row| row.rec.bytes).sum(),
                selected
                    .iter()
                    .filter(|row| row.rec.estimate)
                    .map(|row| row.rec.bytes)
                    .sum(),
                selected.iter().map(|row| row.rec.id.clone()),
            ));
            self.guided_outcome = None;
        }
        self.ops(
            OpsKind::Amber,
            format!("reclaim engaged — {} target(s)", jobs.len()),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        self.clean_rx = Some(rx);
        self.cleaning = true;
        run_clean(jobs, history_path_for_home(&Self::home_dir()), tx);
    }

    fn accept_guided_plan(&mut self, plan: &ReclaimPlan) {
        if !can_apply_guided_plan(
            self.guide_acknowledged,
            self.guide_revision,
            self.recs_revision,
            self.scanning(),
            plan,
        ) {
            return;
        }
        apply_guided_plan(&mut self.recs, plan);
        self.guided_goal_for_review = Some(plan.goal_bytes);
        self.guide_acknowledged = false;
        self.rail_view = RailView::Reclaim;
    }

    fn view_node(&self) -> Option<Arc<Node>> {
        self.view
            .clone()
            .or_else(|| self.scan.as_ref().map(|s| s.root.clone()))
    }
}

struct WorkspaceLayout {
    overview: Rect,
    map: Rect,
    rail: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GuidedLayout {
    nav: Rect,
    body: Rect,
    action: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ReviewRowColumns {
    text_width: f32,
    gutter: f32,
    utility_width: f32,
}

fn review_row_columns(available_width: f32) -> ReviewRowColumns {
    let available_width = available_width.max(0.0);
    let utility_width = available_width.min(72.0);
    let gutter = (available_width - utility_width).clamp(0.0, 8.0);
    let text_width = (available_width - utility_width - gutter).max(0.0);
    ReviewRowColumns {
        text_width,
        gutter,
        utility_width,
    }
}

fn reclaim_history_row_columns(available_width: f32) -> ReviewRowColumns {
    let available_width = available_width.max(0.0);
    let utility_width = available_width.min(104.0);
    let gutter = (available_width - utility_width).clamp(0.0, 8.0);
    let text_width = (available_width - utility_width - gutter).max(0.0);
    ReviewRowColumns {
        text_width,
        gutter,
        utility_width,
    }
}

fn receipt_state_copy(state: &ReceiptState) -> &'static str {
    match state {
        ReceiptState::Ready => "Ready to restore",
        ReceiptState::Missing => "Trash item missing",
        ReceiptState::OriginOccupied => "Original path occupied",
        ReceiptState::Changed => "Changed in Trash",
        ReceiptState::ManualOnly => "Open Trash to restore manually",
        ReceiptState::UnsafeOrigin => "Restore unavailable",
        ReceiptState::SymlinkAncestor => "Original path cannot be verified",
        ReceiptState::CrossDevice => "Original volume changed",
        ReceiptState::Unavailable => "Restore unavailable",
        ReceiptState::Restored => "Restored",
        ReceiptState::Permanent => "Permanent — cannot restore",
    }
}

fn receipt_action_copy(action: ReceiptAction) -> &'static str {
    match action {
        ReceiptAction::Trash => "Trash",
        ReceiptAction::Delete => "Permanent erase",
        ReceiptAction::Empty => "Emptied contents",
        ReceiptAction::Command => "Vetted cleanup command",
    }
}

fn receipt_age_copy(completed_at_ms: i64) -> String {
    let age = now_ms().saturating_sub(completed_at_ms).max(0) / 1000;
    if age < 60 {
        "just now".into()
    } else if age < 3600 {
        format!("{} min ago", age / 60)
    } else if age < 86_400 {
        format!("{} h ago", age / 3600)
    } else {
        format!("{} d ago", age / 86_400)
    }
}

fn can_start_trash_restore(acknowledged: bool, state: &ReceiptState, mutation_busy: bool) -> bool {
    receipt_can_confirm_restore(acknowledged, state) && !mutation_busy
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MapItemActions {
    open: bool,
    reveal: bool,
    move_to_ssd: bool,
}

fn map_item_actions(
    is_dir: bool,
    synthetic: bool,
    denied: bool,
    has_node: bool,
    offload_allowed: bool,
) -> MapItemActions {
    let real = has_node && !synthetic;
    MapItemActions {
        open: real && is_dir && !denied,
        reveal: real,
        move_to_ssd: real && offload_allowed,
    }
}

fn map_item_hint(is_dir: bool, synthetic: bool, denied: bool) -> &'static str {
    if synthetic {
        "Combined smaller items"
    } else if denied {
        "Access unavailable · Grant Full Disk Access to inspect"
    } else if is_dir {
        "Click to open · Right-click for actions"
    } else {
        "Right-click for actions"
    }
}

fn back_target(depth: usize) -> Option<usize> {
    depth.checked_sub(1)
}

fn should_open_from_primary(
    actions: MapItemActions,
    primary_clicked: bool,
    control_down: bool,
) -> bool {
    actions.open && primary_clicked && !control_down
}

fn should_navigate_back_on_escape(
    escape_pressed: bool,
    menu_was_open: bool,
    menu_is_open: bool,
) -> bool {
    escape_pressed && !menu_was_open && !menu_is_open
}

fn can_apply_guided_plan(
    acknowledged: bool,
    draft_revision: Option<u64>,
    recs_revision: u64,
    scanning: bool,
    plan: &crate::reclaim_plan::ReclaimPlan,
) -> bool {
    acknowledged && draft_revision == Some(recs_revision) && !scanning && !plan.items.is_empty()
}

fn apply_guided_plan(rows: &mut [RecRow], plan: &crate::reclaim_plan::ReclaimPlan) {
    let ids: std::collections::BTreeSet<&str> =
        plan.items.iter().map(|item| item.id.as_str()).collect();
    for row in rows {
        row.checked = ids.contains(row.rec.id.as_str()) && row.rec.tier == Tier::Safe;
    }
}

fn goal_error_copy(error: GoalError) -> &'static str {
    match error {
        GoalError::Empty => "Enter a goal in whole gigabytes.",
        GoalError::NotWholeGigabytes => "Use a whole number such as 25.",
        GoalError::Zero => "Choose at least 1 GB.",
        GoalError::ExceedsUsedSpace => "The goal cannot exceed currently used space.",
    }
}

fn plan_status_copy(plan: &ReclaimPlan) -> String {
    if plan.items.is_empty() {
        "No automatically safe targets are available in this scan.".into()
    } else if plan.shortfall_bytes > 0 {
        format!(
            "Safe targets provide about {}. Your goal is short by {}.",
            fmt_bytes(plan.selected_bytes),
            fmt_bytes(plan.shortfall_bytes)
        )
    } else {
        format!(
            "This Safe plan reaches the goal with about {} selected.",
            fmt_bytes(plan.selected_bytes)
        )
    }
}

fn confidence_copy(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Early => "Early estimate",
        Confidence::Developing => "Developing estimate",
        Confidence::Reliable => "Reliable estimate",
    }
}

fn forecast_headline(forecast: &ForecastState) -> String {
    match forecast {
        ForecastState::NeedHistory {
            observations,
            span_days,
        } => format!(
            "Not enough history yet — {observations} compatible scan(s) across {span_days} day(s)."
        ),
        ForecastState::AlreadyLow { .. } => {
            "Storage is already below your low-space threshold.".into()
        }
        ForecastState::Flat { .. } => "Storage use is roughly steady.".into(),
        ForecastState::Improving { .. } => "Free space is improving across recent scans.".into(),
        ForecastState::Volatile { .. } => {
            "Recent storage changes are too volatile for an honest forecast.".into()
        }
        ForecastState::Estimate(estimate) => {
            let low_weeks = (estimate.days_low + 6) / 7;
            let high_weeks = (estimate.days_high + 6) / 7;
            format!(
                "At the recent rate, storage may become low in about {low_weeks}–{high_weeks} weeks."
            )
        }
    }
}

fn guided_layout(content: Rect) -> GuidedLayout {
    let inset = content.shrink2(vec2(12.0, 8.0));
    let nav = Rect::from_min_size(inset.min, vec2(inset.width(), 30.0));
    let action = Rect::from_min_max(pos2(inset.min.x, inset.max.y - 82.0), inset.max);
    let body = Rect::from_min_max(
        pos2(inset.min.x, nav.max.y + 6.0),
        pos2(inset.max.x, action.min.y - 8.0),
    );
    GuidedLayout { nav, body, action }
}

enum MapActionRequest {
    Open {
        node: Arc<Node>,
        source: Rect,
    },
    Reveal(std::path::PathBuf),
    MoveToSsd {
        path: std::path::PathBuf,
        bytes: i64,
    },
}

impl WorkspaceLayout {
    fn from_rect(full: Rect) -> Self {
        let overview = Rect::from_min_size(full.min, vec2(full.width(), 128.0));
        let content_top = overview.max.y + 12.0;
        let rail_w = (full.width() * 0.28).clamp(320.0, 344.0);
        let rail = Rect::from_min_max(pos2(full.max.x - rail_w, content_top), full.max);
        let map = Rect::from_min_max(
            pos2(full.min.x, content_top),
            pos2(rail.min.x - 12.0, full.max.y),
        );
        Self {
            overview,
            map,
            rail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_rec_row(id: &str, tier: Tier, checked: bool) -> RecRow {
        RecRow {
            rec: Rec {
                id: id.into(),
                title: id.into(),
                path: std::path::PathBuf::from(format!("/fixture/{id}")),
                display: format!("/fixture/{id}"),
                bytes: 10,
                tier,
                desc: "fixture",
                restore: "fixture",
                action: Action::Trash,
                command: None,
                allow_trash: true,
                allow_delete: true,
                note: String::new(),
                estimate: false,
            },
            checked,
            action: Action::Trash,
            expanded: false,
            status: RecStatus::Idle,
        }
    }

    fn fixture_plan() -> crate::reclaim_plan::ReclaimPlan {
        crate::reclaim_plan::ReclaimPlan {
            goal_bytes: 10,
            items: vec![crate::reclaim_plan::PlanItem {
                id: "safe-b".into(),
                bytes: 10,
                estimate: false,
            }],
            selected_bytes: 10,
            measured_bytes: 10,
            estimated_bytes: 0,
            shortfall_bytes: 0,
            caution_bytes: 20,
        }
    }

    #[test]
    fn guided_plan_requires_acknowledgement_current_revision_and_items() {
        let plan = fixture_plan();
        assert!(can_apply_guided_plan(true, Some(4), 4, false, &plan));
        assert!(!can_apply_guided_plan(false, Some(4), 4, false, &plan));
        assert!(!can_apply_guided_plan(true, Some(3), 4, false, &plan));
        assert!(!can_apply_guided_plan(true, Some(4), 4, true, &plan));

        let mut empty = plan.clone();
        empty.items.clear();
        assert!(!can_apply_guided_plan(true, Some(4), 4, false, &empty));
    }

    #[test]
    fn guided_plan_checks_only_named_safe_rows() {
        let mut rows = vec![
            fixture_rec_row("safe-a", Tier::Safe, true),
            fixture_rec_row("safe-b", Tier::Safe, true),
            fixture_rec_row("caution", Tier::Caution, true),
        ];
        apply_guided_plan(&mut rows, &fixture_plan());
        assert!(!rows[0].checked);
        assert!(rows[1].checked);
        assert!(!rows[2].checked);
    }

    #[test]
    fn guided_goal_errors_use_plain_actionable_copy() {
        assert_eq!(
            goal_error_copy(GoalError::Empty),
            "Enter a goal in whole gigabytes."
        );
        assert_eq!(
            goal_error_copy(GoalError::NotWholeGigabytes),
            "Use a whole number such as 25."
        );
        assert_eq!(goal_error_copy(GoalError::Zero), "Choose at least 1 GB.");
        assert_eq!(
            goal_error_copy(GoalError::ExceedsUsedSpace),
            "The goal cannot exceed currently used space."
        );
    }

    #[test]
    fn guided_plan_status_distinguishes_empty_shortfall_and_reached() {
        let mut plan = fixture_plan();
        plan.items.clear();
        assert_eq!(
            plan_status_copy(&plan),
            "No automatically safe targets are available in this scan."
        );

        plan.items.push(crate::reclaim_plan::PlanItem {
            id: "safe".into(),
            bytes: 6 * GB,
            estimate: false,
        });
        plan.selected_bytes = 6 * GB;
        plan.shortfall_bytes = 4 * GB;
        assert_eq!(
            plan_status_copy(&plan),
            "Safe targets provide about 6.0 GB. Your goal is short by 4.0 GB."
        );

        plan.selected_bytes = 11 * GB;
        plan.shortfall_bytes = 0;
        assert_eq!(
            plan_status_copy(&plan),
            "This Safe plan reaches the goal with about 11.0 GB selected."
        );
    }

    #[test]
    fn developer_workspace_states_cover_waiting_loading_empty_partial_and_error() {
        assert_eq!(
            developer_workspace_state(None, false, None, false),
            DeveloperWorkspaceState::WaitingForScan
        );
        assert_eq!(
            developer_workspace_state(None, true, None, true),
            DeveloperWorkspaceState::Loading
        );
        assert_eq!(
            developer_workspace_state(None, false, Some("worker stopped"), true),
            DeveloperWorkspaceState::Failed
        );

        let empty = developer::DeveloperReport::default();
        assert_eq!(
            developer_workspace_state(Some(&empty), false, None, true),
            DeveloperWorkspaceState::Empty
        );

        let mut partial = developer::DeveloperReport::default();
        partial.sections.push(developer::DeveloperSectionReport {
            section: developer::DeveloperSection::Projects,
            measured_bytes: 10,
            findings: Vec::new(),
        });
        assert_eq!(
            developer_workspace_state(Some(&partial), false, None, true),
            DeveloperWorkspaceState::Partial
        );
        partial.docker.unavailable = Some("engine unavailable".into());
        assert_eq!(
            developer_workspace_state(Some(&partial), false, None, true),
            DeveloperWorkspaceState::DockerUnavailable
        );

        let mut populated = developer::DeveloperReport::default();
        for section in [
            developer::DeveloperSection::Docker,
            developer::DeveloperSection::Xcode,
            developer::DeveloperSection::Projects,
        ] {
            populated.sections.push(developer::DeveloperSectionReport {
                section,
                measured_bytes: 10,
                findings: Vec::new(),
            });
        }
        assert_eq!(
            developer_workspace_state(Some(&populated), false, None, true),
            DeveloperWorkspaceState::Populated
        );
    }

    #[test]
    fn developer_workspace_copy_is_plain_and_visible() {
        assert_eq!(
            developer_workspace_copy(DeveloperWorkspaceState::WaitingForScan),
            "Complete the foreground scan to inspect developer storage."
        );
        assert_eq!(
            developer_workspace_copy(DeveloperWorkspaceState::Loading),
            "Reading retained scan evidence and fixed read-only tool output…"
        );
        assert_eq!(
            developer_workspace_copy(DeveloperWorkspaceState::Empty),
            "No developer storage passed the bounded evidence checks."
        );
        assert_eq!(
            developer_workspace_copy(DeveloperWorkspaceState::DockerUnavailable),
            "Measured storage is available; Docker's inside-VM detail is unavailable."
        );
        assert_eq!(
            developer_workspace_copy(DeveloperWorkspaceState::Failed),
            "Developer workspace could not be loaded."
        );
    }

    #[test]
    fn docker_footprint_copy_never_claims_a_missing_measurement() {
        assert_eq!(
            docker_footprint_copy(true),
            "Measured on disk; inside-VM categories are explanatory and uncounted."
        );
        assert_eq!(
            docker_footprint_copy(false),
            "Docker's on-disk footprint was not measured in this scan; inside-VM categories remain explanatory and uncounted."
        );
    }

    #[test]
    fn developer_worker_starts_only_on_an_explicit_ready_open() {
        assert!(should_start_developer_worker(true, true, false, false));
        assert!(!should_start_developer_worker(false, true, false, false));
        assert!(!should_start_developer_worker(true, false, false, false));
        assert!(!should_start_developer_worker(true, true, true, false));
        assert!(!should_start_developer_worker(true, true, false, true));
    }

    #[test]
    fn new_scan_invalidates_developer_report_and_error() {
        let mut report = Some(developer::DeveloperReport::default());
        let mut error = Some("old worker".to_string());
        invalidate_developer_workspace(&mut report, &mut error);
        assert!(report.is_none());
        assert!(error.is_none());
    }

    #[test]
    fn forecast_confidence_uses_plain_language() {
        assert_eq!(confidence_copy(Confidence::Early), "Early estimate");
        assert_eq!(
            confidence_copy(Confidence::Developing),
            "Developing estimate"
        );
        assert_eq!(confidence_copy(Confidence::Reliable), "Reliable estimate");
    }

    #[test]
    fn forecast_headline_is_honest_for_non_estimate_states() {
        assert_eq!(
            forecast_headline(&ForecastState::NeedHistory {
                observations: 2,
                span_days: 6,
            }),
            "Not enough history yet — 2 compatible scan(s) across 6 day(s)."
        );
        assert_eq!(
            forecast_headline(&ForecastState::AlreadyLow {
                free_bytes: 9 * GB,
                threshold_bytes: 10 * GB,
            }),
            "Storage is already below your low-space threshold."
        );
        assert_eq!(
            forecast_headline(&ForecastState::Flat {
                observations: 5,
                span_days: 14,
            }),
            "Storage use is roughly steady."
        );
        assert_eq!(
            forecast_headline(&ForecastState::Improving {
                bytes_per_day: -GB,
                observations: 5,
                span_days: 14,
            }),
            "Free space is improving across recent scans."
        );
        assert_eq!(
            forecast_headline(&ForecastState::Volatile {
                observations: 5,
                span_days: 14,
            }),
            "Recent storage changes are too volatile for an honest forecast."
        );
    }

    #[test]
    fn forecast_headline_rounds_estimate_range_up_to_weeks() {
        let estimate = crate::forecast::StorageForecast {
            confidence: Confidence::Developing,
            days_low: 8,
            days_high: 29,
            bytes_per_day: GB,
            observations: 6,
            span_days: 16,
            latest_free_bytes: 40 * GB,
            threshold_bytes: 10 * GB,
        };
        assert_eq!(
            forecast_headline(&ForecastState::Estimate(estimate)),
            "At the recent rate, storage may become low in about 2–5 weeks."
        );
    }

    #[test]
    fn guided_layout_reserves_a_fixed_non_overlapping_action_area() {
        let content = Rect::from_min_size(pos2(0.0, 0.0), vec2(390.0, 650.0));
        let layout = guided_layout(content);
        assert_eq!(layout.action.height(), 82.0);
        assert!(layout.nav.max.y < layout.body.min.y);
        assert!(layout.body.max.y < layout.action.min.y);
        assert!(layout.body.height() > 400.0);
    }

    #[test]
    fn workspace_layout_preserves_map_space_at_minimum_window() {
        let full = Rect::from_min_size(Pos2::ZERO, vec2(1156.0, 564.0));
        let layout = WorkspaceLayout::from_rect(full);
        assert_eq!(layout.overview.height(), 128.0);
        assert!(layout.map.height() >= 410.0);
        assert!(layout.map.width() >= 760.0);
        assert!(layout.map.max.x <= full.max.x - 320.0);
        assert_eq!(layout.overview.min, full.min);
        assert_eq!(layout.map.max.y, full.max.y);
    }

    #[test]
    fn workspace_layout_keeps_twelve_point_gap() {
        let full = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
        let layout = WorkspaceLayout::from_rect(full);
        assert_eq!(layout.map.min.y - layout.overview.max.y, 12.0);
    }

    #[test]
    fn adaptive_native_opens_on_summary_without_the_activity_drawer() {
        let app = App::new();
        assert_eq!(app.rail_view, RailView::Summary);
        assert!(!app.activity_open);
    }

    #[test]
    fn rail_back_returns_each_detail_view_to_summary() {
        assert_eq!(rail_back_target(RailView::Summary), None);
        assert_eq!(
            rail_back_target(RailView::GuidedReclaim),
            Some(RailView::Summary)
        );
        assert_eq!(rail_back_target(RailView::Reclaim), Some(RailView::Summary));
        assert_eq!(
            rail_back_target(RailView::Insights),
            Some(RailView::Summary)
        );
        assert_eq!(rail_back_target(RailView::Moved), Some(RailView::Insights));
        assert_eq!(rail_back_target(RailView::Growth), Some(RailView::Insights));
        assert_eq!(
            rail_back_target(RailView::Developer),
            Some(RailView::Insights)
        );
        assert_eq!(rail_back_target(RailView::Apfs), Some(RailView::Insights));
        assert_eq!(
            rail_back_target(RailView::Leftovers),
            Some(RailView::Insights)
        );
        assert_eq!(
            rail_back_target(RailView::Monitor),
            Some(RailView::Insights)
        );
        assert_eq!(
            rail_back_target(RailView::FileReview),
            Some(RailView::Insights)
        );
        assert_eq!(
            rail_back_target(RailView::ReclaimHistory),
            Some(RailView::Insights)
        );
    }

    #[test]
    fn recovery_copy_is_plain_and_never_promises_permanent_undo() {
        assert_eq!(receipt_state_copy(&ReceiptState::Ready), "Ready to restore");
        assert_eq!(
            receipt_state_copy(&ReceiptState::Permanent),
            "Permanent — cannot restore"
        );
        assert_eq!(
            receipt_state_copy(&ReceiptState::ManualOnly),
            "Open Trash to restore manually"
        );
        assert_eq!(
            receipt_state_copy(&ReceiptState::Changed),
            "Changed in Trash"
        );
    }

    #[test]
    fn mutation_gate_blocks_every_overlapping_pipeline() {
        let mut app = App::new();
        assert!(!app.mutation_busy());
        app.cleaning = true;
        assert!(app.mutation_busy());
        app.cleaning = false;
        app.offloading = true;
        assert!(app.mutation_busy());
        app.offloading = false;
        app.restoring = true;
        assert!(app.mutation_busy());
        app.restoring = false;
        app.restoring_trash = true;
        assert!(app.mutation_busy());
    }

    #[test]
    fn disconnected_trash_restore_worker_clears_busy_state() {
        let mut app = App::new();
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        app.trash_restore_rx = Some(rx);
        app.restoring_trash = true;

        app.poll_trash_restore();

        assert!(!app.restoring_trash);
        assert!(app.trash_restore_rx.is_none());
        assert!(app
            .ops
            .last()
            .unwrap()
            .text
            .contains("stopped before reporting"));
    }

    #[test]
    fn trash_restore_requires_acknowledgement_ready_state_and_idle_mutation_gate() {
        assert!(can_start_trash_restore(true, &ReceiptState::Ready, false));
        assert!(!can_start_trash_restore(false, &ReceiptState::Ready, false));
        assert!(!can_start_trash_restore(
            true,
            &ReceiptState::Changed,
            false
        ));
        assert!(!can_start_trash_restore(true, &ReceiptState::Ready, true));
    }

    #[test]
    fn reclaim_history_rows_reserve_a_non_overlapping_action_column() {
        let columns = reclaim_history_row_columns(276.0);
        assert_eq!(columns.gutter, 8.0);
        assert_eq!(columns.utility_width, 104.0);
        assert_eq!(columns.text_width, 164.0);
        assert!(
            columns.text_width + columns.gutter + columns.utility_width <= 276.0,
            "reclaim-history columns must never exceed the available width"
        );
    }

    #[test]
    fn review_rows_reserve_a_non_overlapping_utility_column() {
        let columns = review_row_columns(276.0);

        assert_eq!(columns.text_width, 196.0);
        assert_eq!(columns.gutter, 8.0);
        assert_eq!(columns.utility_width, 72.0);
        assert!(
            columns.text_width + columns.gutter + columns.utility_width <= 276.0,
            "review-row columns must never exceed the available width"
        );
    }

    #[test]
    fn history_records_completed_scans_only() {
        assert!(should_record_history(ScanState::Done));
        assert!(!should_record_history(ScanState::Idle));
        assert!(!should_record_history(ScanState::Running));
        assert!(!should_record_history(ScanState::Aborted));
    }

    #[test]
    fn map_actions_match_real_synthetic_and_denied_items() {
        assert_eq!(
            map_item_actions(true, false, false, true, false),
            MapItemActions {
                open: true,
                reveal: true,
                move_to_ssd: false,
            }
        );
        assert_eq!(
            map_item_actions(true, false, false, true, true),
            MapItemActions {
                open: true,
                reveal: true,
                move_to_ssd: true,
            }
        );
        assert_eq!(
            map_item_actions(false, false, false, true, true),
            MapItemActions {
                open: false,
                reveal: true,
                move_to_ssd: true,
            }
        );
        assert_eq!(
            map_item_actions(false, true, false, false, true),
            MapItemActions {
                open: false,
                reveal: false,
                move_to_ssd: false,
            }
        );
        assert_eq!(
            map_item_actions(true, false, true, true, true),
            MapItemActions {
                open: false,
                reveal: true,
                move_to_ssd: true,
            }
        );
    }

    #[test]
    fn map_hints_explain_discoverable_actions_without_modifiers() {
        assert_eq!(
            map_item_hint(true, false, false),
            "Click to open · Right-click for actions"
        );
        assert_eq!(
            map_item_hint(false, false, false),
            "Right-click for actions"
        );
        assert_eq!(map_item_hint(false, true, false), "Combined smaller items");
        assert_eq!(
            map_item_hint(true, false, true),
            "Access unavailable · Grant Full Disk Access to inspect"
        );
    }

    #[test]
    fn back_target_is_one_level_and_inert_at_root() {
        assert_eq!(back_target(0), None);
        assert_eq!(back_target(1), Some(0));
        assert_eq!(back_target(3), Some(2));
    }

    #[test]
    fn control_click_opens_the_menu_without_primary_navigation() {
        let actions = MapItemActions {
            open: true,
            reveal: true,
            move_to_ssd: true,
        };
        assert!(should_open_from_primary(actions, true, false));
        assert!(!should_open_from_primary(actions, true, true));
    }

    #[test]
    fn escape_closes_an_open_menu_before_navigating_back() {
        assert!(should_navigate_back_on_escape(true, false, false));
        assert!(!should_navigate_back_on_escape(true, true, false));
        assert!(!should_navigate_back_on_escape(true, false, true));
        assert!(!should_navigate_back_on_escape(false, false, false));
    }
}

/// Quiet native panel surface with a compact title row. Returns its content rect.
fn panel_chrome(ui: &egui::Ui, rect: Rect, title: &str, sub: Option<(String, Color32)>) -> Rect {
    let palette = theme::palette(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, Rounding::same(12.0), palette.surface);
    p.rect_stroke(rect, Rounding::same(12.0), Stroke::new(1.0, palette.edge));
    p.text(
        rect.min + vec2(14.0, 9.0),
        Align2::LEFT_TOP,
        title,
        theme::display_md(13.0),
        palette.muted,
    );
    if let Some((sub, color)) = sub {
        p.text(
            pos2(rect.max.x - 14.0, rect.min.y + 10.0),
            Align2::RIGHT_TOP,
            sub,
            theme::body(10.0),
            color,
        );
    }
    Rect::from_min_max(rect.min + vec2(0.0, 34.0), rect.max)
}

fn circle_arc_points(c: Pos2, r: f32, fraction: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let t = fraction * i as f32 / n as f32;
            let a = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

fn ellipse_points(c: Pos2, rx: f32, ry: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            c + vec2(a.cos() * rx, a.sin() * ry)
        })
        .collect()
}

fn draw_brand_mark(ui: &egui::Ui, rect: Rect) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter();
    let center = rect.center() + vec2(0.0, -4.0);
    for (index, y) in [0.0, 5.5, 11.0].iter().enumerate() {
        let color = if index == 0 {
            palette.accent
        } else {
            palette.muted
        };
        painter.add(egui::Shape::line(
            ellipse_points(center + vec2(0.0, *y), 9.0, 3.5, 24),
            Stroke::new(1.4, color),
        ));
    }

    painter.line_segment(
        [center + vec2(-9.0, 0.0), center + vec2(-9.0, 11.0)],
        Stroke::new(1.4, palette.muted),
    );
    painter.line_segment(
        [center + vec2(9.0, 0.0), center + vec2(9.0, 11.0)],
        Stroke::new(1.4, palette.muted),
    );
    painter.circle_filled(center, 1.8, palette.accent);
}

fn ghost_button(ui: &mut egui::Ui, text: &str, hot: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let font = theme::body(12.0);
    let galley_w = text.chars().count() as f32 * 6.6 + 24.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(galley_w.max(88.0), 32.0), Sense::click());
    let (border, fg, fill) = if hot {
        (
            palette.accent,
            if ui.visuals().dark_mode {
                palette.canvas
            } else {
                Color32::WHITE
            },
            if resp.hovered() {
                palette.accent_dim(230)
            } else {
                palette.accent
            },
        )
    } else {
        (
            palette.edge,
            if resp.hovered() {
                palette.ink
            } else {
                palette.muted
            },
            if resp.hovered() {
                palette.surface_raised
            } else {
                Color32::TRANSPARENT
            },
        )
    };
    let p = ui.painter();
    p.rect_filled(rect, Rounding::same(8.0), fill);
    p.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, border));
    p.text(rect.center(), Align2::CENTER_CENTER, text, font, fg);
    resp
}

fn summary_target_card(
    ui: &mut egui::Ui,
    rect: Rect,
    id_salt: &'static str,
    marker: &str,
    title: &str,
    detail: &str,
    bytes: i64,
    color: Color32,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let response = ui.interact(rect, ui.id().with(id_salt), Sense::click());
    let fill = if response.hovered() {
        palette.surface_raised
    } else {
        palette.surface
    };
    let painter = ui.painter();
    painter.rect_filled(rect, Rounding::same(12.0), fill);
    painter.rect_stroke(rect, Rounding::same(12.0), Stroke::new(1.0, palette.edge));
    let marker_rect = Rect::from_min_size(rect.min + vec2(12.0, 25.0), vec2(18.0, 18.0));
    painter.rect_filled(
        marker_rect,
        Rounding::same(5.0),
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 38),
    );
    painter.text(
        marker_rect.center(),
        Align2::CENTER_CENTER,
        marker,
        theme::display(10.0),
        color,
    );
    painter.text(
        rect.min + vec2(40.0, 15.0),
        Align2::LEFT_TOP,
        title,
        theme::display_md(12.0),
        palette.ink,
    );
    painter.text(
        rect.min + vec2(40.0, 35.0),
        Align2::LEFT_TOP,
        detail,
        theme::body(10.0),
        palette.muted,
    );
    painter.text(
        rect.max - vec2(12.0, 31.0),
        Align2::RIGHT_CENTER,
        fmt_bytes(bytes),
        theme::display_md(12.0),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.booted {
            self.booted = true;
            self.begin_scan();
            self.begin_move_refresh();
            self.begin_growth_refresh();
            self.begin_apfs_refresh();
        }
        if self.scan_done() && !self.recs_built {
            self.on_scan_finished();
        }
        self.poll_clean_events();
        self.poll_offload();
        self.poll_history();
        self.poll_moves();
        self.poll_restore();
        self.poll_reclaim_history();
        self.poll_trash_restore();
        self.poll_growth_watch();
        self.poll_developer();
        self.poll_apfs();
        self.poll_leftovers();
        self.poll_file_review();
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            if self.trash_restore_dialog.take().is_some() {
                self.trash_restore_hold = 0.0;
            } else if self.restore_dialog.take().is_some() {
                self.restore_hold = 0.0;
            } else if self.dialog.is_none() {
                if let Some(target) = rail_back_target(self.rail_view) {
                    if self.rail_view == RailView::Reclaim && !self.cleaning {
                        self.guided_goal_for_review = None;
                    }
                    self.rail_view = target;
                }
            }
        }
        if self.stats_at.elapsed() > Duration::from_secs(5) {
            self.stats = disk_stats();
            self.stats_at = Instant::now();
        }
        self.update_menu_monitor();
        if self.scanning()
            || self.cleaning
            || self.zoom.is_some()
            || self.stamp.is_some()
            || self.offloading
            || self.dialog.is_some()
            || self.history_rx.is_some()
            || self.moves_rx.is_some()
            || self.restoring
            || self.restore_dialog.is_some()
            || self.reclaim_history_rx.is_some()
            || self.restoring_trash
            || self.trash_restore_dialog.is_some()
            || self.trash_restore_rx.is_some()
            || self.growth_watch_rx.is_some()
            || self.developer_rx.is_some()
            || self.apfs_rx.is_some()
            || self.leftovers_rx.is_some()
            || self.file_review_rx.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(40));
        }

        self.top_bar(ctx);
        if self.activity_open {
            self.ops_panel(ctx);
        }
        self.central(ctx);
        self.offload_dialog(ctx);
        self.restore_dialog(ctx);
        self.trash_restore_dialog(ctx);
        self.stamp_overlay(ctx);
    }
}

impl App {
    fn top_bar(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        egui::TopBottomPanel::top("topbar")
            .exact_height(48.0)
            .frame(
                Frame::none()
                    .fill(palette.toolbar)
                    .stroke(Stroke::new(1.0, palette.edge_soft))
                    .inner_margin(Margin::symmetric(16.0, 0.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let (mark, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    draw_brand_mark(ui, mark);
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new("DiskDeck")
                            .font(theme::display(20.0))
                            .color(palette.ink),
                    );
                    ui.add_space(12.0);
                    ui.label(RichText::new("Macintosh HD").font(theme::body(11.5)).color(palette.faint));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let scanning = self.scanning();
                        let label = if scanning {
                            "Stop scan"
                        } else if self.scan.is_some() {
                            "Rescan"
                        } else {
                            "Scan now"
                        };
                        if ghost_button(ui, label, true).clicked() {
                            if scanning {
                                if let Some(s) = &self.scan {
                                    s.cancel.store(true, Relaxed);
                                }
                                self.ops(OpsKind::Amber, "scan abort requested");
                            } else {
                                self.begin_scan();
                            }
                        }
                        ui.add_space(8.0);
                        let fda = ghost_button(ui, "Full Disk Access", false);
                        if fda.clicked() {
                            open_full_disk_access();
                            self.ops(OpsKind::Info,
                                "opening System Settings → Privacy → Full Disk Access — enable DiskDeck, then rescan");
                        }
                        fda.on_hover_text(
                            "If parts of the disk show NO ACCESS, grant DiskDeck Full Disk Access and rescan. A residual count (~185) of root-only system dirs is normal.",
                        );
                        ui.add_space(8.0);
                        let status = if self.cleaning {
                            "Reclaiming"
                        } else if self.scanning() {
                            "Scanning"
                        } else if self.scan_done() {
                            "Scan complete"
                        } else {
                            "Activity"
                        };
                        let activity = ui.add(
                            egui::Button::new(
                                RichText::new(status)
                                    .font(theme::display_md(10.5))
                                    .color(if self.scanning() || self.cleaning {
                                        palette.accent
                                    } else {
                                        palette.muted
                                    }),
                            )
                            .fill(if self.activity_open {
                                palette.surface_raised
                            } else {
                                Color32::TRANSPARENT
                            })
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(7.0)),
                        );
                        if activity.clicked() {
                            self.activity_open = !self.activity_open;
                        }
                        activity.on_hover_text("Show or hide scan activity");
                    });
                });
            });
    }

    fn central(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        egui::CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(palette.canvas)
                    .inner_margin(Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                let full = ui.available_rect_before_wrap();
                let layout = WorkspaceLayout::from_rect(full);

                self.draw_capacity(ui, layout.overview);
                self.draw_map(ui, layout.map);
                self.draw_recs(ui, layout.rail);
            });
    }

    fn draw_capacity(&self, ui: &egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let content = panel_chrome(
            ui,
            rect,
            "Macintosh HD · storage used",
            Some((
                if self.scanning() {
                    "Scanning".to_string()
                } else if self.scan_done() {
                    "Scan complete".to_string()
                } else {
                    "Ready".to_string()
                },
                if self.scanning() {
                    palette.accent
                } else {
                    palette.faint
                },
            )),
        );
        let p = ui.painter();
        let c = pos2(content.max.x - 54.0, content.center().y + 1.0);
        let r = 32.0;
        p.circle_stroke(c, r, Stroke::new(7.0, palette.edge_soft));
        let frac = (self.stats.used_pct / 100.0).clamp(0.0, 1.0) as f32;
        if frac > 0.005 {
            let color = if self.stats.used_pct >= 85.0 {
                palette.danger
            } else if self.stats.used_pct >= 70.0 {
                palette.caution
            } else {
                palette.accent
            };
            p.add(egui::Shape::line(
                circle_arc_points(c, r, frac, 64),
                Stroke::new(7.0, color),
            ));
        }
        p.text(
            c,
            Align2::CENTER_CENTER,
            format!("{:.0}%", self.stats.used_pct),
            theme::mono(11.0),
            palette.ink,
        );
        p.text(
            content.min + vec2(16.0, 12.0),
            Align2::LEFT_TOP,
            fmt_bytes(self.stats.used),
            theme::display(30.0),
            palette.ink,
        );
        p.text(
            content.min + vec2(16.0, 46.0),
            Align2::LEFT_TOP,
            format!(
                "of {}  ·  {} free",
                fmt_bytes(self.stats.total),
                fmt_bytes(self.stats.free),
            ),
            theme::body(11.0),
            palette.muted,
        );
        let history = if self.history_baseline {
            Some((
                "Baseline saved · compare after the next scan".to_string(),
                palette.muted,
            ))
        } else {
            self.regrowth.as_ref().map(|summary| {
                let mut text = format!("Since last scan: {}", fmt_delta(summary.total_delta));
                if let Some(growth) = summary.growers.first() {
                    text.push_str(&format!(
                        "  ·  {} {}",
                        growth.path.display(),
                        fmt_delta(growth.bytes_delta)
                    ));
                }
                (
                    text,
                    if summary.total_delta > 0 {
                        palette.caution
                    } else {
                        palette.safe
                    },
                )
            })
        };
        if let Some((text, color)) = history {
            p.text(
                content.min + vec2(16.0, 68.0),
                Align2::LEFT_TOP,
                text,
                theme::body(10.5),
                color,
            );
        }
    }

    #[allow(dead_code)]
    fn draw_telemetry(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let scanning = self.scanning();
        let (state_txt, state_color) = if scanning {
            ("Scanning".to_string(), palette.accent)
        } else if let Some(s) = &self.scan {
            (
                if s.state() == ScanState::Done {
                    "Scan complete"
                } else {
                    "Scan stopped"
                }
                .to_string(),
                palette.faint,
            )
        } else {
            ("Ready".to_string(), palette.faint)
        };
        let content = panel_chrome(ui, rect, "Scan details", Some((state_txt, state_color)));
        let p = ui.painter();

        let (files, bytes, denied) = self
            .scan
            .as_ref()
            .map(|s| (s.root.files(), s.root.bytes(), s.denied.load(Relaxed)))
            .unwrap_or((0, 0, 0));
        let elapsed = self.scan.as_ref().map_or("0:00".to_string(), |s| {
            if s.state() == ScanState::Running {
                fmt_elapsed(s.started.elapsed())
            } else {
                fmt_elapsed(Duration::from_millis(
                    s.duration_ms.load(Relaxed).max(0) as u64
                ))
            }
        });

        let counters = [
            ("Items mapped", files.max(0).to_string(), palette.ink),
            ("Footprint", fmt_bytes(bytes), palette.ink),
            (
                "No access",
                denied.to_string(),
                if denied > 0 {
                    palette.danger
                } else {
                    palette.ink
                },
            ),
            ("Elapsed", elapsed, palette.ink),
        ];
        let col_w = content.width() / counters.len() as f32;
        for (i, (label, value, color)) in counters.iter().enumerate() {
            let x = content.min.x + col_w * i as f32 + 16.0;
            p.text(
                pos2(x, content.min.y + 12.0),
                Align2::LEFT_TOP,
                label,
                theme::body(9.5),
                palette.faint,
            );
            p.text(
                pos2(x, content.min.y + 28.0),
                Align2::LEFT_TOP,
                value,
                theme::mono(17.0),
                *color,
            );
            if i > 0 {
                let lx = content.min.x + col_w * i as f32;
                p.line_segment(
                    [
                        pos2(lx, content.min.y + 12.0),
                        pos2(lx, content.min.y + 54.0),
                    ],
                    Stroke::new(1.0, palette.edge_soft),
                );
            }
        }
        // NO ACCESS hover explainer
        let na_rect = Rect::from_min_size(
            pos2(content.min.x + col_w * 2.0, content.min.y + 8.0),
            vec2(col_w, 48.0),
        );
        let resp = ui.interact(na_rect, ui.id().with("noaccess"), Sense::hover());
        if denied > 0 {
            resp.on_hover_text(
                "Locations the scan couldn't read. Before granting Full Disk Access these are mostly your gated folders (Desktop, Documents, Mail…). After granting it, what remains is root-only macOS internals — Spotlight index, filesystem journal, audit logs. That's normal: no app you run can read those, and there's nothing reclaimable inside.",
            );
        }

        // ticker
        let ticker = Rect::from_min_max(
            pos2(content.min.x + 14.0, content.max.y - 31.0),
            pos2(content.max.x - 14.0, content.max.y - 10.0),
        );
        p.rect_filled(ticker, Rounding::same(6.0), palette.surface_raised);
        p.rect_stroke(
            ticker,
            Rounding::same(6.0),
            Stroke::new(1.0, palette.edge_soft),
        );
        let cur = self
            .scan
            .as_ref()
            .and_then(|s| s.current.lock().ok().map(|c| c.clone()))
            .unwrap_or_default();
        let ticker_txt = if scanning && !cur.is_empty() {
            tail_str(&rules::display(Path::new(&cur)), 76)
        } else if self.scan_done() {
            "volume charted".into()
        } else {
            "awaiting scan command".into()
        };
        p.text(
            ticker.min + vec2(8.0, 5.0),
            Align2::LEFT_TOP,
            format!("▸ {ticker_txt}"),
            theme::mono(10.0),
            if scanning {
                palette.accent
            } else {
                palette.muted
            },
        );

        let bar = Rect::from_min_max(
            pos2(ticker.min.x, ticker.max.y - 3.0),
            pos2(ticker.max.x, ticker.max.y),
        );
        p.rect_filled(bar, Rounding::same(2.0), palette.edge_soft);
        if scanning {
            let t = ui.input(|i| i.time) as f32;
            let seg = 26.0;
            let off = (t * 90.0) % (seg * 2.0);
            let bp = p.with_clip_rect(bar);
            let mut x = bar.min.x - seg * 2.0 + off;
            while x < bar.max.x {
                bp.rect_filled(
                    Rect::from_min_size(pos2(x, bar.min.y), vec2(seg * 0.55, bar.height())),
                    Rounding::ZERO,
                    palette.accent_dim(180),
                );
                x += seg;
            }
        } else if self.scan_done() {
            p.rect_filled(bar, Rounding::same(2.0), palette.safe_dim(180));
        }
    }

    fn draw_map(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let depth = self.crumbs.len();
        let hint = self.scan.as_ref().map(|scan| {
            (
                format!("{} items mapped", fmt_count(scan.root.files())),
                palette.faint,
            )
        });
        let content = panel_chrome(ui, rect, "Storage map", hint);

        // Clickable breadcrumb trail plus a persistent, root-safe Back button.
        let crumb_rect = Rect::from_min_size(
            content.min + vec2(12.0, 3.0),
            vec2(content.width() - 24.0, 24.0),
        );
        let map_rect = Rect::from_min_max(
            pos2(content.min.x + 12.0, crumb_rect.max.y + 4.0),
            content.max - vec2(12.0, 10.0),
        );
        let mut go_to: Option<usize> = None; // truncate crumbs to this depth
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(crumb_rect), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let seg = |ui: &mut egui::Ui, label: &str, current: bool| -> bool {
                    if current {
                        ui.label(
                            RichText::new(label)
                                .font(theme::mono(10.5))
                                .color(palette.ink),
                        );
                        return false;
                    }
                    ui.add(
                        egui::Button::new(
                            RichText::new(label)
                                .font(theme::mono(10.5))
                                .color(palette.accent),
                        )
                        .frame(false),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                };
                if seg(ui, "Data", depth == 0) {
                    go_to = Some(0);
                }
                for i in 0..depth {
                    ui.label(
                        RichText::new("/")
                            .font(theme::mono(10.5))
                            .color(palette.faint),
                    );
                    let name = self.crumbs[i].name.clone();
                    if seg(ui, &tail_str(&name, 28), i + 1 == depth) {
                        go_to = Some(i + 1);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let back = ui.add_enabled(
                        depth > 0,
                        egui::Button::new(RichText::new("← Back").font(theme::body(11.0)).color(
                            if depth > 0 {
                                palette.accent
                            } else {
                                palette.faint
                            },
                        ))
                        .fill(palette.accent_dim(if depth > 0 { 16 } else { 0 }))
                        .stroke(Stroke::new(
                            1.0,
                            if depth > 0 {
                                palette.accent_dim(90)
                            } else {
                                palette.edge_soft
                            },
                        ))
                        .rounding(Rounding::same(6.0)),
                    );
                    if back
                        .on_hover_text("Return to the previous folder")
                        .clicked()
                    {
                        go_to = back_target(depth);
                    }
                });
            });
        });
        if let Some(d) = go_to {
            self.crumbs.truncate(d);
            self.view = self.crumbs.last().cloned();
            self.zoom = None;
        }

        let Some(node) = self.view_node() else {
            ui.painter().text(
                map_rect.center(),
                Align2::CENTER_CENTER,
                "No map data yet",
                theme::body(13.0),
                palette.faint,
            );
            return;
        };

        let items = treemap::collect_items(&node);
        if items.is_empty() {
            let msg = if self.scanning() {
                "Mapping storage…"
            } else {
                "Nothing to show"
            };
            ui.painter().text(
                map_rect.center(),
                Align2::CENTER_CENTER,
                msg,
                theme::body(13.0),
                palette.faint,
            );
            return;
        }
        let laid = treemap::squarify(&items, map_rect);

        let zoom = self.zoom.and_then(|(src, t0)| {
            let t = t0.elapsed().as_secs_f32() / ZOOM_SECS;
            if t >= 1.0 {
                None
            } else {
                Some((src, 1.0 - (1.0 - t).powi(3)))
            }
        });
        if zoom.is_none() {
            self.zoom = None;
        }

        let interactions_enabled = zoom.is_none();
        let hover = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|position| map_rect.contains(*position));
        let hovered = treemap::paint(ui, map_rect, &items, &laid, hover, zoom);
        let mut requested_action: Option<MapActionRequest> = None;
        let mut menu_open = false;
        let mut menu_was_open = false;
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();

        for &(idx, item_rect) in &laid {
            let item = &items[idx];
            let response = ui.interact(
                item_rect,
                ui.id().with(("treemap-item", idx)),
                Sense::click(),
            );
            let item_node = item.node.clone();
            let offload_block = item_node
                .as_ref()
                .and_then(|node| classify_movable(&strip_data_root(&node.path), &home).err());
            let actions = map_item_actions(
                item.is_dir,
                item.synthetic,
                item.denied,
                item.node.is_some(),
                offload_block.is_none(),
            );

            let primary_clicked = response.clicked();
            let control_down = ui.input(|input| input.modifiers.ctrl);
            if interactions_enabled
                && should_open_from_primary(actions, primary_clicked, control_down)
            {
                if let Some(node) = item_node.clone() {
                    requested_action = Some(MapActionRequest::Open {
                        node,
                        source: item_rect,
                    });
                }
            }

            menu_was_open |= response.context_menu_opened();
            response.context_menu(|menu_ui| {
                menu_ui.set_min_width(180.0);
                if menu_ui
                    .add_enabled(actions.open, egui::Button::new("Open"))
                    .clicked()
                {
                    requested_action = item_node.clone().map(|node| MapActionRequest::Open {
                        node,
                        source: item_rect,
                    });
                    menu_ui.close_menu();
                }
                if menu_ui
                    .add_enabled(actions.reveal, egui::Button::new("Reveal in Finder"))
                    .clicked()
                {
                    requested_action = item_node
                        .as_ref()
                        .map(|node| MapActionRequest::Reveal(node.path.clone()));
                    menu_ui.close_menu();
                }
                menu_ui.separator();
                let mut move_response =
                    menu_ui.add_enabled(actions.move_to_ssd, egui::Button::new("Move to SSD…"));
                if let Some(block) = offload_block {
                    move_response = move_response.on_disabled_hover_text(block.message());
                }
                if move_response.clicked() {
                    requested_action = item_node.as_ref().map(|node| MapActionRequest::MoveToSsd {
                        path: strip_data_root(&node.path),
                        bytes: node.bytes(),
                    });
                    menu_ui.close_menu();
                }
            });
            menu_open |= response.context_menu_opened();
        }

        if let Some(idx) = hovered.filter(|_| !menu_open) {
            let it = &items[idx];
            egui::Area::new(ui.id().with("tm_tt"))
                .order(egui::Order::Tooltip)
                .interactable(false)
                .fixed_pos(hover.unwrap_or(map_rect.center()) + vec2(16.0, 18.0))
                .show(ui.ctx(), |tui| {
                    Frame::none()
                        .fill(palette.surface_raised)
                        .stroke(Stroke::new(1.0, palette.edge))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(11.0, 8.0))
                        .show(tui, |tui| {
                            tui.label(
                                RichText::new(&it.label)
                                    .font(theme::body(12.5))
                                    .color(palette.ink)
                                    .strong(),
                            );
                            let meta = if it.synthetic {
                                format!(
                                    "{} · {} small items aggregated",
                                    fmt_bytes(it.bytes),
                                    fmt_count(it.files)
                                )
                            } else {
                                let total = node.bytes().max(1);
                                format!(
                                    "{} · {:.1}% of this view · {} files",
                                    fmt_bytes(it.bytes),
                                    it.bytes as f64 / total as f64 * 100.0,
                                    fmt_count(it.files)
                                )
                            };
                            tui.label(
                                RichText::new(meta)
                                    .font(theme::mono(10.5))
                                    .color(palette.muted),
                            );
                            tui.label(
                                RichText::new(map_item_hint(it.is_dir, it.synthetic, it.denied))
                                    .font(theme::body(9.5))
                                    .color(palette.faint),
                            );
                        });
                });
        }

        match requested_action {
            Some(MapActionRequest::Open { node, source }) => {
                self.crumbs.push(node.clone());
                self.view = Some(node);
                self.zoom = Some((source, Instant::now()));
            }
            Some(MapActionRequest::Reveal(path)) => reveal_in_finder(&path),
            Some(MapActionRequest::MoveToSsd { path, bytes }) => {
                self.open_offload_dialog(path, bytes);
            }
            None => {}
        }

        let escape_pressed = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if should_navigate_back_on_escape(escape_pressed, menu_was_open, menu_open) {
            if let Some(target) = back_target(self.crumbs.len()) {
                self.crumbs.truncate(target);
                self.view = self.crumbs.last().cloned();
                self.zoom = None;
            }
        }
    }

    fn draw_recs(&mut self, ui: &mut egui::Ui, rect: Rect) {
        match self.rail_view {
            RailView::Summary => {
                self.draw_reclaim_summary(ui, rect);
                return;
            }
            RailView::GuidedReclaim => {
                self.draw_guided_reclaim(ui, rect);
                return;
            }
            RailView::Insights => {
                self.draw_insights(ui, rect);
                return;
            }
            RailView::Moved => {
                self.draw_moved_items(ui, rect);
                return;
            }
            RailView::Growth => {
                self.draw_growth_watch(ui, rect);
                return;
            }
            RailView::Developer => {
                self.draw_developer_lens(ui, rect);
                return;
            }
            RailView::Apfs => {
                self.draw_apfs_accounting(ui, rect);
                return;
            }
            RailView::Leftovers => {
                self.draw_app_leftovers(ui, rect);
                return;
            }
            RailView::Monitor => {
                self.draw_menu_monitor(ui, rect);
                return;
            }
            RailView::FileReview => {
                self.draw_file_review(ui, rect);
                return;
            }
            RailView::ReclaimHistory => {
                self.draw_reclaim_history(ui, rect);
                return;
            }
            RailView::Reclaim => {}
        }

        let palette = theme::palette(ui.ctx());
        let meta = if self.recs.is_empty() {
            String::new()
        } else {
            let total: i64 = self.recs.iter().map(|r| r.rec.bytes).sum();
            format!("{} targets · {}", self.recs.len(), fmt_bytes(total))
        };
        let content = panel_chrome(ui, rect, "Review targets", Some((meta, palette.faint)));
        let footer_h = 64.0;
        let nav_rect = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 28.0),
        );
        let list_rect = Rect::from_min_max(
            pos2(content.min.x, nav_rect.max.y + 2.0),
            pos2(content.max.x, content.max.y - footer_h),
        );
        let footer_rect = Rect::from_min_max(pos2(content.min.x, list_rect.max.y), content.max);

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav_rect), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("← Reclaim summary")
                            .font(theme::display_md(10.5))
                            .color(palette.accent),
                    )
                    .frame(false),
                )
                .clicked()
            {
                if !self.cleaning {
                    self.guided_goal_for_review = None;
                }
                self.rail_view = RailView::Summary;
            }
        });

        let mut reveal: Option<std::path::PathBuf> = None;
        let mut offload_req: Option<(std::path::PathBuf, i64)> = None;
        ui.allocate_new_ui(
            egui::UiBuilder::new().max_rect(list_rect.shrink2(vec2(10.0, 4.0))),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for tier in [Tier::Safe, Tier::Caution] {
                            let group: Vec<usize> = self
                                .recs
                                .iter()
                                .enumerate()
                                .filter(|(_, r)| r.rec.tier == tier)
                                .map(|(i, _)| i)
                                .collect();
                            if group.is_empty() {
                                continue;
                            }
                            let (label, color) = match tier {
                                Tier::Safe => ("Safe · regenerates automatically", palette.safe),
                                Tier::Caution => {
                                    ("Review · may require a download", palette.caution)
                                }
                            };
                            let total: i64 = group.iter().map(|&i| self.recs[i].rec.bytes).sum();
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                let (dot, _) =
                                    ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 3.5, color);
                                ui.label(
                                    RichText::new(label)
                                        .font(theme::display_md(10.5))
                                        .color(color),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(fmt_bytes(total))
                                                .font(theme::mono(10.5))
                                                .color(palette.muted),
                                        );
                                    },
                                );
                            });
                            ui.add_space(5.0);
                            for i in group {
                                if let Some(path) = self.rec_card(ui, i, &mut offload_req) {
                                    reveal = Some(path);
                                }
                                ui.add_space(6.0);
                            }
                        }
                        ui.add_space(6.0);
                    });
            },
        );
        if let Some(path) = reveal {
            reveal_in_finder(&path);
        }
        if let Some((path, size)) = offload_req {
            self.open_offload_dialog(path, size);
        }
        self.reclaim_footer(ui, footer_rect);
    }

    fn draw_reclaim_summary(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let total: i64 = self.recs.iter().map(|row| row.rec.bytes).sum();
        let safe: i64 = self
            .recs
            .iter()
            .filter(|row| row.rec.tier == Tier::Safe)
            .map(|row| row.rec.bytes)
            .sum();
        let caution: i64 = self
            .recs
            .iter()
            .filter(|row| row.rec.tier == Tier::Caution)
            .map(|row| row.rec.bytes)
            .sum();
        let selected: i64 = self
            .recs
            .iter()
            .filter(|row| row.checked)
            .map(|row| row.rec.bytes)
            .sum();

        let top = Rect::from_min_size(rect.min, vec2(rect.width(), 84.0));
        ui.painter()
            .rect_filled(top, Rounding::same(12.0), palette.surface);
        ui.painter()
            .rect_stroke(top, Rounding::same(12.0), Stroke::new(1.0, palette.edge));
        ui.painter().text(
            top.min + vec2(14.0, 13.0),
            Align2::LEFT_TOP,
            "Reclaimable",
            theme::display_md(11.0),
            palette.muted,
        );
        ui.painter().text(
            top.min + vec2(14.0, 36.0),
            Align2::LEFT_TOP,
            if self.recs_built {
                fmt_bytes(total)
            } else {
                "Scanning…".to_string()
            },
            theme::display(22.0),
            palette.ink,
        );
        ui.painter().text(
            top.max - vec2(14.0, 22.0),
            Align2::RIGHT_CENTER,
            if self.recs_built {
                format!("{} targets", self.recs.len())
            } else {
                "Read-only scan".to_string()
            },
            theme::body(10.0),
            palette.faint,
        );

        let safe_rect =
            Rect::from_min_size(pos2(rect.min.x, top.max.y + 8.0), vec2(rect.width(), 70.0));
        let caution_rect = Rect::from_min_size(
            pos2(rect.min.x, safe_rect.max.y + 8.0),
            vec2(rect.width(), 70.0),
        );
        let safe_response = summary_target_card(
            ui,
            safe_rect,
            "safe-summary",
            "✓",
            "Safe caches",
            "Regenerates automatically",
            safe,
            palette.safe,
        );
        let caution_response = summary_target_card(
            ui,
            caution_rect,
            "caution-summary",
            "!",
            "Needs review",
            "May require re-download",
            caution,
            palette.caution,
        );
        if (safe_response.clicked() || caution_response.clicked()) && self.recs_built {
            self.rail_view = RailView::Reclaim;
        }

        let moved_rect = Rect::from_min_size(
            pos2(rect.min.x, caution_rect.max.y + 10.0),
            vec2(rect.width(), 34.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(moved_rect), |ui| {
            let button = egui::Button::new(
                RichText::new("Insights")
                    .font(theme::display_md(10.5))
                    .color(palette.accent),
            )
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.edge_soft))
            .rounding(Rounding::same(8.0));
            if ui
                .add_sized(ui.available_size(), button)
                .on_hover_text(
                    "Reclaim history, moved items, growth, developer storage, APFS, and app leftovers",
                )
                .clicked()
            {
                self.rail_view = RailView::Insights;
            }
        });
        let last_reclaim = self.reclaim_history.items.first().map(|last| {
            format!(
                "Last reclaim · {} · {}",
                tail_str(&last.receipt.title, 25),
                receipt_age_copy(last.receipt.completed_at_ms)
            )
        });
        if let Some(detail) = last_reclaim {
            let history_rect = Rect::from_min_size(
                pos2(rect.min.x, moved_rect.max.y + 8.0),
                vec2(rect.width(), 38.0),
            );
            let mut open_history = false;
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(history_rect), |ui| {
                if ui
                    .add_sized(
                        ui.available_size(),
                        egui::Button::new(
                            RichText::new(detail)
                                .font(theme::body(9.5))
                                .color(palette.muted),
                        )
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, palette.edge_soft))
                        .rounding(Rounding::same(8.0)),
                    )
                    .on_hover_text("Open local cleanup receipts and Trash recovery")
                    .clicked()
                {
                    open_history = true;
                }
            });
            if open_history {
                self.rail_view = RailView::ReclaimHistory;
                self.begin_reclaim_history_refresh();
            }
        }
        let footer = Rect::from_min_max(pos2(rect.min.x, rect.max.y - 112.0), rect.max);
        let footer_fill = if ui.visuals().dark_mode {
            Color32::from_rgb(0x14, 0x2a, 0x2c)
        } else {
            Color32::from_rgb(0xf7, 0xfb, 0xfa)
        };
        ui.painter()
            .rect_filled(footer, Rounding::same(12.0), footer_fill);
        ui.painter().rect_stroke(
            footer,
            Rounding::same(12.0),
            Stroke::new(1.0, palette.safe_dim(90)),
        );
        ui.painter().text(
            footer.min + vec2(14.0, 12.0),
            Align2::LEFT_TOP,
            "Selected safely",
            theme::display_md(10.0),
            palette.muted,
        );
        ui.painter().text(
            footer.min + vec2(14.0, 31.0),
            Align2::LEFT_TOP,
            fmt_bytes(selected),
            theme::display(20.0),
            palette.ink,
        );
        let button =
            Rect::from_min_max(footer.min + vec2(10.0, 66.0), footer.max - vec2(10.0, 10.0));
        let enabled = self.recs_built;
        let button_color = if ui.visuals().dark_mode {
            palette.safe
        } else {
            palette.accent
        };
        let response = ui
            .allocate_new_ui(egui::UiBuilder::new().max_rect(button), |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    ui.add_sized(
                        ui.available_size(),
                        egui::Button::new(
                            RichText::new(if enabled {
                                "Free up space"
                            } else {
                                "Scanning for targets…"
                            })
                            .font(theme::display_md(11.0))
                            .color(if ui.visuals().dark_mode {
                                Color32::from_rgb(0x08, 0x2c, 0x29)
                            } else {
                                Color32::WHITE
                            }),
                        )
                        .fill(button_color)
                        .stroke(Stroke::new(1.0, button_color))
                        .rounding(Rounding::same(8.0)),
                    )
                })
                .inner
            })
            .inner;
        if enabled && response.clicked() {
            self.guide_revision = Some(self.recs_revision);
            self.guide_acknowledged = false;
            self.rail_view = RailView::GuidedReclaim;
        }
    }

    fn draw_guided_reclaim(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        if let Some(outcome) = self.guided_outcome.clone() {
            self.draw_guided_outcome(ui, rect, &outcome);
            return;
        }
        let recs: Vec<Rec> = self.recs.iter().map(|row| row.rec.clone()).collect();
        let plan = build_plan(&recs, self.guide_goal_bytes);
        let current = self.guide_revision == Some(self.recs_revision) && !self.scanning();
        let can_apply = can_apply_guided_plan(
            self.guide_acknowledged,
            self.guide_revision,
            self.recs_revision,
            self.scanning(),
            &plan,
        );
        let content = panel_chrome(
            ui,
            rect,
            "Free up space",
            Some(("Safe plan · local only".into(), palette.safe)),
        );
        let layout = guided_layout(content);
        let mut go_back = false;
        let mut choose_goal = None;
        let mut submit_custom = false;
        let mut review_optional = false;
        let mut apply = false;

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(layout.nav), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("← Reclaim summary")
                            .font(theme::display_md(10.5))
                            .color(palette.accent),
                    )
                    .frame(false),
                )
                .clicked()
            {
                go_back = true;
            }
        });

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(layout.body), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("How much space do you need?")
                            .font(theme::display_md(13.0))
                            .color(palette.ink),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        for gb in [10i64, 20, 50] {
                            let active = self.guide_goal_bytes == gb * GB;
                            let button = egui::Button::new(
                                RichText::new(format!("{gb} GB"))
                                    .font(theme::body(10.5))
                                    .color(if active { palette.canvas } else { palette.ink }),
                            )
                            .fill(if active {
                                palette.accent
                            } else {
                                palette.surface_raised
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if active { palette.accent } else { palette.edge },
                            ));
                            if ui.add_sized(vec2(72.0, 30.0), button).clicked() {
                                choose_goal = Some(gb * GB);
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.guide_custom_gb)
                                .hint_text("Custom GB")
                                .desired_width(118.0),
                        );
                        if ui.button("Use custom goal").clicked() {
                            submit_custom = true;
                        }
                    });
                    if let Some(error) = self.guide_goal_error {
                        ui.label(
                            RichText::new(goal_error_copy(error))
                                .font(theme::body(9.5))
                                .color(palette.caution),
                        );
                    }
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(plan_status_copy(&plan))
                            .font(theme::body(10.5))
                            .color(if plan.shortfall_bytes > 0 {
                                palette.caution
                            } else {
                                palette.safe
                            }),
                    );
                    ui.add_space(7.0);

                    for item in &plan.items {
                        let finding = self
                            .recs
                            .iter()
                            .find(|row| row.rec.id == item.id)
                            .map(|row| &row.rec);
                        let title = finding
                            .map(|rec| rec.title.as_str())
                            .unwrap_or(item.id.as_str());
                        Frame::none()
                            .fill(palette.surface_raised)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(8.0))
                            .inner_margin(Margin::symmetric(10.0, 8.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(title)
                                            .font(theme::display_md(10.5))
                                            .color(palette.ink),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}{}",
                                                    if item.estimate { "≈ " } else { "" },
                                                    fmt_bytes(item.bytes)
                                                ))
                                                .font(theme::mono(10.0))
                                                .color(if item.estimate {
                                                    palette.caution
                                                } else {
                                                    palette.safe
                                                }),
                                            );
                                        },
                                    );
                                });
                                if let Some(rec) = finding {
                                    ui.label(
                                        RichText::new(rec.desc)
                                            .font(theme::body(9.5))
                                            .color(palette.muted),
                                    );
                                    ui.label(
                                        RichText::new(format!("Afterward: {}", rec.restore))
                                            .font(theme::body(9.0))
                                            .color(palette.faint),
                                    );
                                }
                            });
                        ui.add_space(6.0);
                    }

                    if plan.caution_bytes > 0 {
                        ui.label(
                            RichText::new(format!(
                                "More options: {} need review and remain unchecked.",
                                fmt_bytes(plan.caution_bytes)
                            ))
                            .font(theme::body(9.5))
                            .color(palette.caution),
                        );
                        if ui.button("Review optional items").clicked() {
                            review_optional = true;
                        }
                    }
                    if !current {
                        ui.label(
                            RichText::new("This plan is stale. Scan again before applying it.")
                                .font(theme::body(9.5))
                                .color(palette.caution),
                        );
                    }
                });
        });

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(layout.action), |ui| {
            ui.checkbox(
                &mut self.guide_acknowledged,
                "I reviewed this Safe plan and will confirm each selected target.",
            );
            let button = egui::Button::new(
                RichText::new("Review this plan")
                    .font(theme::display_md(10.5))
                    .color(if can_apply {
                        if ui.visuals().dark_mode {
                            palette.canvas
                        } else {
                            Color32::WHITE
                        }
                    } else {
                        palette.muted
                    }),
            )
            .fill(if can_apply {
                palette.safe
            } else {
                palette.surface_raised
            })
            .rounding(Rounding::same(8.0));
            if ui
                .add_enabled_ui(can_apply, |ui| {
                    ui.add_sized(vec2(ui.available_width(), 34.0), button)
                })
                .inner
                .clicked()
            {
                apply = true;
            }
        });

        if go_back {
            self.rail_view = RailView::Summary;
        }
        if let Some(goal) = choose_goal {
            if goal <= self.stats.used {
                self.guide_goal_bytes = goal;
                self.guide_goal_error = None;
                self.guide_acknowledged = false;
                self.guide_revision = Some(self.recs_revision);
            } else {
                self.guide_goal_error = Some(GoalError::ExceedsUsedSpace);
            }
        }
        if submit_custom {
            match parse_goal_gb(&self.guide_custom_gb, self.stats.used) {
                Ok(goal) => {
                    self.guide_goal_bytes = goal;
                    self.guide_goal_error = None;
                    self.guide_acknowledged = false;
                    self.guide_revision = Some(self.recs_revision);
                }
                Err(error) => self.guide_goal_error = Some(error),
            }
        }
        if review_optional {
            self.guided_goal_for_review = None;
            self.rail_view = RailView::Reclaim;
        }
        if apply {
            self.accept_guided_plan(&plan);
        }
    }

    fn draw_guided_outcome(&mut self, ui: &mut egui::Ui, rect: Rect, outcome: &ReclaimOutcome) {
        let palette = theme::palette(ui.ctx());
        let content = panel_chrome(
            ui,
            rect,
            "Reclaim result",
            Some(("measured after cleanup".into(), palette.safe)),
        );
        let mut go_back = false;
        let mut scan_again = false;
        let mut show_trash = false;

        ui.allocate_new_ui(
            egui::UiBuilder::new().max_rect(content.shrink2(vec2(12.0, 10.0))),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("← Back to summary")
                                        .font(theme::display_md(10.5))
                                        .color(palette.accent),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            go_back = true;
                        }
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Actually freed")
                                .font(theme::display_md(10.5))
                                .color(palette.muted),
                        );
                        ui.label(
                            RichText::new(fmt_bytes(outcome.actual_freed_bytes))
                                .font(theme::display(24.0))
                                .color(palette.safe),
                        );
                        ui.add_space(8.0);
                        Frame::none()
                            .fill(palette.surface_raised)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(9.0))
                            .inner_margin(Margin::symmetric(11.0, 9.0))
                            .show(ui, |ui| {
                                let planned_prefix = if outcome.planned_estimated_bytes > 0 {
                                    "≈ "
                                } else {
                                    ""
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "Goal {} · planned {planned_prefix}{}",
                                        fmt_bytes(outcome.goal_bytes),
                                        fmt_bytes(outcome.planned_bytes)
                                    ))
                                    .font(theme::body(10.5))
                                    .color(palette.ink),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} reviewed item(s) · {} failed",
                                        outcome.attempted_items, outcome.failed_items
                                    ))
                                    .font(theme::body(9.5))
                                    .color(
                                        if outcome.failed_items > 0 {
                                            palette.caution
                                        } else {
                                            palette.muted
                                        },
                                    ),
                                );
                            });
                        ui.add_space(8.0);
                        if outcome.goal_shortfall_bytes > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "Still short by {} based on space actually freed.",
                                    fmt_bytes(outcome.goal_shortfall_bytes)
                                ))
                                .font(theme::body(10.0))
                                .color(palette.caution),
                            );
                        } else {
                            ui.label(
                                RichText::new("Goal reached with measured free space.")
                                    .font(theme::body(10.0))
                                    .color(palette.safe),
                            );
                        }
                        if outcome.pending_trash_bytes > 0 {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(format!(
                                    "{} is waiting in Trash and is not counted as freed yet.",
                                    fmt_bytes(outcome.pending_trash_bytes)
                                ))
                                .font(theme::body(10.0))
                                .color(palette.caution),
                            );
                            if ui.button("Open Trash").clicked() {
                                show_trash = true;
                            }
                        }
                        ui.add_space(12.0);
                        if ui.button("Scan again").clicked() {
                            scan_again = true;
                        }
                    });
            },
        );

        if show_trash {
            open_trash();
        }
        if scan_again {
            self.begin_scan();
            self.rail_view = RailView::Summary;
        } else if go_back {
            self.rail_view = RailView::Summary;
        }
    }

    fn draw_insights(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let content = panel_chrome(
            ui,
            rect,
            "Insights",
            Some(("local · read-only views".into(), palette.faint)),
        );
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("← Reclaim summary")
                            .font(theme::display_md(10.5))
                            .color(palette.accent),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.rail_view = RailView::Summary;
            }
        });
        let body = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 5.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let moved_count = self
            .moved_items
            .iter()
            .filter(|item| item.state != MoveState::Restored)
            .count();
        let entries = [
            (
                "Reclaim History",
                if self.reclaim_history.items.is_empty() {
                    "Cleanup receipts and verified Trash recovery".into()
                } else {
                    format!(
                        "{} local cleanup receipt(s)",
                        self.reclaim_history.items.len()
                    )
                },
                RailView::ReclaimHistory,
            ),
            (
                "Moved items",
                format!("{moved_count} currently away"),
                RailView::Moved,
            ),
            (
                "Growth Watch",
                format!("{} retained scans", self.growth_watch.timeline.len()),
                RailView::Growth,
            ),
            (
                "Developer Lens",
                "Explain measured tool storage".into(),
                RailView::Developer,
            ),
            (
                "APFS accounting",
                "Container, snapshots, purgeable truth".into(),
                RailView::Apfs,
            ),
            (
                "App leftovers",
                format!("{} evidence-backed finding(s)", self.leftovers.len()),
                RailView::Leftovers,
            ),
            (
                "Menu-bar monitor",
                if self.monitor_settings.enabled {
                    "Enabled · five-minute updates".into()
                } else {
                    "Off · opt in explicitly".into()
                },
                RailView::Monitor,
            ),
            (
                "Duplicate & large-old review",
                "Off · starts only when you ask".into(),
                RailView::FileReview,
            ),
        ];
        let mut open = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (title, detail, target) in entries {
                        let button = egui::Button::new(
                            RichText::new(title)
                                .font(theme::display_md(11.0))
                                .color(palette.ink),
                        )
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, palette.edge_soft))
                        .rounding(Rounding::same(9.0));
                        if ui
                            .add_sized(vec2(ui.available_width(), 42.0), button)
                            .on_hover_text(detail)
                            .clicked()
                        {
                            open = Some(target);
                        }
                        ui.add_space(7.0);
                    }
                });
        });
        if let Some(target) = open {
            self.rail_view = target;
            match target {
                RailView::ReclaimHistory => self.begin_reclaim_history_refresh(),
                RailView::Moved => self.begin_move_refresh(),
                RailView::Growth => self.begin_growth_refresh(),
                RailView::Developer => self.begin_developer_refresh(false),
                RailView::Apfs => self.begin_apfs_refresh(),
                _ => {}
            }
        }
    }

    fn draw_reclaim_history(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let meta = format!("{} receipts · local only", self.reclaim_history.items.len());
        let content = panel_chrome(ui, rect, "Reclaim History", Some((meta, palette.faint)));
        let semantic_title = Rect::from_min_size(rect.min + vec2(10.0, 2.0), vec2(160.0, 28.0));
        ui.interact(
            semantic_title,
            ui.id().with("reclaim-history-heading"),
            Sense::hover(),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Reclaim History")
        });
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("← Insights")
                                .font(theme::display_md(10.5))
                                .color(palette.accent),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.rail_view = RailView::Insights;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            self.reclaim_history_rx.is_none(),
                            egui::Button::new(
                                RichText::new(if self.reclaim_history_rx.is_some() {
                                    "Refreshing…"
                                } else {
                                    "Refresh"
                                })
                                .font(theme::mono(9.5)),
                            ),
                        )
                        .clicked()
                    {
                        self.begin_reclaim_history_refresh();
                    }
                });
            });
        });

        let body = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 4.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let mut restore_index = None;
        let mut reveal_path = None;
        let mut show_trash = false;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(body), |ui| {
            if let Some(error) = &self.reclaim_history_error {
                ui.label(
                    RichText::new(format!("Reclaim History unavailable\n{error}"))
                        .font(theme::body(10.5))
                        .color(palette.danger),
                );
                return;
            }
            if self.reclaim_history_rx.is_some() && self.reclaim_history.items.is_empty() {
                ui.label(
                    RichText::new("Checking local cleanup receipts…")
                        .font(theme::body(10.5))
                        .color(palette.muted),
                );
                return;
            }
            if self.reclaim_history.items.is_empty() {
                ui.label(
                    RichText::new(
                        "No cleanup receipts yet. Successful reclaims appear here; exact Trash moves can be restored while they remain unchanged.",
                    )
                    .font(theme::body(10.5))
                    .color(palette.muted),
                );
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let card_width = ((ui.available_width() - 12.0) / 3.0).max(72.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (label, value, color) in [
                            (
                                "FREED",
                                fmt_bytes(self.reclaim_history.freed_bytes),
                                palette.safe,
                            ),
                            (
                                "IN TRASH",
                                fmt_bytes(self.reclaim_history.pending_bytes),
                                palette.caution,
                            ),
                            (
                                "RECOVERABLE",
                                self.reclaim_history.recoverable_count.to_string(),
                                palette.accent,
                            ),
                        ] {
                            Frame::none()
                                .fill(palette.surface)
                                .stroke(Stroke::new(1.0, palette.edge_soft))
                                .rounding(Rounding::same(8.0))
                                .inner_margin(Margin::symmetric(7.0, 6.0))
                                .show(ui, |ui| {
                                    ui.set_min_width(card_width - 16.0);
                                    ui.label(
                                        RichText::new(label)
                                            .font(theme::mono(7.5))
                                            .color(palette.faint),
                                    );
                                    ui.label(
                                        RichText::new(value)
                                            .font(theme::display_md(10.5))
                                            .color(color),
                                    );
                                });
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Receipt totals are local records, not a new disk scan.")
                            .font(theme::body(8.5))
                            .color(palette.faint),
                    );
                    ui.add_space(6.0);

                    for (index, item) in self.reclaim_history.items.iter().enumerate() {
                        let state_color = match item.state {
                            ReceiptState::Ready | ReceiptState::Restored => palette.safe,
                            ReceiptState::Permanent => palette.danger,
                            ReceiptState::Missing
                            | ReceiptState::OriginOccupied
                            | ReceiptState::Changed
                            | ReceiptState::ManualOnly
                            | ReceiptState::UnsafeOrigin
                            | ReceiptState::SymlinkAncestor
                            | ReceiptState::CrossDevice
                            | ReceiptState::Unavailable => palette.caution,
                        };
                        Frame::none()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(9.0))
                            .inner_margin(Margin::symmetric(9.0, 8.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let columns = reclaim_history_row_columns(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    ui.allocate_ui_with_layout(
                                        vec2(columns.text_width, 76.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_width(columns.text_width);
                                            ui.label(
                                                RichText::new(tail_str(&item.receipt.title, 30))
                                                    .font(theme::display_md(11.0))
                                                    .color(palette.ink),
                                            );
                                            ui.label(
                                                RichText::new(tail_str(
                                                    &item.receipt.origin.display().to_string(),
                                                    38,
                                                ))
                                                .font(theme::mono(8.5))
                                                .color(palette.faint),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} · {} · {}",
                                                    receipt_action_copy(item.receipt.action),
                                                    receipt_age_copy(
                                                        item.receipt.completed_at_ms
                                                    ),
                                                    fmt_bytes(
                                                        item.receipt.freed_bytes.max(
                                                            item.receipt.pending_bytes
                                                        )
                                                    )
                                                ))
                                                .font(theme::body(8.5))
                                                .color(palette.muted),
                                            );
                                            ui.label(
                                                RichText::new(receipt_state_copy(&item.state))
                                                    .font(theme::body(9.0))
                                                    .color(state_color),
                                            );
                                        },
                                    );
                                    ui.add_space(columns.gutter);
                                    ui.allocate_ui_with_layout(
                                        vec2(columns.utility_width, 76.0),
                                        egui::Layout::top_down(egui::Align::Max),
                                        |ui| {
                                            ui.set_width(columns.utility_width);
                                            if item.state == ReceiptState::Ready
                                                && ui
                                                    .add_enabled(
                                                        !self.mutation_busy(),
                                                        egui::Button::new(
                                                            RichText::new("Restore…")
                                                                .font(theme::display_md(9.0)),
                                                        ),
                                                    )
                                                    .clicked()
                                            {
                                                restore_index = Some(index);
                                            }
                                            let revealable = matches!(
                                                item.state,
                                                ReceiptState::Ready
                                                    | ReceiptState::OriginOccupied
                                                    | ReceiptState::Changed
                                                    | ReceiptState::SymlinkAncestor
                                                    | ReceiptState::CrossDevice
                                            );
                                            if revealable {
                                                if ui
                                                    .button(
                                                        RichText::new("Reveal in Trash")
                                                            .font(theme::body(8.5)),
                                                    )
                                                    .clicked()
                                                {
                                                    reveal_path = item
                                                        .receipt
                                                        .trash
                                                        .as_ref()
                                                        .map(|evidence| evidence.path.clone());
                                                }
                                            } else if matches!(
                                                item.state,
                                                ReceiptState::ManualOnly | ReceiptState::Missing
                                            ) && ui
                                                .button(
                                                    RichText::new("Open Trash")
                                                        .font(theme::body(8.5)),
                                                )
                                                .clicked()
                                            {
                                                show_trash = true;
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(6.0);
                    }
                });
        });

        if let Some(index) = restore_index {
            self.trash_restore_dialog = Some(TrashRestoreDialog {
                item: self.reclaim_history.items[index].clone(),
                acknowledged: false,
            });
            self.trash_restore_hold = 0.0;
        }
        if let Some(path) = reveal_path {
            reveal_in_finder(&path);
        }
        if show_trash {
            open_trash();
        }
    }

    fn draw_moved_items(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let active = self
            .moved_items
            .iter()
            .filter(|item| item.state != MoveState::Restored)
            .count();
        let meta = format!("{active} away · {} recorded", self.moved_items.len());
        let content = panel_chrome(ui, rect, "Moved items", Some((meta, palette.faint)));
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("← Insights")
                                .font(theme::display_md(10.5))
                                .color(palette.accent),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.rail_view = RailView::Insights;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            self.moves_rx.is_none(),
                            egui::Button::new(
                                RichText::new(if self.moves_rx.is_some() {
                                    "Refreshing…"
                                } else {
                                    "Refresh"
                                })
                                .font(theme::mono(9.5)),
                            ),
                        )
                        .clicked()
                    {
                        self.begin_move_refresh();
                    }
                });
            });
        });

        let list = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 4.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let mut restore_index = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(list), |ui| {
            if let Some(error) = &self.moves_error {
                ui.label(
                    RichText::new(format!("Moved items unavailable\n{error}"))
                        .font(theme::body(10.5))
                        .color(palette.danger),
                );
                return;
            }
            if self.moves_rx.is_some() && self.moved_items.is_empty() {
                ui.label(
                    RichText::new("Checking local move records…")
                        .font(theme::body(10.5))
                        .color(palette.muted),
                );
                return;
            }
            if self.moved_items.is_empty() {
                ui.label(
                    RichText::new(
                        "Nothing has been moved yet. Use Move to SSD from a target or the map; verified moves appear here.",
                    )
                    .font(theme::body(10.5))
                    .color(palette.muted),
                );
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (index, item) in self.moved_items.iter().enumerate() {
                        let state_color = match item.state {
                            MoveState::Ready => palette.safe,
                            MoveState::DriveDisconnected | MoveState::TargetMissing => {
                                palette.caution
                            }
                            MoveState::OriginChanged => palette.danger,
                            MoveState::Restored => palette.faint,
                        };
                        Frame::none()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(9.0))
                            .inner_margin(Margin::symmetric(10.0, 9.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let name = item
                                    .record
                                    .origin
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "Moved item".into());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(tail_str(&name, 28))
                                            .font(theme::display_md(11.5))
                                            .color(palette.ink),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(fmt_bytes(item.record.bytes))
                                                    .font(theme::mono(10.0))
                                                    .color(palette.muted),
                                            );
                                        },
                                    );
                                });
                                ui.label(
                                    RichText::new(tail_str(
                                        &item.record.dest.display().to_string(),
                                        42,
                                    ))
                                    .font(theme::mono(9.0))
                                    .color(palette.faint),
                                );
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(state_reason(item.state))
                                            .font(theme::body(9.5))
                                            .color(state_color),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let enabled = item.state == MoveState::Ready
                                                && !self.mutation_busy();
                                            if ui
                                                .add_enabled(
                                                    enabled,
                                                    egui::Button::new(
                                                        RichText::new("Restore to Mac…")
                                                            .font(theme::display_md(9.5)),
                                                    ),
                                                )
                                                .on_disabled_hover_text(state_reason(item.state))
                                                .clicked()
                                            {
                                                restore_index = Some(index);
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(6.0);
                    }
                });
        });
        if let Some(index) = restore_index {
            let item = self.moved_items[index].clone();
            let roots = RestoreRoots::production(Self::home_dir());
            let block = restore_block(&item.record, &roots);
            self.restore_dialog = Some(RestoreDialog {
                item,
                acknowledged: false,
                block,
            });
            self.restore_hold = 0.0;
        }
    }

    fn current_forecast(&self) -> ForecastState {
        forecast::analyze(
            &self.growth_watch.capacity,
            self.monitor_settings.threshold_gb as i64 * 1_000_000_000,
        )
    }

    fn draw_growth_watch(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let forecast = self.current_forecast();
        let meta = format!(
            "{} snapshots · local only",
            self.growth_watch.timeline.len()
        );
        let content = panel_chrome(ui, rect, "Growth Watch", Some((meta, palette.faint)));
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("← Insights")
                                .font(theme::display_md(10.5))
                                .color(palette.accent),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.rail_view = RailView::Insights;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            self.growth_watch_rx.is_none(),
                            egui::Button::new(
                                RichText::new(if self.growth_watch_rx.is_some() {
                                    "Refreshing…"
                                } else {
                                    "Refresh"
                                })
                                .font(theme::mono(9.5)),
                            ),
                        )
                        .clicked()
                    {
                        self.begin_growth_refresh();
                    }
                });
            });
        });

        let list = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 4.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let mut watch_change: Option<(std::path::PathBuf, bool)> = None;
        let mut start_scan = false;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(list), |ui| {
            if let Some(error) = &self.growth_watch_error {
                ui.label(
                    RichText::new(format!("Growth Watch unavailable\n{error}"))
                        .font(theme::body(10.5))
                        .color(palette.danger),
                );
                return;
            }
            if self.growth_watch_rx.is_some() && self.growth_watch.timeline.is_empty() {
                ui.label(
                    RichText::new("Reading local scan history…")
                        .font(theme::body(10.5))
                        .color(palette.muted),
                );
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("STORAGE FORECAST")
                            .font(theme::display_md(9.5))
                            .color(palette.accent),
                    );
                    Frame::none()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, palette.edge_soft))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(10.0, 9.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(forecast_headline(&forecast))
                                    .font(theme::body(10.5))
                                    .color(palette.ink),
                            );
                            if let ForecastState::Estimate(estimate) = &forecast {
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {} compatible scans · {} days · median loss {}/day",
                                        confidence_copy(estimate.confidence),
                                        estimate.observations,
                                        estimate.span_days,
                                        fmt_bytes(estimate.bytes_per_day)
                                    ))
                                    .font(theme::mono(8.5))
                                    .color(palette.muted),
                                );
                            }
                            ui.label(
                                RichText::new(
                                    "Guidance from completed local scans — not reclaimable space or a guarantee.",
                                )
                                .font(theme::body(9.0))
                                .color(palette.faint),
                            );
                            if matches!(forecast, ForecastState::NeedHistory { .. }) {
                                ui.add_space(4.0);
                                if ui
                                    .add_enabled(
                                        !self.scanning(),
                                        egui::Button::new(
                                            RichText::new(if self.scanning() {
                                                "Scanning…"
                                            } else {
                                                "Scan now"
                                            })
                                            .font(theme::display_md(9.5)),
                                        ),
                                    )
                                    .clicked()
                                {
                                    start_scan = true;
                                }
                            }
                        });
                    ui.add_space(10.0);

                    if self.growth_watch.timeline.is_empty() {
                        ui.label(
                            RichText::new("Complete a scan to create the first local baseline.")
                                .font(theme::body(10.5))
                                .color(palette.muted),
                        );
                    } else {
                        ui.label(
                            RichText::new("TOTAL STORAGE TREND")
                                .font(theme::display_md(9.5))
                                .color(palette.muted),
                        );
                        let (chart, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 76.0),
                            Sense::hover(),
                        );
                        draw_growth_sparkline(ui, chart, &self.growth_watch.timeline);
                        ui.add_space(8.0);
                    }

                    if self.growth_watch.timeline.len() == 1 {
                        ui.label(
                            RichText::new(
                                "Baseline saved. Complete another normal scan to compare growth.",
                            )
                            .font(theme::body(9.5))
                            .color(palette.faint),
                        );
                    }

                    if !self.growth_watch.watched.is_empty() {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("WATCHED FOLDERS")
                                .font(theme::display_md(9.5))
                                .color(palette.accent),
                        );
                        for series in &self.growth_watch.watched {
                            Frame::none()
                                .fill(palette.surface)
                                .stroke(Stroke::new(1.0, palette.edge_soft))
                                .rounding(Rounding::same(8.0))
                                .inner_margin(Margin::symmetric(9.0, 7.0))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(tail_str(
                                                    &series.path.display().to_string(),
                                                    30,
                                                ))
                                                .font(theme::display_md(10.5))
                                                .color(palette.ink),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} · {}",
                                                    fmt_delta(series.bytes_delta),
                                                    fmt_percent_tenths(series.percent_tenths)
                                                ))
                                                .font(theme::mono(9.0))
                                                .color(if series.bytes_delta > 0 {
                                                    palette.caution
                                                } else {
                                                    palette.safe
                                                }),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add_enabled(
                                                        self.growth_watch_rx.is_none(),
                                                        egui::Button::new(
                                                            RichText::new("Unwatch")
                                                                .font(theme::mono(9.0)),
                                                        ),
                                                    )
                                                    .clicked()
                                                {
                                                    watch_change =
                                                        Some((series.path.clone(), false));
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(5.0);
                        }
                    }

                    if !self.growth_watch.recurring.is_empty() {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("RECURRING GROWERS")
                                .font(theme::display_md(9.5))
                                .color(palette.caution),
                        );
                        for growth in &self.growth_watch.recurring {
                            Frame::none()
                                .fill(palette.surface)
                                .stroke(Stroke::new(1.0, palette.edge_soft))
                                .rounding(Rounding::same(8.0))
                                .inner_margin(Margin::symmetric(9.0, 7.0))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(tail_str(
                                                    &growth.path.display().to_string(),
                                                    28,
                                                ))
                                                .font(theme::display_md(10.5))
                                                .color(palette.ink),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "net {} across {} growth interval(s) · {}",
                                                    fmt_delta(growth.bytes_delta),
                                                    growth.positive_intervals,
                                                    fmt_percent_tenths(growth.percent_tenths)
                                                ))
                                                .font(theme::mono(8.5))
                                                .color(palette.muted),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                let label = if growth.watched {
                                                    "Watching"
                                                } else {
                                                    "Watch"
                                                };
                                                if ui
                                                    .add_enabled(
                                                        !growth.watched
                                                            && self.growth_watch_rx.is_none(),
                                                        egui::Button::new(
                                                            RichText::new(label)
                                                                .font(theme::mono(9.0)),
                                                        ),
                                                    )
                                                    .clicked()
                                                {
                                                    watch_change =
                                                        Some((growth.path.clone(), true));
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(5.0);
                        }
                    }
                });
        });
        if let Some((path, watched)) = watch_change {
            self.set_growth_folder(path, watched);
        }
        if start_scan {
            self.begin_scan();
        }
    }

    fn draw_developer_lens(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let report = self.developer_report.clone();
        let state = developer_workspace_state(
            report.as_ref(),
            self.developer_rx.is_some(),
            self.developer_error.as_deref(),
            self.developer_scan_ready(),
        );
        let meta = report
            .as_ref()
            .map(|report| {
                format!(
                    "{} sections · {} measured",
                    report.sections.len(),
                    fmt_bytes(report.measured_bytes)
                )
            })
            .unwrap_or_else(|| "opt-in · read-only".into());
        let content = panel_chrome(ui, rect, "Developer Deep Dive", Some((meta, palette.faint)));
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        let mut refresh = false;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("← Insights")
                                .font(theme::display_md(10.5))
                                .color(palette.accent),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.rail_view = RailView::Insights;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            self.developer_scan_ready() && self.developer_rx.is_none(),
                            egui::Button::new(RichText::new("Refresh").font(theme::mono(9.0))),
                        )
                        .clicked()
                    {
                        refresh = true;
                    }
                });
            });
        });
        let list = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 4.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let mut reveal = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("DEVELOPER WORKSPACE")
                            .font(theme::display_md(9.5))
                            .color(palette.accent),
                    );
                    ui.label(
                        RichText::new(developer_workspace_copy(state))
                            .font(theme::body(9.5))
                            .color(if state == DeveloperWorkspaceState::Failed {
                                palette.danger
                            } else {
                                palette.muted
                            }),
                    );
                    ui.label(
                        RichText::new(
                            "Read-only evidence · discovered paths never become cleanup rules · totals exclude overlaps and inside-Docker detail.",
                        )
                        .font(theme::body(8.5))
                        .color(palette.faint),
                    );
                    ui.add_space(6.0);

                    if let Some(error) = &self.developer_error {
                        ui.label(
                            RichText::new(error)
                                .font(theme::body(9.0))
                                .color(palette.danger),
                        );
                    }

                    let Some(report) = &report else {
                        return;
                    };
                    let docker_section = report
                        .sections
                        .iter()
                        .find(|section| section.section == developer::DeveloperSection::Docker);
                    Frame::none()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, palette.edge_soft))
                        .rounding(Rounding::same(9.0))
                        .inner_margin(Margin::symmetric(10.0, 9.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Docker")
                                        .font(theme::display_md(11.0))
                                        .color(palette.ink),
                                );
                                if let Some(section) = docker_section {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} measured",
                                                    fmt_bytes(section.measured_bytes)
                                                ))
                                                .font(theme::mono(9.0))
                                                .color(palette.accent),
                                            );
                                        },
                                    );
                                }
                            });
                            ui.label(
                                RichText::new(docker_footprint_copy(docker_section.is_some()))
                                    .font(theme::body(8.5))
                                    .color(palette.muted),
                            );
                            if let Some(section) = docker_section {
                                for finding in &section.findings {
                                    ui.add_space(5.0);
                                    if let Some(path) = draw_developer_finding(ui, finding) {
                                        reveal = Some(path);
                                    }
                                }
                            }
                            for detail in &report.docker.details {
                                ui.add_space(5.0);
                                draw_docker_detail(ui, detail);
                            }
                            if let Some(unavailable) = &report.docker.unavailable {
                                ui.add_space(5.0);
                                ui.label(
                                    RichText::new(unavailable)
                                        .font(theme::body(8.5))
                                        .color(palette.caution),
                                );
                            }
                        });
                    ui.add_space(6.0);

                    for section in report
                        .sections
                        .iter()
                        .filter(|section| section.section != developer::DeveloperSection::Docker)
                    {
                        Frame::none()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(9.0))
                            .inner_margin(Margin::symmetric(10.0, 9.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(section.section.title())
                                            .font(theme::display_md(11.0))
                                            .color(palette.ink),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} measured",
                                                    fmt_bytes(section.measured_bytes)
                                                ))
                                                    .font(theme::mono(9.0))
                                                    .color(palette.accent),
                                            );
                                        },
                                    );
                                });
                                ui.label(
                                    RichText::new(section.section.explanation())
                                        .font(theme::body(8.5))
                                        .color(palette.muted),
                                );
                                for finding in &section.findings {
                                    ui.add_space(5.0);
                                    if let Some(path) = draw_developer_finding(ui, finding) {
                                        reveal = Some(path);
                                    }
                                }
                            });
                        ui.add_space(6.0);
                    }
                });
        });
        if let Some(path) = reveal {
            reveal_in_finder(&path);
        }
        if refresh {
            self.begin_developer_refresh(true);
        }
    }

    fn draw_apfs_accounting(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let meta = self
            .apfs
            .as_ref()
            .map(|value| format!("{} container", fmt_bytes(value.container_size)))
            .unwrap_or_else(|| "read-only".into());
        let content = panel_chrome(ui, rect, "APFS accounting", Some((meta, palette.faint)));
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("← Insights")
                                .font(theme::display_md(10.5))
                                .color(palette.accent),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.rail_view = RailView::Insights;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            self.apfs_rx.is_none(),
                            egui::Button::new(
                                RichText::new(if self.apfs_rx.is_some() {
                                    "Refreshing…"
                                } else {
                                    "Refresh"
                                })
                                .font(theme::mono(9.5)),
                            ),
                        )
                        .clicked()
                    {
                        self.begin_apfs_refresh();
                    }
                });
            });
        });
        let body = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 6.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(body), |ui| {
            if let Some(error) = &self.apfs_error {
                ui.label(
                    RichText::new(format!("APFS accounting unavailable\n{error}"))
                        .font(theme::body(10.5))
                        .color(palette.danger),
                );
                return;
            }
            let Some(value) = &self.apfs else {
                ui.label(
                    RichText::new("Reading fixed APFS container values from macOS…")
                        .font(theme::body(10.5))
                        .color(palette.muted),
                );
                return;
            };
            let used = value.container_size.saturating_sub(value.container_free);
            ui.label(
                RichText::new(
                    "APFS shares one container across system volumes. These values are accounting facts, not cleanup recommendations.",
                )
                .font(theme::body(10.0))
                .color(palette.muted),
            );
            ui.add_space(10.0);
            for (label, amount, color) in [
                ("Container capacity", fmt_bytes(value.container_size), palette.ink),
                ("Container used", fmt_bytes(used), palette.caution),
                ("Container free", fmt_bytes(value.container_free), palette.safe),
            ] {
                Frame::none()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.edge_soft))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(10.0, 9.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(label)
                                    .font(theme::body(10.5))
                                    .color(palette.muted),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(amount)
                                            .font(theme::display_md(11.5))
                                            .color(color),
                                    );
                                },
                            );
                        });
                    });
                ui.add_space(6.0);
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "LOCAL SNAPSHOTS  ·  {}",
                    value
                        .snapshot_count
                        .map(|count| count.to_string())
                        .unwrap_or_else(|| "unavailable".into())
                ))
                .font(theme::display_md(10.0))
                .color(palette.accent),
            );
            ui.label(
                RichText::new(if value.snapshot_bytes.is_some() {
                    "Snapshot byte size is measured separately."
                } else {
                    "Exact snapshot byte size is not reliably reported by macOS, so DiskDeck does not add it to reclaimable space."
                })
                .font(theme::body(9.5))
                .color(palette.muted),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("PURGEABLE CAPACITY  ·  NOT RELIABLY REPORTED")
                    .font(theme::display_md(10.0))
                    .color(palette.caution),
            );
            ui.label(
                RichText::new(if value.purgeable_bytes.is_some() {
                    "macOS supplied a purgeable estimate; it is still system-managed."
                } else {
                    "DiskDeck will not invent a purgeable number or claim that system-managed capacity can be freed immediately."
                })
                .font(theme::body(9.5))
                .color(palette.muted),
            );
        });
    }

    fn draw_app_leftovers(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let total: i64 = self.leftovers.iter().map(|finding| finding.bytes).sum();
        let meta = format!("{} findings · {}", self.leftovers.len(), fmt_bytes(total));
        let content = panel_chrome(ui, rect, "App leftovers", Some((meta, palette.faint)));
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("← Insights")
                            .font(theme::display_md(10.5))
                            .color(palette.accent),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.rail_view = RailView::Insights;
            }
        });
        let body = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 5.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let mut reveal = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(body), |ui| {
            if let Some(error) = &self.leftovers_error {
                ui.label(
                    RichText::new(format!("App leftovers unavailable\n{error}"))
                        .font(theme::body(10.5))
                        .color(palette.danger),
                );
                return;
            }
            if self.leftovers_rx.is_some() {
                ui.label(
                    RichText::new("Verifying large sandbox bundle identifiers locally…")
                        .font(theme::body(10.5))
                        .color(palette.muted),
                );
                return;
            }
            if self.leftovers.is_empty() {
                ui.label(
                    RichText::new(
                        "No large app sandbox passed the conservative absence proof. Uncertain matches are omitted.",
                    )
                    .font(theme::body(10.5))
                    .color(palette.muted),
                );
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "CAUTION · read-only findings · inspect in Finder before deciding anything.",
                        )
                        .font(theme::body(9.5))
                        .color(palette.caution),
                    );
                    ui.add_space(6.0);
                    for finding in &self.leftovers {
                        Frame::none()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(9.0))
                            .inner_margin(Margin::symmetric(10.0, 9.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(tail_str(&finding.bundle_id, 32))
                                            .font(theme::display_md(10.5))
                                            .color(palette.ink),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(fmt_bytes(finding.bytes))
                                                    .font(theme::mono(10.0))
                                                    .color(palette.caution),
                                            );
                                        },
                                    );
                                });
                                ui.label(
                                    RichText::new(tail_str(
                                        &finding.path.display().to_string(),
                                        40,
                                    ))
                                    .font(theme::mono(8.5))
                                    .color(palette.faint),
                                );
                                ui.label(
                                    RichText::new(&finding.evidence)
                                        .font(theme::body(9.0))
                                        .color(palette.muted),
                                );
                                if ui
                                    .button(
                                        RichText::new("Reveal in Finder")
                                            .font(theme::mono(9.0)),
                                    )
                                    .clicked()
                                {
                                    reveal = Some(finding.path.clone());
                                }
                            });
                        ui.add_space(6.0);
                    }
                });
        });
        if let Some(path) = reveal {
            reveal_in_finder(&path);
        }
    }

    fn draw_menu_monitor(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let forecast = self.current_forecast();
        let status = if self.monitor_settings.enabled {
            "enabled"
        } else {
            "off"
        };
        let content = panel_chrome(
            ui,
            rect,
            "Menu-bar monitor",
            Some((status.into(), palette.faint)),
        );
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("← Insights")
                            .font(theme::display_md(10.5))
                            .color(palette.accent),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.rail_view = RailView::Insights;
            }
        });
        let body = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 7.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let mut next = self.monitor_settings;
        let mut changed = false;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(
                    "A native menu-bar free-space readout updated every five minutes. It uses statfs only and never starts a disk scan.",
                )
                .font(theme::body(10.0))
                .color(palette.muted),
            );
            ui.add_space(10.0);
            changed |= ui
                .checkbox(&mut next.enabled, "Show free space in the menu bar")
                .changed();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Low-space warning below")
                        .font(theme::body(10.0))
                        .color(palette.muted),
                );
                egui::ComboBox::from_id_salt("monitor-threshold")
                    .selected_text(format!("{} GB", next.threshold_gb))
                    .show_ui(ui, |ui| {
                        for value in [5, 10, 15, 20, 30, 50, 100] {
                            changed |= ui
                                .selectable_value(
                                    &mut next.threshold_gb,
                                    value,
                                    format!("{value} GB"),
                                )
                                .changed();
                        }
                    });
            });
            ui.label(
                RichText::new(format!(
                    "Current free space: {}{}",
                    fmt_bytes(self.stats.free),
                    if monitor::is_low(self.stats.free, next.threshold_gb) {
                        " · below threshold"
                    } else {
                        ""
                    }
                ))
                .font(theme::mono(9.5))
                .color(if monitor::is_low(self.stats.free, next.threshold_gb) {
                    palette.caution
                } else {
                    palette.safe
                }),
            );
            ui.add_space(14.0);
            ui.label(
                RichText::new("FOREGROUND FORECAST")
                    .font(theme::display_md(9.5))
                    .color(palette.accent),
            );
            Frame::none()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.edge_soft))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(10.0, 9.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(forecast_headline(&forecast))
                            .font(theme::body(10.5))
                            .color(palette.ink),
                    );
                    ui.label(
                        RichText::new("Updated only after a completed foreground scan.")
                            .font(theme::body(9.0))
                            .color(palette.faint),
                    );
                });
            ui.add_space(14.0);
            changed |= ui
                .checkbox(&mut next.launch_at_login, "Launch DiskDeck at login")
                .changed();
            ui.label(
                RichText::new(
                    "Separate opt-in. This installs a user-owned LaunchAgent for /Applications/DiskDeck.app; no privileged daemon.",
                )
                .font(theme::body(9.0))
                .color(palette.faint),
            );
            if let Some(error) = &self.monitor_error {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(error)
                        .font(theme::body(9.5))
                    .color(palette.danger),
                );
            }
                });
        });
        if changed {
            self.apply_monitor_settings(next);
        }
    }

    fn draw_file_review(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let meta = self
            .file_review
            .as_ref()
            .map(|result| format!("{} files reviewed", fmt_count(result.files_visited as i64)))
            .unwrap_or_else(|| {
                if self.file_review_rx.is_some() {
                    "reviewing…".into()
                } else {
                    "opt-in · off".into()
                }
            });
        let content = panel_chrome(
            ui,
            rect,
            "Duplicate & large-old review",
            Some((meta, palette.faint)),
        );
        let nav = Rect::from_min_size(
            content.min + vec2(10.0, 4.0),
            vec2(content.width() - 20.0, 30.0),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("← Insights")
                            .font(theme::display_md(10.5))
                            .color(palette.accent),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.rail_view = RailView::Insights;
            }
        });
        let body = Rect::from_min_max(
            pos2(content.min.x + 10.0, nav.max.y + 6.0),
            pos2(content.max.x - 10.0, content.max.y - 4.0),
        );
        let mut start = false;
        let mut cancel = false;
        let mut reveal = None;
        let mut quick_look = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(body), |ui| {
            ui.label(
                RichText::new(
                    "Read-only scan of standard user folders. It skips Library, hidden folders, symlinks, node_modules, and app/media-library bundles.",
                )
                .font(theme::body(9.5))
                .color(palette.muted),
            );
            ui.add_space(8.0);
            if self.file_review_rx.is_some() {
                ui.label(
                    RichText::new("Comparing candidate files byte-for-byte…")
                        .font(theme::body(10.0))
                        .color(palette.accent),
                );
                cancel = ui.button("Cancel review").clicked();
                return;
            }
            if self.file_review.is_none() {
                ui.label(
                    RichText::new(
                        "Nothing starts automatically. Results have Quick Look and Reveal only—no delete or move action.",
                    )
                    .font(theme::body(9.5))
                    .color(palette.faint),
                );
                ui.add_space(8.0);
                start = ui
                    .add(egui::Button::new(
                        RichText::new("Start review scan")
                            .font(theme::display_md(10.5))
                            .color(palette.accent),
                    ))
                    .clicked();
                if let Some(error) = &self.file_review_error {
                    ui.label(RichText::new(error).font(theme::body(9.5)).color(palette.danger));
                }
                return;
            }
            let result = self.file_review.as_ref().unwrap();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "EXACT DUPLICATES · {} GROUPS",
                                result.duplicate_groups.len()
                            ))
                            .font(theme::display_md(9.5))
                            .color(palette.caution),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            start = ui.small_button("Run again").clicked();
                        });
                    });
                    if result.duplicate_groups.is_empty() {
                        ui.label(RichText::new("No exact duplicate group met the 10 MB floor.").font(theme::body(9.0)).color(palette.faint));
                    }
                    for group in &result.duplicate_groups {
                        Frame::none()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(8.0))
                            .inner_margin(Margin::symmetric(9.0, 8.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    RichText::new(format!(
                                        "{} exact copies · {} each · {} duplicated",
                                        group.paths.len(),
                                        fmt_bytes(group.bytes_each),
                                        fmt_bytes(group.wasted_bytes)
                                    ))
                                    .font(theme::body(9.5))
                                    .color(palette.ink),
                                );
                                for path in group.paths.iter().take(5) {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(tail_str(&path.display().to_string(), 33))
                                                .font(theme::mono(8.5))
                                                .color(palette.muted),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("Reveal").clicked() {
                                                reveal = Some(path.clone());
                                            }
                                            if ui.small_button("Quick Look").clicked() {
                                                quick_look = Some(path.clone());
                                            }
                                        });
                                    });
                                }
                                ui.label(
                                    RichText::new("All copies are preserved; DiskDeck does not choose one for deletion.")
                                        .font(theme::body(8.5))
                                        .color(palette.safe),
                                );
                            });
                        ui.add_space(6.0);
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("LARGE & OLD · {} FILES", result.large_old.len()))
                            .font(theme::display_md(9.5))
                            .color(palette.caution),
                    );
                    ui.label(
                        RichText::new("Uses macOS last-access metadata, which may be coarse or disabled on some volumes.")
                            .font(theme::body(8.5))
                            .color(palette.faint),
                    );
                    let now = unsafe { libc::time(std::ptr::null_mut()) };
                    for file in &result.large_old {
                        let days = now.saturating_sub(file.accessed_at) / (24 * 60 * 60);
                        Frame::none()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.edge_soft))
                            .rounding(Rounding::same(8.0))
                            .inner_margin(Margin::symmetric(9.0, 7.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    RichText::new(tail_str(&file.path.display().to_string(), 38))
                                        .font(theme::mono(9.0))
                                        .color(palette.ink),
                                );
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{} · accessed ~{days} days ago", fmt_bytes(file.bytes)))
                                            .font(theme::body(8.5))
                                            .color(palette.muted),
                                    );
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button("Reveal").clicked() { reveal = Some(file.path.clone()); }
                                        if ui.small_button("Quick Look").clicked() { quick_look = Some(file.path.clone()); }
                                    });
                                });
                            });
                        ui.add_space(5.0);
                    }
                });
        });
        if start {
            self.begin_file_review();
        }
        if cancel {
            if let Some(flag) = &self.file_review_cancel {
                flag.store(true, Relaxed);
            }
        }
        if let Some(path) = reveal {
            reveal_in_finder(&path);
        }
        if let Some(path) = quick_look {
            if let Err(error) = std::process::Command::new("/usr/bin/qlmanage")
                .arg("-p")
                .arg(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                self.ops(OpsKind::Amber, format!("Quick Look unavailable — {error}"));
            }
        }
    }

    /// One recommendation card. Returns Some(path) if "reveal" was clicked.
    fn rec_card(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        offload_out: &mut Option<(std::path::PathBuf, i64)>,
    ) -> Option<std::path::PathBuf> {
        let palette = theme::palette(ui.ctx());
        let mut reveal = None;
        let cleaning = self.cleaning;
        let rec_real = strip_data_root(&self.recs[idx].rec.path);
        let rec_size = self.recs[idx].rec.bytes;
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let offload_block = classify_movable(&rec_real, &home).err();
        let row = &mut self.recs[idx];
        let (border, fill) = match (&row.status, row.checked) {
            (RecStatus::Running, _) => (palette.accent_dim(110), palette.surface_raised),
            (RecStatus::Failed(_), _) => (palette.danger_dim(120), palette.surface_raised),
            (_, true) => (palette.edge, palette.surface),
            _ => (palette.edge_soft, palette.surface),
        };
        let dimmed = matches!(row.status, RecStatus::Cleared(_) | RecStatus::InTrash(_));

        Frame::none()
            .fill(fill)
            .stroke(Stroke::new(1.0, border))
            .rounding(Rounding::same(10.0))
            .inner_margin(Margin::symmetric(11.0, 10.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if dimmed {
                    ui.disable();
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.checkbox(&mut row.checked, "");
                    ui.add_space(8.0);
                    let columns = review_row_columns(ui.available_width());
                    ui.allocate_ui_with_layout(
                        vec2(columns.text_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(columns.text_width);
                            let title_resp = ui.add(
                                Label::new(
                                    RichText::new(&row.rec.title)
                                        .font(theme::display_md(13.0))
                                        .color(palette.ink),
                                )
                                .sense(Sense::click())
                                .truncate(),
                            );
                            ui.add(
                                Label::new(
                                    RichText::new(&row.rec.display)
                                        .font(theme::mono(9.5))
                                        .color(palette.faint),
                                )
                                .truncate(),
                            );
                            if title_resp.clicked() {
                                row.expanded = !row.expanded;
                            }
                            if row.expanded {
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(row.rec.desc)
                                        .font(theme::body(11.0))
                                        .color(palette.muted),
                                );
                                ui.label(
                                    RichText::new("To restore")
                                        .font(theme::body(9.5))
                                        .color(palette.faint),
                                );
                                ui.label(
                                    RichText::new(row.rec.restore)
                                        .font(theme::body(11.0))
                                        .color(palette.muted),
                                );
                                if !row.rec.note.is_empty() {
                                    ui.label(
                                        RichText::new(&row.rec.note)
                                            .font(theme::body(11.0))
                                            .color(palette.caution),
                                    );
                                }
                                if let Some(cmd) = row.rec.command {
                                    ui.label(
                                        RichText::new("Runs")
                                            .font(theme::body(9.5))
                                            .color(palette.faint),
                                    );
                                    Frame::none()
                                        .fill(palette.surface)
                                        .stroke(Stroke::new(1.0, palette.edge_soft))
                                        .rounding(Rounding::same(7.0))
                                        .inner_margin(Margin::symmetric(7.0, 4.0))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(cmd)
                                                    .font(theme::mono(10.0))
                                                    .color(palette.accent),
                                            );
                                        });
                                }
                                ui.add_space(2.0);
                                if ui
                                    .add(
                                        Label::new(
                                            RichText::new("reveal in Finder ↗")
                                                .font(theme::mono(10.0))
                                                .color(palette.accent),
                                        )
                                        .sense(Sense::click()),
                                    )
                                    .clicked()
                                {
                                    reveal = Some(row.rec.path.clone());
                                }
                                ui.add_space(2.0);
                                let mut offload_response = ui.add_enabled(
                                    offload_block.is_none(),
                                    Label::new(
                                        RichText::new("→ SSD")
                                            .font(theme::mono(10.0))
                                            .color(palette.muted),
                                    )
                                    .sense(Sense::click()),
                                );
                                if let Some(block) = offload_block {
                                    offload_response =
                                        offload_response.on_disabled_hover_text(block.message());
                                } else {
                                    offload_response = offload_response
                                        .on_hover_text("move this to an attached external drive");
                                }
                                if offload_response.clicked() {
                                    *offload_out = Some((rec_real.clone(), rec_size));
                                }
                            }
                        },
                    );
                    ui.add_space(columns.gutter);
                    ui.allocate_ui_with_layout(
                        vec2(columns.utility_width, 0.0),
                        egui::Layout::top_down(egui::Align::Max),
                        |ui| {
                            ui.set_width(columns.utility_width);
                            let size_txt = if row.rec.estimate {
                                format!("≈{}", fmt_bytes(row.rec.bytes))
                            } else {
                                fmt_bytes(row.rec.bytes)
                            };
                            ui.label(
                                RichText::new(size_txt)
                                    .font(theme::mono(12.5))
                                    .color(palette.ink)
                                    .strong(),
                            );
                            // action chip
                            let (chip_txt, chip_color, cyclable) =
                                match (row.rec.action, row.action) {
                                    (Action::Command, _) => ("Script", palette.accent, false),
                                    (Action::Empty, _) => ("Empty", palette.accent, false),
                                    (_, Action::Trash) => ("Trash", palette.safe, true),
                                    (_, Action::Delete) => ("Erase", palette.danger, true),
                                    _ => ("?", palette.muted, false),
                                };
                            let (chip, resp) =
                                ui.allocate_exact_size(vec2(64.0, 18.0), Sense::click());
                            let p = ui.painter();
                            p.rect_stroke(
                                chip,
                                Rounding::same(6.0),
                                Stroke::new(1.0, chip_color.gamma_multiply(0.55)),
                            );
                            p.text(
                                chip.center(),
                                Align2::CENTER_CENTER,
                                chip_txt,
                                theme::body(9.5),
                                chip_color,
                            );
                            if cyclable && resp.clicked() && !cleaning {
                                let allowed: Vec<Action> = [
                                    (row.rec.allow_trash, Action::Trash),
                                    (row.rec.allow_delete, Action::Delete),
                                ]
                                .iter()
                                .filter(|(ok, _)| *ok)
                                .map(|(_, a)| *a)
                                .collect();
                                if allowed.len() > 1 {
                                    let pos =
                                        allowed.iter().position(|a| *a == row.action).unwrap_or(0);
                                    row.action = allowed[(pos + 1) % allowed.len()];
                                }
                            }
                            if cyclable {
                                resp.on_hover_text(
                                    "click to switch between Trash and permanent erase",
                                );
                            }
                            // status
                            let status = match &row.status {
                                RecStatus::Idle => None,
                                RecStatus::Running => Some(("Running".to_string(), palette.accent)),
                                RecStatus::Cleared(_) => {
                                    Some(("Cleared".to_string(), palette.safe))
                                }
                                RecStatus::InTrash(_) => {
                                    Some(("In Trash".to_string(), palette.safe))
                                }
                                RecStatus::Failed(_) => {
                                    Some(("Failed".to_string(), palette.danger))
                                }
                            };
                            if let Some((txt, color)) = status {
                                let resp = ui
                                    .label(RichText::new(txt).font(theme::mono(9.5)).color(color));
                                if let RecStatus::Failed(msg) = &row.status {
                                    resp.on_hover_text(msg);
                                }
                            }
                        },
                    );
                });
            });
        reveal
    }

    fn reclaim_footer(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let p = ui.painter();
        p.line_segment(
            [
                pos2(rect.min.x + 1.0, rect.min.y),
                pos2(rect.max.x - 1.0, rect.min.y),
            ],
            Stroke::new(1.0, palette.edge_soft),
        );
        let armed: Vec<&RecRow> = self.recs.iter().filter(|r| r.checked).collect();
        let bytes: i64 = armed.iter().map(|r| r.rec.bytes).sum();
        let count = armed.len();
        let needs_review = armed
            .iter()
            .any(|row| row.rec.tier == Tier::Caution || row.action == Action::Delete);
        let action_color = if self.cleaning {
            palette.accent
        } else if needs_review {
            palette.caution
        } else {
            palette.safe
        };

        p.text(
            rect.min + vec2(16.0, 12.0),
            Align2::LEFT_TOP,
            if count > 0 {
                format!("{count} selected")
            } else {
                "Nothing selected".to_string()
            },
            theme::body(10.0),
            palette.muted,
        );
        if count > 0 {
            p.text(
                rect.min + vec2(16.0, 28.0),
                Align2::LEFT_TOP,
                format!("≈ {}", fmt_bytes(bytes)),
                theme::mono(16.0),
                action_color,
            );
        }

        // hold button
        let btn = Rect::from_min_size(
            pos2(rect.max.x - 226.0, rect.min.y + 11.0),
            vec2(210.0, 42.0),
        );
        let enabled = count > 0 && !self.mutation_busy();
        let resp = ui.interact(btn, ui.id().with("reclaim"), Sense::click_and_drag());
        let alpha = if enabled { 1.0 } else { 0.35 };
        let border = action_color;
        let fill_alpha = if resp.hovered() && enabled { 42 } else { 24 };
        let fill = Color32::from_rgba_unmultiplied(
            action_color.r(),
            action_color.g(),
            action_color.b(),
            fill_alpha,
        );
        p.rect_filled(btn, Rounding::same(9.0), fill);
        p.rect_stroke(
            btn,
            Rounding::same(9.0),
            Stroke::new(1.0, border.gamma_multiply(0.6 * alpha)),
        );

        // hold ring
        let ring_c = pos2(btn.min.x + 24.0, btn.center().y);
        p.circle_stroke(ring_c, 10.0, Stroke::new(3.0, palette.edge));
        if self.hold > 0.0 {
            let pts: Vec<Pos2> = (0..=32)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * self.hold * i as f32 / 32.0;
                    ring_c + vec2(a.cos(), a.sin()) * 10.0
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(3.0, action_color)));
        }
        let label = if self.cleaning {
            "Reclaiming…"
        } else {
            "Hold to reclaim"
        };
        p.text(
            pos2(btn.min.x + 44.0, btn.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::body(12.0),
            border.gamma_multiply(alpha),
        );

        if enabled && resp.is_pointer_button_down_on() {
            self.hold += ui.input(|i| i.stable_dt).min(0.1) / HOLD_SECS;
            ui.ctx().request_repaint();
            if self.hold >= 1.0 {
                self.hold = 0.0;
                self.fire_reclaim();
            }
        } else {
            self.hold = 0.0;
        }
    }

    fn offload_dialog(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        let Some(mut dlg) = self.dialog.take() else {
            return;
        };
        let mut keep_open = true;
        let mut launch = false;

        egui::Window::new(RichText::new("Move to external drive").font(theme::body(13.0)))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(dlg.src.display().to_string()).font(theme::mono(10.5)).color(palette.muted));
                    if ui
                        .add(Label::new(RichText::new("⧉ copy").font(theme::mono(9.5)).color(palette.accent)).sense(Sense::click()))
                        .on_hover_text("copy path to clipboard")
                        .clicked()
                    {
                        let p = dlg.src.display().to_string();
                        ui.ctx().output_mut(|o| o.copied_text = p);
                    }
                });
                ui.label(RichText::new(fmt_bytes(dlg.size)).font(theme::mono(10.5)).color(palette.faint));
                ui.add_space(8.0);

                if let Some(reason) = dlg.reason.clone() {
                    ui.label(RichText::new(format!("✗ {reason}")).font(theme::mono(10.5)).color(palette.danger));
                    ui.add_space(8.0);
                    if ui.button(RichText::new("Close").font(theme::mono(10.5))).clicked() {
                        keep_open = false;
                    }
                    return;
                }

                // target volume
                if dlg.vols.len() > 1 {
                    let names: Vec<String> = dlg.vols.iter().map(|v| v.name.clone()).collect();
                    egui::ComboBox::from_label("target")
                        .selected_text(names[dlg.vol_idx].clone())
                        .show_ui(ui, |ui| {
                            for (i, n) in names.iter().enumerate() {
                                ui.selectable_value(&mut dlg.vol_idx, i, n);
                            }
                        });
                } else {
                    ui.label(RichText::new(format!("target: {}", dlg.vols[0].name)).font(theme::mono(10.5)).color(palette.muted));
                }
                let vol = &dlg.vols[dlg.vol_idx];
                let room = has_room(dlg.size, vol.free_bytes);
                let free_color = if room { palette.faint } else { palette.danger };
                ui.label(RichText::new(format!("{} free", fmt_bytes(vol.free_bytes))).font(theme::mono(9.5)).color(free_color));
                if vol.fs_type == "exfat" {
                    ui.label(RichText::new("note: exFAT can't keep macOS metadata (xattrs, resource forks, internal symlinks)").font(theme::mono(9.0)).color(palette.caution));
                }
                ui.add_space(8.0);

                // symlink vs clean
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut dlg.leave_symlink, true, "Leave a symlink");
                    ui.selectable_value(&mut dlg.leave_symlink, false, "Clean move");
                    if ui.small_button("ⓘ").clicked() {
                        dlg.show_info = !dlg.show_info;
                    }
                });
                if dlg.show_info {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Leave a symlink — apps and paths that point at the old location keep working; you free internal space with nothing to reconfigure. Trade-off: the link dangles while the SSD is unplugged (works again on reconnect).").font(theme::mono(9.5)).color(palette.muted));
                    ui.add_space(2.0);
                    ui.label(RichText::new("Clean move — nothing points back: no dangling-link risk, fully portable. Trade-off: anything referencing the old path breaks until you move it back.").font(theme::mono(9.5)).color(palette.muted));
                }
                ui.add_space(10.0);

                ui.checkbox(
                    &mut dlg.acknowledged,
                    "I understand the original is removed only after the copy is verified",
                );
                ui.add_space(8.0);

                // hold-to-confirm + cancel
                ui.horizontal(|ui| {
                    let enabled = can_confirm_offload(dlg.acknowledged, room, dlg.reason.is_none());
                    let label = if !room {
                        "Not enough space"
                    } else if !dlg.acknowledged {
                        "Confirm the acknowledgement"
                    } else {
                        "Hold to move"
                    };
                    let (rect, resp) = ui.allocate_exact_size(vec2(220.0, 30.0), Sense::click_and_drag());
                    let base = if enabled { palette.caution } else { palette.edge };
                    ui.painter().rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, base));
                    if self.dialog_hold > 0.0 {
                        let fill = Rect::from_min_size(rect.min, vec2(rect.width() * self.dialog_hold, rect.height()));
                        ui.painter().rect_filled(fill, Rounding::same(8.0), palette.caution_dim(34));
                    }
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, theme::body(10.5), if enabled { palette.caution } else { palette.faint });
                    if enabled && resp.is_pointer_button_down_on() {
                        self.dialog_hold += ui.input(|i| i.stable_dt).min(0.1) / HOLD_SECS;
                        ui.ctx().request_repaint();
                        if self.dialog_hold >= 1.0 {
                            self.dialog_hold = 0.0;
                            launch = true;
                        }
                    } else {
                        self.dialog_hold = 0.0;
                    }
                    if ui.button(RichText::new("Cancel").font(theme::mono(10.5))).clicked() {
                        keep_open = false;
                    }
                });
            });

        if launch && self.mutation_busy() {
            self.ops(
                OpsKind::Amber,
                "another storage operation is already active",
            );
            self.dialog = Some(dlg);
        } else if launch {
            let vol = dlg.vols[dlg.vol_idx].clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.offload_rx = Some(rx);
            self.offloading = true;
            self.ops(
                OpsKind::Amber,
                format!("offload engaged — {} → {}", dlg.src.display(), vol.name),
            );
            run_offload(
                OffloadJob {
                    src: dlg.src.clone(),
                    mount_path: vol.mount_path.clone(),
                    leave_symlink: dlg.leave_symlink,
                    home: std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_default(),
                },
                tx,
            );
            // dialog closes (not restored)
        } else if keep_open {
            self.dialog = Some(dlg);
        }
    }

    fn restore_dialog(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        let Some(mut dialog) = self.restore_dialog.take() else {
            return;
        };
        let mut keep_open = true;
        let mut launch = false;

        egui::Window::new(RichText::new("Restore to Mac").font(theme::body(13.0)))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(
                    RichText::new(tail_str(
                        &dialog.item.record.origin.display().to_string(),
                        68,
                    ))
                    .font(theme::mono(10.5))
                    .color(palette.ink),
                );
                ui.label(
                    RichText::new(format!(
                        "{} will be copied back to its original location.",
                        fmt_bytes(dialog.item.record.bytes)
                    ))
                    .font(theme::body(10.5))
                    .color(palette.muted),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "DiskDeck verifies the internal copy before removing the external copy. If cleanup cannot finish, both verified copies are kept and a warning is shown.",
                    )
                    .font(theme::body(10.0))
                    .color(palette.muted),
                );
                ui.add_space(10.0);

                if let Some(block) = dialog.block {
                    ui.label(
                        RichText::new(format!("✗ {}", block.message()))
                            .font(theme::body(10.5))
                            .color(palette.danger),
                    );
                } else {
                    ui.checkbox(
                        &mut dialog.acknowledged,
                        "I understand the external copy is removed only after verification",
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let enabled = can_confirm_restore(dialog.acknowledged, dialog.block);
                    let label = if dialog.block.is_some() {
                        "Restore unavailable"
                    } else if !dialog.acknowledged {
                        "Confirm the acknowledgement"
                    } else {
                        "Hold to restore"
                    };
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(220.0, 30.0), Sense::click_and_drag());
                    let base = if enabled { palette.safe } else { palette.edge };
                    ui.painter().rect_stroke(
                        rect,
                        Rounding::same(8.0),
                        Stroke::new(1.0, base),
                    );
                    if self.restore_hold > 0.0 {
                        let fill = Rect::from_min_size(
                            rect.min,
                            vec2(rect.width() * self.restore_hold, rect.height()),
                        );
                        ui.painter().rect_filled(
                            fill,
                            Rounding::same(8.0),
                            palette.safe_dim(34),
                        );
                    }
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        theme::body(10.5),
                        if enabled { palette.safe } else { palette.faint },
                    );
                    if enabled && response.is_pointer_button_down_on() {
                        self.restore_hold += ui.input(|input| input.stable_dt).min(0.1) / HOLD_SECS;
                        ui.ctx().request_repaint();
                        if self.restore_hold >= 1.0 {
                            self.restore_hold = 0.0;
                            launch = true;
                        }
                    } else {
                        self.restore_hold = 0.0;
                    }
                    if ui
                        .button(RichText::new("Cancel").font(theme::mono(10.5)))
                        .clicked()
                    {
                        keep_open = false;
                    }
                });
            });

        if launch && self.mutation_busy() {
            self.ops(
                OpsKind::Amber,
                "another storage operation is already active",
            );
            self.restore_dialog = Some(dialog);
        } else if launch {
            let home = Self::home_dir();
            let (tx, rx) = std::sync::mpsc::channel();
            let job = RestoreJob {
                record: dialog.item.record.clone(),
                registry_path: registry_path_for_home(&home),
                roots: RestoreRoots::production(home),
            };
            match run_restore(job, tx) {
                Ok(()) => {
                    self.restore_rx = Some(rx);
                    self.restoring = true;
                    self.ops(
                        OpsKind::Amber,
                        format!("restore engaged — {}", dialog.item.record.origin.display()),
                    );
                }
                Err(error) => {
                    self.ops(OpsKind::Err, format!("✗ restore could not start — {error}"))
                }
            }
        } else if keep_open {
            self.restore_dialog = Some(dialog);
        }
    }

    fn trash_restore_dialog(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        let Some(mut dialog) = self.trash_restore_dialog.take() else {
            return;
        };
        let mut keep_open = true;
        let mut launch = false;

        egui::Window::new(RichText::new("Restore from Trash").font(theme::body(13.0)))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(
                    RichText::new(&dialog.item.receipt.title)
                        .font(theme::display_md(13.0))
                        .color(palette.ink),
                );
                ui.label(
                    RichText::new(format!(
                        "Original: {}",
                        tail_str(&dialog.item.receipt.origin.display().to_string(), 66)
                    ))
                    .font(theme::mono(9.5))
                    .color(palette.muted),
                );
                let trash = dialog
                    .item
                    .receipt
                    .trash
                    .as_ref()
                    .map(|evidence| evidence.path.display().to_string())
                    .unwrap_or_else(|| "Finder-managed destination".into());
                ui.label(
                    RichText::new(format!("Trash: {}", tail_str(&trash, 69)))
                        .font(theme::mono(9.5))
                        .color(palette.muted),
                );
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        fmt_bytes(dialog.item.receipt.pending_bytes),
                        receipt_age_copy(dialog.item.receipt.completed_at_ms)
                    ))
                    .font(theme::body(9.5))
                    .color(palette.faint),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "DiskDeck repeats every path and identity check, then performs one atomic no-overwrite rename. It never empties Trash or replaces an existing original path.",
                    )
                    .font(theme::body(10.0))
                    .color(palette.muted),
                );
                ui.add_space(10.0);

                if dialog.item.state == ReceiptState::Ready {
                    ui.checkbox(
                        &mut dialog.acknowledged,
                        "I understand this restores the item to its original path",
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "Restore unavailable — {}",
                            receipt_state_copy(&dialog.item.state)
                        ))
                        .font(theme::body(10.5))
                        .color(palette.caution),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let busy = self.mutation_busy();
                    let enabled = can_start_trash_restore(
                        dialog.acknowledged,
                        &dialog.item.state,
                        busy,
                    );
                    let label = if dialog.item.state != ReceiptState::Ready {
                        "Restore unavailable"
                    } else if busy {
                        "Another operation is active"
                    } else if !dialog.acknowledged {
                        "Confirm the acknowledgement"
                    } else {
                        "Hold to restore from Trash"
                    };
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(250.0, 30.0), Sense::click_and_drag());
                    let base = if enabled { palette.safe } else { palette.edge };
                    ui.painter().rect_stroke(
                        rect,
                        Rounding::same(8.0),
                        Stroke::new(1.0, base),
                    );
                    if self.trash_restore_hold > 0.0 {
                        let fill = Rect::from_min_size(
                            rect.min,
                            vec2(
                                rect.width() * self.trash_restore_hold,
                                rect.height(),
                            ),
                        );
                        ui.painter().rect_filled(
                            fill,
                            Rounding::same(8.0),
                            palette.safe_dim(34),
                        );
                    }
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        theme::body(10.0),
                        if enabled { palette.safe } else { palette.faint },
                    );
                    if enabled && response.is_pointer_button_down_on() {
                        self.trash_restore_hold +=
                            ui.input(|input| input.stable_dt).min(0.1) / HOLD_SECS;
                        ui.ctx().request_repaint();
                        if self.trash_restore_hold >= 1.0 {
                            self.trash_restore_hold = 0.0;
                            launch = true;
                        }
                    } else {
                        self.trash_restore_hold = 0.0;
                    }
                    if ui
                        .button(RichText::new("Cancel").font(theme::mono(10.5)))
                        .clicked()
                    {
                        keep_open = false;
                    }
                });
            });

        if launch && !self.mutation_busy() {
            let home = Self::home_dir();
            let (tx, rx) = std::sync::mpsc::channel();
            let job = TrashRestoreJob {
                receipt: dialog.item.receipt.clone(),
                history_path: history_path_for_home(&home),
                home,
            };
            match run_trash_restore(job, tx) {
                Ok(()) => {
                    self.trash_restore_rx = Some(rx);
                    self.restoring_trash = true;
                    self.ops(
                        OpsKind::Amber,
                        format!(
                            "Trash restore engaged — {}",
                            dialog.item.receipt.origin.display()
                        ),
                    );
                }
                Err(error) => {
                    self.ops(
                        OpsKind::Err,
                        format!("✗ Trash restore could not start — {error}"),
                    );
                }
            }
        } else if keep_open {
            self.trash_restore_dialog = Some(dialog);
        }
    }

    fn ops_panel(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        egui::TopBottomPanel::bottom("ops")
            .exact_height(108.0)
            .frame(Frame::none().fill(palette.canvas).inner_margin(Margin {
                left: 12.0,
                right: 12.0,
                top: 0.0,
                bottom: 12.0,
            }))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let sub = if self.cleaning {
                    ("Reclaiming".to_string(), palette.accent)
                } else if self.scanning() {
                    ("Scanning".to_string(), palette.accent)
                } else {
                    ("Idle".to_string(), palette.faint)
                };
                let content = panel_chrome(ui, rect, "Activity", Some(sub));
                ui.allocate_new_ui(
                    egui::UiBuilder::new().max_rect(content.shrink2(vec2(14.0, 6.0))),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                for line in &self.ops {
                                    let color = match line.kind {
                                        OpsKind::Info => palette.accent,
                                        OpsKind::Ok => palette.safe,
                                        OpsKind::Err => palette.danger,
                                        OpsKind::Dim => palette.muted,
                                        OpsKind::Amber => palette.caution,
                                    };
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(&line.time)
                                                .font(theme::mono(9.5))
                                                .color(palette.faint),
                                        );
                                        ui.label(
                                            RichText::new(&line.text)
                                                .font(theme::mono(10.5))
                                                .color(color),
                                        );
                                    });
                                }
                            });
                    },
                );
            });
    }

    fn stamp_overlay(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        let Some((text, t0)) = &self.stamp else {
            return;
        };
        let t = t0.elapsed().as_secs_f32();
        if t > 2.8 {
            self.stamp = None;
            return;
        }
        let alpha = if t < 0.3 {
            t / 0.3
        } else if t > 2.3 {
            (1.0 - (t - 2.3) / 0.5).max(0.0)
        } else {
            1.0
        };
        let text = text.clone();
        egui::Area::new("stamp".into())
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, -60.0))
            .interactable(false)
            .show(ctx, |ui| {
                let safe = palette.safe.gamma_multiply(alpha);
                Frame::none()
                    .fill(palette.surface.gamma_multiply(alpha * 0.92))
                    .stroke(Stroke::new(2.0, safe))
                    .rounding(Rounding::same(12.0))
                    .inner_margin(Margin::symmetric(30.0, 14.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new(&text).font(theme::display(34.0)).color(safe));
                    });
            });
    }
}
