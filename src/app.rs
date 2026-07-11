//! The DiskDeck application: top bar, capacity gauge, scan telemetry,
//! live terrain map, reclaim plan with hold-to-reclaim, ops feed.

use crate::clean::{
    fmt_bytes, fmt_count, open_full_disk_access, reveal_in_finder, run_clean, CleanEvent, CleanJob,
};
use crate::offload::{
    can_confirm_offload, check_movable, external_volumes, has_room, run_offload, OffloadEvent,
    OffloadJob, Volume,
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

pub struct App {
    scan: Option<ScanHandle>,
    view: Option<Arc<Node>>,
    crumbs: Vec<Arc<Node>>,
    zoom: Option<(Rect, Instant)>,
    recs: Vec<RecRow>,
    recs_built: bool,
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

impl App {
    pub fn new() -> Self {
        App {
            scan: None,
            view: None,
            crumbs: Vec::new(),
            zoom: None,
            recs: Vec::new(),
            recs_built: false,
            clean_rx: None,
            cleaning: false,
            hold: 0.0,
            ops: vec![OpsLine {
                time: now_hms(),
                text: "diskdeck v1.0 — feed online. nothing is ever removed without your explicit selection.".into(),
                kind: OpsKind::Dim,
            }],
            stats: disk_stats(),
            stats_at: Instant::now(),
            stamp: None,
            booted: false,
            offload_rx: None,
            offloading: false,
            dialog: None,
            dialog_hold: 0.0,
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
        self.scan = Some(start_scan(DATA_ROOT.into()));
        self.view = None;
        self.crumbs.clear();
        self.zoom = None;
        self.recs.clear();
        self.recs_built = false;
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
        let (files, bytes, denied, ms, done) = (
            scan.root.files(),
            scan.root.bytes(),
            scan.denied.load(Relaxed),
            scan.duration_ms.load(Relaxed),
            scan.state() == ScanState::Done,
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
                } => {
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
                    } else {
                        self.ops(OpsKind::Err, format!("✗ {title} — {message}"));
                    }
                }
                CleanEvent::Done { freed, pending } => {
                    self.cleaning = false;
                    self.clean_rx = None;
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
                    self.ops(OpsKind::Dim, "rescan to refresh the terrain map");
                }
                OffloadEvent::Failed { error } => {
                    self.offloading = false;
                    self.offload_rx = None;
                    self.ops(OpsKind::Err, format!("✗ offload failed — {error}"));
                }
            }
        }
    }

    /// Prepare and show the offload confirm dialog for a real (stripped) path.
    fn open_offload_dialog(&mut self, src: std::path::PathBuf, size: i64) {
        if self.offloading {
            self.ops(OpsKind::Amber, "a move is already in progress");
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
        let reason = check_movable(&src, &home).err();
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
        if self.cleaning {
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
        self.ops(
            OpsKind::Amber,
            format!("reclaim engaged — {} target(s)", jobs.len()),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        self.clean_rx = Some(rx);
        self.cleaning = true;
        run_clean(jobs, tx);
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
}

impl WorkspaceLayout {
    fn from_rect(full: Rect) -> Self {
        let overview = Rect::from_min_size(full.min, vec2(full.width(), 142.0));
        let map = Rect::from_min_max(pos2(full.min.x, overview.max.y + 12.0), full.max);
        Self { overview, map }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_layout_preserves_map_space_at_minimum_window() {
        let full = Rect::from_min_size(Pos2::ZERO, vec2(766.0, 564.0));
        let layout = WorkspaceLayout::from_rect(full);
        assert_eq!(layout.overview.height(), 142.0);
        assert!(layout.map.height() >= 410.0);
        assert_eq!(layout.overview.min, full.min);
        assert_eq!(layout.map.max, full.max);
    }

    #[test]
    fn workspace_layout_keeps_twelve_point_gap() {
        let full = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
        let layout = WorkspaceLayout::from_rect(full);
        assert_eq!(layout.map.min.y - layout.overview.max.y, 12.0);
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
            theme::mono(10.0),
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

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.booted {
            self.booted = true;
            self.begin_scan();
        }
        if self.scan_done() && !self.recs_built {
            self.on_scan_finished();
        }
        self.poll_clean_events();
        self.poll_offload();
        if self.stats_at.elapsed() > Duration::from_secs(5) {
            self.stats = disk_stats();
            self.stats_at = Instant::now();
        }
        if self.scanning()
            || self.cleaning
            || self.zoom.is_some()
            || self.stamp.is_some()
            || self.offloading
            || self.dialog.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(40));
        }

        self.top_bar(ctx);
        self.ops_panel(ctx);
        self.recs_panel(ctx);
        self.central(ctx);
        self.offload_dialog(ctx);
        self.stamp_overlay(ctx);
    }
}

impl App {
    fn top_bar(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        egui::TopBottomPanel::top("topbar")
            .exact_height(56.0)
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

                let capacity_rect =
                    Rect::from_min_size(layout.overview.min, vec2(330.0, layout.overview.height()));
                let tele_rect = Rect::from_min_max(
                    pos2(capacity_rect.max.x + 12.0, layout.overview.min.y),
                    layout.overview.max,
                );
                self.draw_capacity(ui, capacity_rect);
                self.draw_telemetry(ui, tele_rect);
                self.draw_map(ui, layout.map);
            });
    }

    fn draw_capacity(&self, ui: &egui::Ui, rect: Rect) {
        let palette = theme::palette(ui.ctx());
        let content = panel_chrome(
            ui,
            rect,
            "Storage used",
            Some((format!("{:.0}%", self.stats.used_pct), palette.faint)),
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
            theme::mono(28.0),
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
    }

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
        let hint = if depth > 0 {
            Some(("right-click or esc = back".to_string(), palette.faint))
        } else {
            None
        };
        let content = panel_chrome(ui, rect, "Storage map", hint);

        // ── clickable breadcrumb trail + UP button ──
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
                if depth > 0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let up = ui.add(
                            egui::Button::new(
                                RichText::new("↑ Up")
                                    .font(theme::body(11.0))
                                    .color(palette.accent),
                            )
                            .fill(palette.accent_dim(16))
                            .stroke(Stroke::new(1.0, palette.accent_dim(90)))
                            .rounding(Rounding::same(6.0)),
                        );
                        if up
                            .on_hover_text(
                                "back to the previous level (right-click or Esc also works)",
                            )
                            .clicked()
                        {
                            go_to = Some(depth - 1);
                        }
                    });
                }
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

        // back navigation strip (click crumb area): simple "↑ UP" zone
        let resp = ui.interact(map_rect, ui.id().with("treemap"), Sense::click());
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

        let hover = resp.hover_pos();
        let hovered = treemap::paint(ui, map_rect, &items, &laid, hover, zoom);

        // interactions
        if let Some(idx) = hovered {
            let it = &items[idx];
            // tooltip
            egui::Area::new(ui.id().with("tm_tt"))
                .order(egui::Order::Tooltip)
                .interactable(false) // must never swallow clicks meant for the map
                .fixed_pos(hover.unwrap_or(map_rect.center()) + vec2(16.0, 18.0))
                .show(ui.ctx(), |tui| {
                    Frame::none()
                        .fill(palette.surface_raised)
                        .stroke(Stroke::new(1.0, palette.edge))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(11.0, 8.0))
                        .show(tui, |tui| {
                            tui.label(RichText::new(&it.label).font(theme::body(12.5)).color(palette.ink).strong());
                            let meta = if it.synthetic {
                                format!("{} · {} small items aggregated", fmt_bytes(it.bytes), fmt_count(it.files))
                            } else {
                                let total = node.bytes().max(1);
                                format!(
                                    "{} · {:.1}% of this view · {} files",
                                    fmt_bytes(it.bytes),
                                    it.bytes as f64 / total as f64 * 100.0,
                                    fmt_count(it.files)
                                )
                            };
                            tui.label(RichText::new(meta).font(theme::mono(10.5)).color(palette.muted));
                            let hint = if it.denied {
                                "access denied — grant Full Disk Access to see inside"
                            } else if it.is_dir && !it.synthetic {
                                "click = drill in · ⌘-click = move to SSD · right-click = back · ⌥-click = reveal"
                            } else if !it.synthetic {
                                "⌘-click = move to SSD · right-click = back · ⌥-click = reveal"
                            } else {
                                "aggregate of items too small to chart · right-click = back"
                            };
                            tui.label(RichText::new(hint).font(theme::mono(9.5)).color(palette.faint));
                        });
                });

            if resp.clicked() {
                let cmd = ui.input(|i| i.modifiers.command);
                let alt = ui.input(|i| i.modifiers.alt);
                if cmd {
                    if let Some(n) = &it.node {
                        if !it.synthetic {
                            let real = strip_data_root(&n.path);
                            self.open_offload_dialog(real, n.bytes());
                        }
                    }
                } else if alt {
                    if let Some(n) = &it.node {
                        reveal_in_finder(&n.path);
                    }
                } else if it.is_dir && !it.synthetic && !it.denied {
                    if let Some(n) = &it.node {
                        let src = laid
                            .iter()
                            .find(|(i, _)| *i == idx)
                            .map(|(_, r)| *r)
                            .unwrap_or(map_rect);
                        self.crumbs.push(n.clone());
                        self.view = Some(n.clone());
                        self.zoom = Some((src, Instant::now()));
                    }
                }
            }
        }
        // secondary click / Esc-style up navigation
        if resp.secondary_clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.crumbs.pop();
            self.view = self.crumbs.last().cloned();
            self.zoom = None;
        }
    }

    fn recs_panel(&mut self, ctx: &Context) {
        let palette = theme::palette(ctx);
        let meta = if self.recs.is_empty() {
            String::new()
        } else {
            let total: i64 = self.recs.iter().map(|r| r.rec.bytes).sum();
            format!("{} targets · {}", self.recs.len(), fmt_bytes(total))
        };
        egui::SidePanel::right("recs")
            .exact_width(390.0)
            .frame(Frame::none().fill(palette.canvas).inner_margin(Margin {
                left: 0.0,
                right: 12.0,
                top: 12.0,
                bottom: 12.0,
            }))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let content = panel_chrome(ui, rect, "Reclaimable", Some((meta, palette.faint)));
                let footer_h = 64.0;
                let list_rect = Rect::from_min_max(content.min, pos2(content.max.x, content.max.y - footer_h));
                let footer_rect = Rect::from_min_max(pos2(content.min.x, list_rect.max.y), content.max);

                let mut reveal: Option<std::path::PathBuf> = None;
                let mut offload_req: Option<(std::path::PathBuf, i64)> = None;
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(list_rect.shrink2(vec2(10.0, 6.0))), |ui| {
                    if self.recs.is_empty() {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new("Recommendations appear after a scan")
                                    .font(theme::body(12.0))
                                    .color(palette.faint),
                            );
                            ui.label(
                                RichText::new("each one explains what it is, what restoring costs,\nand how it gets removed")
                                    .font(theme::mono(9.5))
                                    .color(palette.faint),
                            );
                        });
                        return;
                    }
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
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
                                Tier::Caution => ("Review · may require a download", palette.caution),
                            };
                            let total: i64 = group.iter().map(|&i| self.recs[i].rec.bytes).sum();
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 3.5, color);
                                ui.label(RichText::new(label).font(theme::body(10.5)).color(color).strong());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(fmt_bytes(total)).font(theme::mono(10.5)).color(palette.muted));
                                });
                            });
                            ui.add_space(4.0);
                            for i in group {
                                if let Some(path) = self.rec_card(ui, i, &mut offload_req) {
                                    reveal = Some(path);
                                }
                                ui.add_space(5.0);
                            }
                        }
                        ui.add_space(6.0);
                    });
                });
                if let Some(p) = reveal {
                    reveal_in_finder(&p);
                }
                if let Some((p, sz)) = offload_req {
                    self.open_offload_dialog(p, sz);
                }

                self.reclaim_footer(ui, footer_rect);
            });
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
        let row = &mut self.recs[idx];
        let selected_color = if row.action == Action::Delete {
            palette.danger
        } else if row.rec.tier == Tier::Caution {
            palette.caution
        } else {
            palette.safe
        };
        let (border, fill) = match (&row.status, row.checked) {
            (RecStatus::Running, _) => (palette.accent_dim(110), palette.surface_raised),
            (RecStatus::Failed(_), _) => (palette.danger_dim(120), palette.surface_raised),
            (_, true) => (
                Color32::from_rgba_unmultiplied(
                    selected_color.r(),
                    selected_color.g(),
                    selected_color.b(),
                    110,
                ),
                palette.surface_raised,
            ),
            _ => (palette.edge_soft, palette.surface_raised),
        };
        let dimmed = matches!(row.status, RecStatus::Cleared(_) | RecStatus::InTrash(_));

        Frame::none()
            .fill(fill)
            .stroke(Stroke::new(1.0, border))
            .rounding(Rounding::same(10.0))
            .inner_margin(Margin::symmetric(10.0, 9.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if dimmed {
                    ui.disable();
                }
                ui.horizontal(|ui| {
                    ui.checkbox(&mut row.checked, "");
                    ui.vertical(|ui| {
                        let title_resp = ui.add(
                            Label::new(
                                RichText::new(&row.rec.title)
                                    .font(theme::body(13.0))
                                    .color(palette.ink)
                                    .strong(),
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
                            if ui
                                .add(
                                    Label::new(
                                        RichText::new("→ SSD")
                                            .font(theme::mono(10.0))
                                            .color(palette.muted),
                                    )
                                    .sense(Sense::click()),
                                )
                                .on_hover_text("move this to an attached external drive")
                                .clicked()
                            {
                                *offload_out = Some((rec_real.clone(), rec_size));
                            }
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        ui.vertical(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
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
                            });
                            // action chip
                            let (chip_txt, chip_color, cyclable) =
                                match (row.rec.action, row.action) {
                                    (Action::Command, _) => ("Script", palette.accent, false),
                                    (Action::Empty, _) => ("Empty", palette.accent, false),
                                    (_, Action::Trash) => ("Trash", palette.safe, true),
                                    (_, Action::Delete) => ("Erase", palette.danger, true),
                                    _ => ("?", palette.muted, false),
                                };
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
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
                                        let pos = allowed
                                            .iter()
                                            .position(|a| *a == row.action)
                                            .unwrap_or(0);
                                        row.action = allowed[(pos + 1) % allowed.len()];
                                    }
                                }
                                if cyclable {
                                    resp.on_hover_text(
                                        "click to switch between Trash and permanent erase",
                                    );
                                }
                            });
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
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Min),
                                    |ui| {
                                        let resp = ui.label(
                                            RichText::new(txt).font(theme::mono(9.5)).color(color),
                                        );
                                        if let RecStatus::Failed(msg) = &row.status {
                                            resp.on_hover_text(msg);
                                        }
                                    },
                                );
                            }
                        });
                    });
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
        let enabled = count > 0 && !self.cleaning;
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

        if launch {
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
                },
                tx,
            );
            // dialog closes (not restored)
        } else if keep_open {
            self.dialog = Some(dlg);
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
