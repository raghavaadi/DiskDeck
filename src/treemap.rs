//! Squarified treemap: layout + GPU painting, with live growth during the
//! scan and a DaisyDisk-style zoom animation on drill-down.

use crate::clean::fmt_bytes;
use crate::scan::Node;
use crate::theme;
use egui::{Align2, Color32, Pos2, Rect, Stroke, Vec2};
use std::sync::Arc;

const MAX_RECTS: usize = 40;

pub struct Item {
    pub node: Option<Arc<Node>>,
    pub label: String,
    pub bytes: i64,
    pub files: i64,
    pub is_dir: bool,
    pub denied: bool,
    pub synthetic: bool,
}

/// Snapshot a node's children for layout: top N by size plus one synthetic
/// aggregate for everything smaller (and the node's own folded small items).
pub fn collect_items(node: &Arc<Node>) -> Vec<Item> {
    let mut kids = node.kids();
    kids.sort_by_key(|c| -c.bytes());

    let mut items: Vec<Item> = Vec::new();
    let mut overflow_bytes = 0i64;
    let mut overflow_count = 0i64;
    for (i, c) in kids.iter().enumerate() {
        if c.bytes() <= 0 {
            continue;
        }
        if i < MAX_RECTS {
            items.push(Item {
                label: c.name.clone(),
                bytes: c.bytes(),
                files: c.files(),
                is_dir: c.is_dir,
                denied: c.denied.load(std::sync::atomic::Ordering::Relaxed),
                synthetic: false,
                node: Some(c.clone()),
            });
        } else {
            overflow_bytes += c.bytes();
            overflow_count += c.files();
        }
    }
    let small_b = node.small_bytes.load(std::sync::atomic::Ordering::Relaxed) + overflow_bytes;
    let small_c = node.small_count.load(std::sync::atomic::Ordering::Relaxed) + overflow_count;
    if small_b > 0 {
        items.push(Item {
            label: format!("{} smaller items", crate::clean::fmt_count(small_c)),
            bytes: small_b,
            files: small_c,
            is_dir: false,
            denied: false,
            synthetic: true,
            node: None,
        });
    }
    items
}

/// Classic squarified layout. Returns (item index, rect) pairs.
pub fn squarify(items: &[Item], area: Rect) -> Vec<(usize, Rect)> {
    let total: i64 = items.iter().map(|i| i.bytes).sum();
    if total <= 0 || area.width() < 4.0 || area.height() < 4.0 {
        return Vec::new();
    }
    let scale = (area.width() * area.height()) as f64 / total as f64;
    let mut scaled: Vec<(usize, f64)> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.bytes > 0)
        .map(|(i, it)| (i, it.bytes as f64 * scale))
        .collect();
    scaled.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = Vec::with_capacity(scaled.len());
    let (mut cx, mut cy) = (area.min.x as f64, area.min.y as f64);
    let (mut cw, mut ch) = (area.width() as f64, area.height() as f64);
    let mut row: Vec<(usize, f64)> = Vec::new();

    fn worst(row: &[(usize, f64)], side: f64) -> f64 {
        let sum: f64 = row.iter().map(|e| e.1).sum();
        if sum <= 0.0 {
            return f64::INFINITY;
        }
        let max = row.iter().map(|e| e.1).fold(0.0f64, f64::max);
        let min = row.iter().map(|e| e.1).fold(f64::INFINITY, f64::min);
        let s2 = sum * sum;
        (side * side * max / s2).max(s2 / (side * side * min))
    }

    let layout_row = |row: &[(usize, f64)],
                      cx: &mut f64,
                      cy: &mut f64,
                      cw: &mut f64,
                      ch: &mut f64,
                      out: &mut Vec<(usize, Rect)>| {
        let sum: f64 = row.iter().map(|e| e.1).sum();
        if sum <= 0.0 {
            return;
        }
        if *cw >= *ch {
            let rw = sum / *ch;
            let mut oy = *cy;
            for &(idx, a) in row {
                let rh = a / rw;
                out.push((
                    idx,
                    Rect::from_min_size(
                        Pos2::new(*cx as f32, oy as f32),
                        Vec2::new(rw as f32, rh as f32),
                    ),
                ));
                oy += rh;
            }
            *cx += rw;
            *cw -= rw;
        } else {
            let rh = sum / *cw;
            let mut ox = *cx;
            for &(idx, a) in row {
                let rw = a / rh;
                out.push((
                    idx,
                    Rect::from_min_size(
                        Pos2::new(ox as f32, *cy as f32),
                        Vec2::new(rw as f32, rh as f32),
                    ),
                ));
                ox += rw;
            }
            *cy += rh;
            *ch -= rh;
        }
    };

    for e in scaled {
        let side = cw.min(ch);
        if row.is_empty() || {
            let mut with = row.clone();
            with.push(e);
            worst(&with, side) <= worst(&row, side)
        } {
            row.push(e);
        } else {
            layout_row(&row, &mut cx, &mut cy, &mut cw, &mut ch, &mut out);
            row.clear();
            row.push(e);
        }
    }
    if !row.is_empty() {
        layout_row(&row, &mut cx, &mut cy, &mut cw, &mut ch, &mut out);
    }
    out
}

