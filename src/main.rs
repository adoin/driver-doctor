#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod ai_context;
mod app;
mod config;
mod icons;
mod scan;
mod shell_icons;
mod structure;

fn main() -> eframe::Result<()> {
    app::run()
}
