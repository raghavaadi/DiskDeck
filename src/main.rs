#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod apfs;
mod app;
mod clean;
mod developer;
mod file_review;
mod forecast;
mod history;
mod leftovers;
mod monitor;
mod moves;
mod offload;
mod reclaim_plan;
mod rules;
mod scan;
mod theme;
mod transfer;
mod treemap;

use std::sync::Arc;

fn main() -> eframe::Result {
    let icon = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .map(|img| {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        })
        .ok();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1480.0, 920.0])
        .with_min_inner_size([1180.0, 740.0])
        .with_title("DiskDeck")
        .with_app_id("com.buddyhq.headroom-rs");
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    eframe::run_native(
        "DiskDeck",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(app::App::new()))
        }),
    )
}