const FILLS: [Color32; 6] = [
    Color32::from_rgb(36, 49, 70),
    Color32::from_rgb(33, 44, 62),
    Color32::from_rgb(30, 40, 55),
    Color32::from_rgb(28, 36, 49),
    Color32::from_rgb(26, 33, 44),
    Color32::from_rgb(24, 30, 40),
];

/// Map `r` (laid out in `full`) into `src`, for the zoom-from animation.
fn map_into(r: Rect, full: Rect, src: Rect) -> Rect {
    let fx = |x: f32| src.min.x + (x - full.min.x) / full.width() * src.width();
    let fy = |y: f32| src.min.y + (y - full.min.y) / full.height() * src.height();
    Rect::from_min_max(
        Pos2::new(fx(r.min.x), fy(r.min.y)),
        Pos2::new(fx(r.max.x), fy(r.max.y)),
    )
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min + (b.min - a.min) * t, a.max + (b.max - a.max) * t)
}

/// Paint the treemap. `zoom` is (source rect, eased 0..1 progress) for the
/// drill-down animation. Returns the index of the hovered item.
pub fn paint(
    ui: &egui::Ui,
    area: Rect,
    items: &[Item],
    laid: &[(usize, Rect)],
    hover_pos: Option<Pos2>,
    zoom: Option<(Rect, f32)>,
) -> Option<usize> {
    let painter = ui.painter().with_clip_rect(area);
    let mut hovered = None;

    for &(idx, rect) in laid {
        let it = &items[idx];
        let rect = match zoom {
            Some((src, t)) if t < 1.0 => lerp_rect(map_into(rect, area, src), rect, t),
            _ => rect,
        };
        let r = rect.shrink(1.0);
        if r.width() < 1.0 || r.height() < 1.0 {
            continue;
        }
        let is_hover = hover_pos.map_or(false, |p| rect.contains(p));
        if is_hover {
            hovered = Some(idx);
        }
        let mut fill = FILLS[idx.min(FILLS.len() - 1)];
        if it.synthetic {
            fill = Color32::from_rgb(17, 22, 30);
        }
        painter.rect_filled(r, 2.5, fill);
        let stroke = if is_hover {
            Stroke::new(1.5, theme::amber_dim(220))
        } else if it.denied {
            Stroke::new(1.0, theme::red_dim(140))
        } else {
            Stroke::new(1.0, theme::edge_soft())
        };
        painter.rect_stroke(r, 2.5, stroke);

        // labels only where they fit
        if r.width() > 64.0 && r.height() > 30.0 {
            let p = painter.with_clip_rect(r.shrink(4.0));
            let name_color = if it.synthetic {
                theme::FAINT
            } else {
                theme::INK
            };
            p.text(
                r.min + Vec2::new(8.0, 6.0),
                Align2::LEFT_TOP,
                &it.label,
                theme::body(if r.width() > 160.0 { 12.0 } else { 10.5 }),
                name_color,
            );
            if r.height() > 46.0 {
                p.text(
                    r.min + Vec2::new(8.0, 22.0),
                    Align2::LEFT_TOP,
                    fmt_bytes(it.bytes),
                    theme::mono(10.0),
                    theme::amber_dim(210),
                );
            }
        }
    }
    hovered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(bytes: i64) -> Item {
        Item {
            node: None,
            label: String::new(),
            bytes,
            files: 0,
            is_dir: true,
            denied: false,
            synthetic: false,
        }
    }

    #[test]
    fn squarify_conserves_area_and_bounds() {
        let items: Vec<Item> = [600, 300, 100, 50, 25, 10]
            .iter()
            .map(|&b| item(b))
            .collect();
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0));
        let laid = squarify(&items, area);
        assert_eq!(laid.len(), items.len());
        let sum: f32 = laid.iter().map(|(_, r)| r.width() * r.height()).sum();
        let total = area.width() * area.height();
        assert!((sum - total).abs() / total < 0.01, "area conserved");
        for (_, r) in &laid {
            assert!(area.expand(0.5).contains_rect(*r), "rect within bounds");
        }
    }

    #[test]
    fn squarify_empty_and_degenerate() {
        assert!(squarify(
            &[],
            Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0))
        )
        .is_empty());
        let items = vec![item(100)];
        assert!(squarify(&items, Rect::from_min_size(Pos2::ZERO, Vec2::new(1.0, 1.0))).is_empty());
    }
}
