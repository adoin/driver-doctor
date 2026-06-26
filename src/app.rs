use crate::ai::{
    analyze_current_structure, analyze_deep_structure, analyze_folder, generate_cleanup_plan,
    generate_cleanup_plan_from_health_report, generate_health_check_step,
    native_search_profile_label, test_connection, HealthCheckReply,
};
use crate::config::{AiConfig, AppConfig};
use crate::icons::{bar_color, health_check_button, size_bar};
use crate::scan::{
    format_size, list_drives_with_space, quick_list_directory, scan_all_drives, scan_directory,
    scan_directory_with_tree, DriveInfo, ScanEntry, ScanNode, ScanProgress, ScanResult,
};
use crate::shell_icons::{drive_icon_path, ShellIconCache};
use crate::structure::{
    build_deep_structure_rag, compact_scan_entries, folded_label, format_current_level_rag,
    format_diagnostic_tree_rag, format_scan_tree_diagnostic_rag,
};
use eframe::egui;
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const HEALTH_CHECK_LABEL: &str = "健康检查";
const CARD_RADIUS: u8 = 6;
const DRIVE_CARD_GAP: f32 = 8.0;
const REGION_GAP: i8 = 8;
const REGION_INNER_MARGIN: i8 = 8;
const BUTTON_WIDTH: f32 = 72.0;
const BUTTON_HEIGHT: f32 = 24.0;
const SHOW_PANEL_SEPARATOR_LINES: bool = false;
const DIRECTORY_TABLE_HEADER_RESIZE: bool = true;
const DIRECTORY_TABLE_HEADER_HEIGHT: f32 = 22.0;
const DIRECTORY_HEADER_GAP_HALF: f32 = 3.0;
const DIRECTORY_HEADER_RESIZE_HANDLE_WIDTH: f32 = 12.0;
const PANEL_RESIZE_HANDLE_WIDTH: f32 = 16.0;
const SHOW_RESIZE_DEBUG_LINES: bool = false;
const RESIZE_DEBUG_LOG_FILE: &str = "driver-doctor-resize-debug.log";
const HEALTH_DEBUG_LOG_FILE: &str = "driver-doctor-health-debug.log";
const HEALTH_CHECK_INITIAL_SCOPE_BUDGET: usize = 24;
const HEALTH_CHECK_INITIAL_MAX_DEPTH: u32 = 4;
const SIDEBAR_DEFAULT_WIDTH: f32 = 220.0;
const SIDEBAR_MIN_WIDTH: f32 = 200.0;
const SIDEBAR_MAX_WIDTH: f32 = 280.0;
const AI_PANEL_DEFAULT_WIDTH: f32 = 340.0;
const AI_PANEL_MIN_WIDTH: f32 = 280.0;
const AI_PANEL_MAX_WIDTH: f32 = 1000.0;
const CENTER_PANEL_MIN_WIDTH: f32 = 360.0;
const DIRECTORY_TABLE_COLUMN_MIN_WIDTHS: [f32; 6] = [160.0, 132.0, 64.0, 56.0, 56.0, 64.0];
const DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS: [f32; 6] = [220.0, 150.0, 72.0, 64.0, 56.0, 64.0];

fn app_bg_color() -> egui::Color32 {
    egui::Color32::from_rgb(232, 238, 246)
}

fn app_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 2],
        blur: 10,
        spread: 0,
        color: egui::Color32::from_black_alpha(24),
    }
}

fn quiet_button(label: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(label)
        .min_size(egui::vec2(BUTTON_WIDTH, BUTTON_HEIGHT))
        .fill(egui::Color32::from_rgb(238, 242, 247))
        .stroke(egui::Stroke::NONE)
}

fn danger_button(label: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(label)
        .min_size(egui::vec2(BUTTON_WIDTH, BUTTON_HEIGHT))
        .fill(egui::Color32::from_rgb(190, 70, 70))
        .stroke(egui::Stroke::NONE)
}

fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 8))
        .outer_margin(egui::Margin::symmetric(0, DRIVE_CARD_GAP as i8 / 2))
        .corner_radius(egui::CornerRadius::same(CARD_RADIUS))
        .fill(egui::Color32::from_rgb(252, 253, 255))
        .stroke(egui::Stroke::NONE)
        .shadow(app_shadow())
}

fn region_frame() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(REGION_INNER_MARGIN))
        .outer_margin(egui::Margin::same(REGION_GAP / 2))
        .corner_radius(egui::CornerRadius::same(CARD_RADIUS))
        .fill(egui::Color32::from_rgb(248, 250, 253))
        .stroke(egui::Stroke::NONE)
        .shadow(app_shadow())
}

fn region_card_frame() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(REGION_INNER_MARGIN))
        .corner_radius(egui::CornerRadius::same(CARD_RADIUS))
        .fill(egui::Color32::from_rgb(248, 250, 253))
        .stroke(egui::Stroke::NONE)
        .shadow(app_shadow())
}

fn panel_shell_frame() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::ZERO)
        .fill(app_bg_color())
        .stroke(egui::Stroke::NONE)
}

fn drive_summary_label(drive: &DriveInfo) -> String {
    format!(
        "{}  容量 {}  可用 {}  ({:.0}% 已用)",
        drive.label(),
        format_size(drive.total_bytes),
        format_size(drive.free_bytes),
        drive.used_percent()
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Name,
    Size,
    Percent,
    FileCount,
}

fn directory_table_columns() -> &'static [(SortColumn, &'static str)] {
    &[
        (SortColumn::Name, "名称"),
        (SortColumn::Size, "占用"),
        (SortColumn::Percent, "占比"),
        (SortColumn::FileCount, "文件"),
    ]
}

fn clamp_directory_column_width(index: usize, width: f32) -> f32 {
    width.max(DIRECTORY_TABLE_COLUMN_MIN_WIDTHS[index])
}

fn resize_directory_columns(widths: &mut [f32; 6], index: usize, delta: f32) {
    if index + 1 >= widths.len() || delta.abs() < f32::EPSILON {
        return;
    }
    let min_left = DIRECTORY_TABLE_COLUMN_MIN_WIDTHS[index];
    let min_right = DIRECTORY_TABLE_COLUMN_MIN_WIDTHS[index + 1];
    let min_delta = min_left - widths[index];
    let max_delta = widths[index + 1] - min_right;
    let delta = delta.clamp(min_delta, max_delta);
    widths[index] += delta;
    widths[index + 1] -= delta;
}

fn clamp_panel_width(width: f32, min: f32, max: f32) -> f32 {
    width.clamp(min, max)
}

fn configured_panel_width(value: Option<f32>, default: f32, min: f32, max: f32) -> f32 {
    value
        .map(|width| clamp_panel_width(width, min, max))
        .unwrap_or(default)
}

fn dynamic_ai_panel_max_width(full_width: f32, sidebar_width: f32) -> f32 {
    let gap = REGION_GAP as f32;
    let available = full_width - gap * 4.0 - sidebar_width - CENTER_PANEL_MIN_WIDTH;
    available.max(AI_PANEL_MIN_WIDTH).min(AI_PANEL_MAX_WIDTH)
}

fn configured_directory_col_widths(value: Option<[f32; 6]>) -> [f32; 6] {
    let mut widths = value.unwrap_or(DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS);
    for (index, width) in widths.iter_mut().enumerate() {
        *width = clamp_directory_column_width(index, *width);
    }
    widths
}

fn directory_table_total_width(widths: &[f32; 6]) -> f32 {
    widths
        .iter()
        .take(directory_table_columns().len())
        .sum::<f32>()
}

fn directory_table_content_width(available_width: f32, widths: &[f32; 6]) -> f32 {
    available_width.max(directory_table_total_width(widths))
}

fn directory_header_fill_rect(rect: egui::Rect) -> egui::Rect {
    rect.shrink2(egui::vec2(DIRECTORY_HEADER_GAP_HALF, 0.0))
}

fn directory_header_sort_rect(rect: egui::Rect, index: usize, total: usize) -> egui::Rect {
    let mut sort_rect = directory_header_fill_rect(rect);
    let gutter = DIRECTORY_HEADER_RESIZE_HANDLE_WIDTH / 2.0;
    if index > 0 {
        sort_rect.min.x += gutter;
    }
    if index + 1 < total {
        sort_rect.max.x -= gutter;
    }
    if sort_rect.max.x < sort_rect.min.x {
        sort_rect.max.x = sort_rect.min.x;
    }
    sort_rect
}

fn write_resize_debug_log(
    kind: &str,
    id: impl std::fmt::Display,
    handle: egui::Rect,
    response: &egui::Response,
    ui: &egui::Ui,
) {
    if !SHOW_RESIZE_DEBUG_LINES {
        return;
    }

    let (pointer_pos, any_down, primary_down, delta) = ui.input(|input| {
        (
            input.pointer.hover_pos(),
            input.pointer.any_down(),
            input.pointer.primary_down(),
            input.pointer.delta(),
        )
    });
    let pointer_inside = pointer_pos.is_some_and(|pos| handle.contains(pos));
    let pointer_near = pointer_pos.is_some_and(|pos| handle.expand(8.0).contains(pos));

    if !(pointer_near
        || response.hovered()
        || response.drag_started()
        || response.dragged()
        || response.drag_stopped())
    {
        return;
    }

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let pointer = pointer_pos
        .map(|pos| format!("{:.1},{:.1}", pos.x, pos.y))
        .unwrap_or_else(|| "-".into());
    let line = format!(
        "{millis} kind={kind} id={id} pointer={pointer} inside={pointer_inside} near={pointer_near} hovered={} drag_started={} dragged={} drag_stopped={} any_down={} primary_down={} delta={:.1},{:.1} handle=({:.1},{:.1})-({:.1},{:.1})\n",
        response.hovered(),
        response.drag_started(),
        response.dragged(),
        response.drag_stopped(),
        any_down,
        primary_down,
        delta.x,
        delta.y,
        handle.left(),
        handle.top(),
        handle.right(),
        handle.bottom(),
    );

    let path = std::env::temp_dir().join(RESIZE_DEBUG_LOG_FILE);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn write_health_debug_log(line: impl AsRef<str>) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(HEALTH_DEBUG_LOG_FILE);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{millis} {}", line.as_ref());
    }
}

fn vertical_resize_delta(ui: &mut egui::Ui, rect: egui::Rect, id: &'static str) -> Option<f32> {
    let handle = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(PANEL_RESIZE_HANDLE_WIDTH, rect.height()),
    );
    if SHOW_RESIZE_DEBUG_LINES {
        ui.painter().rect_filled(
            handle,
            egui::CornerRadius::ZERO,
            egui::Color32::from_rgba_unmultiplied(45, 120, 255, 32),
        );
        ui.painter().line_segment(
            [
                egui::pos2(handle.center().x, handle.top()),
                egui::pos2(handle.center().x, handle.bottom()),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(40, 110, 255)),
        );
    }
    let response = ui
        .interact(handle, ui.id().with(id), egui::Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    write_resize_debug_log("panel", id, handle, &response, ui);
    response
        .dragged()
        .then(|| ui.input(|input| input.pointer.delta().x))
        .filter(|delta| delta.abs() > 0.0)
}

fn directory_column_resize_delta(
    ui: &mut egui::Ui,
    header_rect: egui::Rect,
    x: f32,
    index: usize,
) -> Option<f32> {
    let handle = egui::Rect::from_center_size(
        egui::pos2(x, header_rect.center().y),
        egui::vec2(DIRECTORY_HEADER_RESIZE_HANDLE_WIDTH, header_rect.height()),
    );
    if SHOW_RESIZE_DEBUG_LINES {
        ui.painter().rect_filled(
            handle,
            egui::CornerRadius::ZERO,
            egui::Color32::from_rgba_unmultiplied(255, 80, 80, 36),
        );
        ui.painter().line_segment(
            [
                egui::pos2(x, header_rect.top()),
                egui::pos2(x, header_rect.bottom()),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 50, 50)),
        );
    }
    let response = ui
        .interact(
            handle,
            ui.id().with(("directory-column-resize", index)),
            egui::Sense::click_and_drag(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    write_resize_debug_log("column", index, handle, &response, ui);
    response
        .dragged()
        .then(|| ui.input(|input| input.pointer.delta().x))
        .filter(|delta| delta.abs() > 0.0)
}

fn table_cell_ui(ui: &mut egui::Ui, rect: egui::Rect, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            add_contents(ui);
        },
    );
}

fn region_at_rect(ui: &mut egui::Ui, rect: egui::Rect, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        region_card_frame().show(ui, |ui| {
            let inner = egui::vec2(
                (rect.width() - REGION_INNER_MARGIN as f32 * 2.0).max(0.0),
                (rect.height() - REGION_INNER_MARGIN as f32 * 2.0).max(0.0),
            );
            ui.set_min_size(inner);
            add_contents(ui);
        });
    });
}

#[derive(Clone, Copy, Debug)]
struct MainLayoutRects {
    sidebar: egui::Rect,
    center: egui::Rect,
    ai: egui::Rect,
    sidebar_center_gap: egui::Rect,
    center_ai_gap: egui::Rect,
}

fn main_layout_rects(full: egui::Rect, sidebar_width: f32, ai_panel_width: f32) -> MainLayoutRects {
    let gap = REGION_GAP as f32;
    let content = full.shrink(gap);
    let sidebar_right = content.left() + sidebar_width;
    let ai_left = content.right() - ai_panel_width;
    let sidebar = egui::Rect::from_min_max(
        content.left_top(),
        egui::pos2(sidebar_right, content.bottom()),
    );
    let sidebar_center_gap = egui::Rect::from_min_max(
        egui::pos2(sidebar.right(), content.top()),
        egui::pos2(sidebar.right() + gap, content.bottom()),
    );
    let ai = egui::Rect::from_min_max(egui::pos2(ai_left, content.top()), content.right_bottom());
    let center_ai_gap = egui::Rect::from_min_max(
        egui::pos2(ai.left() - gap, content.top()),
        egui::pos2(ai.left(), content.bottom()),
    );
    let center =
        egui::Rect::from_min_max(sidebar_center_gap.right_top(), center_ai_gap.left_bottom());

    MainLayoutRects {
        sidebar,
        center,
        ai,
        sidebar_center_gap,
        center_ai_gap,
    }
}

fn drive_capacity_cell(entry: &ScanEntry) -> String {
    if entry.allocated > entry.size {
        format!(
            "{}/{}",
            format_size(entry.size),
            format_size(entry.allocated)
        )
    } else {
        format_size(entry.size)
    }
}

fn health_check_scan_status(label: &str) -> String {
    format!("正在扫描 {label}，完成后生成健康检查报告...")
}

fn health_check_report_status(label: &str) -> String {
    format!("正在基于 {label} 扫描结果生成健康检查报告...")
}

fn health_check_ai_round_status(round: u32, force_report: bool) -> String {
    if force_report {
        format!("正在请求 AI 生成最终健康检查报告（第 {round} 轮）...")
    } else {
        format!("正在请求 AI 判断是否需要深挖（第 {round} 轮）...")
    }
}

fn health_check_deep_scan_status(round: u32, index: usize, total: usize, path: &Path) -> String {
    format!(
        "AI 请求继续深挖 {total} 个路径。\n正在扫描第 {index}/{total} 个（第 {round} 轮）：{}",
        path.display()
    )
}

fn health_check_initial_deep_targets(entries: &[ScanEntry]) -> Vec<PathBuf> {
    let total: u64 = entries.iter().map(|entry| entry.size).sum();
    let min_size = 1024_u64 * 1024 * 1024;
    let mut targets: Vec<&ScanEntry> = entries
        .iter()
        .filter(|entry| entry.is_dir)
        .filter(|entry| {
            entry.size >= min_size || (total > 0 && entry.size as f64 / total as f64 >= 0.02)
        })
        .collect();
    targets.sort_by(|a, b| b.size.cmp(&a.size));
    targets
        .into_iter()
        .take(6)
        .map(|entry| entry.path.clone())
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortPhase {
    Asc,
    Desc,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AiPanelTab {
    FolderAnalysis,
    CleanupPlan,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AiTaskTarget {
    Analysis,
    Cleanup,
    HealthCheck,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Home,
    Directory,
    FullScan,
}

#[derive(Clone)]
enum FileListRow {
    Entry {
        entry: ScanEntry,
        source_index: usize,
    },
    Folded {
        key: String,
        label: String,
        count: usize,
        size: u64,
    },
}

enum ScanMsg {
    Progress {
        generation: u64,
        progress: ScanProgress,
    },
    Done {
        generation: u64,
        result: Result<ScanResult, String>,
    },
}

enum AiMsg {
    Progress {
        generation: u64,
        target: AiTaskTarget,
        message: String,
    },
    Done {
        generation: u64,
        target: AiTaskTarget,
        result: Result<String, String>,
    },
}

pub struct DriverDoctorApp {
    config: AppConfig,
    scan_path: String,
    entries: Vec<ScanEntry>,
    scan_tree: Option<ScanNode>,
    drives: Vec<DriveInfo>,
    view_mode: ViewMode,
    selected: Option<usize>,
    expanded_folded: BTreeSet<String>,
    sort_col: Option<SortColumn>,
    sort_phase: Option<SortPhase>,
    sidebar_width: f32,
    ai_panel_width: f32,
    directory_col_widths: [f32; 6],

    scanning: bool,
    scan_generation: u64,
    scan_cancel: Arc<AtomicBool>,
    scan_rx: Option<Receiver<ScanMsg>>,
    scan_status: String,
    scanned_files: u64,
    scan_started_at: Option<Instant>,

    show_settings: bool,
    settings_draft: AiConfig,
    settings_test_loading: bool,
    settings_test_result: String,
    settings_test_rx: Option<Receiver<AiMsg>>,
    settings_test_started_at: Option<Instant>,

    ai_tab: AiPanelTab,
    ai_analysis_output: String,
    ai_cleanup_output: String,
    cleanup_bat_status: String,
    ai_loading: bool,
    ai_generation: u64,
    ai_cancel: Arc<AtomicBool>,
    ai_rx: Option<Receiver<AiMsg>>,
    ai_started_at: Option<Instant>,
    pending_health_check: Option<DriveInfo>,

    shell_icons: ShellIconCache,
}

impl Default for DriverDoctorApp {
    fn default() -> Self {
        let config = AppConfig::load();
        let drives = list_drives_with_space();
        let sidebar_width = configured_panel_width(
            config.layout.sidebar_width,
            SIDEBAR_DEFAULT_WIDTH,
            SIDEBAR_MIN_WIDTH,
            SIDEBAR_MAX_WIDTH,
        );
        let ai_panel_width = configured_panel_width(
            config.layout.ai_panel_width,
            AI_PANEL_DEFAULT_WIDTH,
            AI_PANEL_MIN_WIDTH,
            AI_PANEL_MAX_WIDTH,
        );
        let directory_col_widths =
            configured_directory_col_widths(config.layout.directory_col_widths);

        Self {
            settings_draft: config.ai.clone(),
            config,
            scan_path: String::new(),
            entries: Vec::new(),
            scan_tree: None,
            drives,
            view_mode: ViewMode::Home,
            selected: None,
            expanded_folded: BTreeSet::new(),
            sort_col: None,
            sort_phase: None,
            sidebar_width,
            ai_panel_width,
            directory_col_widths,
            scanning: false,
            scan_generation: 0,
            scan_cancel: Arc::new(AtomicBool::new(false)),
            scan_rx: None,
            scan_status: "请选择一个磁盘，或输入路径后点击「转到」".into(),
            scanned_files: 0,
            scan_started_at: None,
            show_settings: false,
            settings_test_loading: false,
            settings_test_result: String::new(),
            settings_test_rx: None,
            settings_test_started_at: None,
            ai_tab: AiPanelTab::FolderAnalysis,
            ai_analysis_output: String::new(),
            ai_cleanup_output: String::new(),
            cleanup_bat_status: String::new(),
            ai_loading: false,
            ai_generation: 0,
            ai_cancel: Arc::new(AtomicBool::new(false)),
            ai_rx: None,
            ai_started_at: None,
            pending_health_check: None,
            shell_icons: ShellIconCache::default(),
        }
    }
}

impl DriverDoctorApp {
    fn format_elapsed(start: Instant) -> String {
        let secs = start.elapsed().as_secs_f64();
        if secs >= 60.0 {
            format!("{:.1} 分钟", secs / 60.0)
        } else {
            format!("{:.1} 秒", secs)
        }
    }

    fn append_elapsed(mut text: String, start: Option<Instant>) -> String {
        if let Some(t) = start {
            text.push_str(&format!("\n\n---\n耗时: {}", Self::format_elapsed(t)));
        }
        text
    }

    fn refresh_drives(&mut self) {
        self.drives = list_drives_with_space();
    }

    fn go_home(&mut self) {
        self.view_mode = ViewMode::Home;
        self.scan_path.clear();
        self.entries.clear();
        self.scan_tree = None;
        self.selected = None;
        self.expanded_folded.clear();
        self.refresh_drives();
        self.scan_status = "请选择一个磁盘，或输入路径后点击「转到」".into();
    }

    fn navigate_to(&mut self, path: PathBuf) {
        if self.scanning {
            self.scan_cancel.store(true, Ordering::Relaxed);
        }
        self.view_mode = ViewMode::Directory;
        self.scan_path = path.display().to_string();
        self.config.last_scan_path = self.scan_path.clone();
        self.config.save();
        self.start_scan_path(path);
    }

    fn start_scan_path(&mut self, path: PathBuf) {
        self.scan_generation += 1;
        let generation = self.scan_generation;

        self.scanning = true;
        self.scan_started_at = Some(Instant::now());
        self.scan_cancel = Arc::new(AtomicBool::new(false));
        self.selected = None;
        self.expanded_folded.clear();
        self.scanned_files = 0;

        self.entries = quick_list_directory(&path).unwrap_or_default();
        self.scan_tree = None;
        self.scan_status = format!(
            "已进入 {}，正在计算大小（{} 项）...",
            path.display(),
            self.entries.len()
        );

        let cancel = self.scan_cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);

        thread::spawn(move || {
            let result = scan_directory_with_tree(&path, &cancel, |p| {
                let _ = tx.send(ScanMsg::Progress {
                    generation,
                    progress: p,
                });
            });
            let _ = tx.send(ScanMsg::Done { generation, result });
        });
    }

    fn navigate_up(&mut self) {
        let current = PathBuf::from(&self.scan_path);
        if self.view_mode == ViewMode::Directory {
            if let Some(parent) = current.parent() {
                if parent.as_os_str().is_empty() {
                    self.go_home();
                    return;
                }
                let parent_str = parent.display().to_string();
                if parent_str.len() <= 3 && parent_str.contains(':') {
                    self.go_home();
                } else {
                    self.navigate_to(parent.to_path_buf());
                }
            } else {
                self.go_home();
            }
        } else {
            self.go_home();
        }
    }

    fn start_full_scan(&mut self) {
        if self.scanning {
            self.scan_cancel.store(true, Ordering::Relaxed);
        }
        self.scan_generation += 1;
        let generation = self.scan_generation;

        self.view_mode = ViewMode::FullScan;
        self.scanning = true;
        self.scan_started_at = Some(Instant::now());
        self.scan_cancel = Arc::new(AtomicBool::new(false));
        self.entries.clear();
        self.scan_tree = None;
        self.selected = None;
        self.expanded_folded.clear();
        self.scanned_files = 0;
        self.scan_status = "正在全盘扫描各盘根目录...".into();

        let cancel = self.scan_cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);

        thread::spawn(move || {
            let result = scan_all_drives(
                &cancel,
                |p| {
                    let _ = tx.send(ScanMsg::Progress {
                        generation,
                        progress: p,
                    });
                },
                50,
            )
            .map(|entries| ScanResult {
                entries,
                tree: None,
            });
            let _ = tx.send(ScanMsg::Done { generation, result });
        });
    }

    fn cancel_scan(&mut self) {
        self.scan_cancel.store(true, Ordering::Relaxed);
        self.scan_status = "正在取消...".into();
    }

    fn poll_scan(&mut self) {
        let mut messages = Vec::new();
        if let Some(rx) = &self.scan_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }

        let mut done_generation: Option<u64> = None;
        let mut completed_ok = false;
        for msg in messages {
            match msg {
                ScanMsg::Progress {
                    generation,
                    progress,
                } if generation == self.scan_generation => {
                    self.scanned_files = progress.scanned_files;
                    self.scan_status = progress.current_path;
                }
                ScanMsg::Done { generation, result } if generation == self.scan_generation => {
                    done_generation = Some(generation);
                    let elapsed = self.scan_started_at;
                    match result {
                        Ok(scan_result) => {
                            self.entries = scan_result.entries;
                            self.scan_tree = scan_result.tree;
                            completed_ok = true;
                            let mut msg = format!("完成，共 {} 项", self.entries.len());
                            if let Some(t) = elapsed {
                                msg.push_str(&format!("，耗时 {}", Self::format_elapsed(t)));
                            }
                            self.scan_status = msg;
                        }
                        Err(e) => {
                            if e.contains("取消") {
                                // 已被新导航取代，不覆盖状态
                            } else {
                                let mut msg = e;
                                if let Some(t) = elapsed {
                                    msg.push_str(&format!("（耗时 {}）", Self::format_elapsed(t)));
                                }
                                self.scan_status = msg;
                            }
                        }
                    }
                    self.scan_started_at = None;
                }
                _ => {}
            }
        }

        if done_generation == Some(self.scan_generation) {
            self.scanning = false;
            self.scan_rx = None;
            if completed_ok {
                self.start_pending_health_check_if_ready();
            } else {
                self.pending_health_check = None;
            }
        }
    }

    fn format_size_cell(computing: bool, bytes: u64) -> String {
        if computing {
            "计算中…".into()
        } else {
            format_size(bytes)
        }
    }

    fn format_count_cell(computing: bool, count: u64) -> String {
        if computing {
            "待定".into()
        } else {
            count.to_string()
        }
    }

    fn format_percent_cell(computing: bool, percent: f64) -> String {
        if computing {
            "…".into()
        } else {
            format!("{percent:.2}%")
        }
    }

    fn compare_entries(a: &ScanEntry, b: &ScanEntry, col: SortColumn) -> std::cmp::Ordering {
        match col {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Size => a.size.cmp(&b.size),
            SortColumn::FileCount => a.file_count.cmp(&b.file_count),
            SortColumn::Percent => a
                .percent
                .partial_cmp(&b.percent)
                .unwrap_or(std::cmp::Ordering::Equal),
        }
    }

    fn sort_by_column(&mut self, col: SortColumn) {
        match (self.sort_col, self.sort_phase) {
            (Some(active), Some(SortPhase::Asc)) if active == col => {
                self.sort_phase = Some(SortPhase::Desc);
            }
            (Some(active), Some(SortPhase::Desc)) if active == col => {
                self.sort_col = None;
                self.sort_phase = None;
            }
            _ => {
                self.sort_col = Some(col);
                self.sort_phase = Some(SortPhase::Asc);
            }
        }
    }

    fn persist_layout_if_changed(
        &mut self,
        sidebar_width: f32,
        ai_panel_width: f32,
        directory_col_widths: [f32; 6],
    ) {
        let sidebar_width = clamp_panel_width(sidebar_width, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        let ai_panel_width =
            clamp_panel_width(ai_panel_width, AI_PANEL_MIN_WIDTH, AI_PANEL_MAX_WIDTH);
        let directory_col_widths = configured_directory_col_widths(Some(directory_col_widths));

        let changed = (self.sidebar_width - sidebar_width).abs() > 0.5
            || (self.ai_panel_width - ai_panel_width).abs() > 0.5
            || self
                .directory_col_widths
                .iter()
                .zip(directory_col_widths.iter())
                .any(|(a, b)| (*a - *b).abs() > 0.5);

        if changed {
            self.sidebar_width = sidebar_width;
            self.ai_panel_width = ai_panel_width;
            self.directory_col_widths = directory_col_widths;
            self.config.layout.sidebar_width = Some(sidebar_width);
            self.config.layout.ai_panel_width = Some(ai_panel_width);
            self.config.layout.directory_col_widths = Some(directory_col_widths);
            self.config.save();
        }
    }

    fn display_rows(&self) -> Vec<ScanEntry> {
        let mut rows = if self.view_mode == ViewMode::Home && !self.scanning {
            self.drives.iter().map(|d| d.to_entry()).collect()
        } else {
            self.entries.clone()
        };

        if let (Some(col), Some(phase)) = (self.sort_col, self.sort_phase) {
            rows.sort_by(|a, b| {
                let ord = Self::compare_entries(a, b, col);
                match phase {
                    SortPhase::Asc => ord,
                    SortPhase::Desc => ord.reverse(),
                }
            });
        }
        rows
    }

    fn folded_key(&self) -> String {
        match self.view_mode {
            ViewMode::Home => "home".into(),
            ViewMode::Directory => format!("dir:{}", self.scan_path),
            ViewMode::FullScan => "full-scan".into(),
        }
    }

    fn file_list_rows(&self, rows: &[ScanEntry], size_computing: bool) -> Vec<FileListRow> {
        let home = self.view_mode == ViewMode::Home && !self.scanning;
        if home || size_computing || rows.len() <= 20 {
            return rows
                .iter()
                .cloned()
                .enumerate()
                .map(|(source_index, entry)| FileListRow::Entry {
                    entry,
                    source_index,
                })
                .collect();
        }

        let compacted = compact_scan_entries(rows);
        if compacted.folded.is_empty() {
            return rows
                .iter()
                .cloned()
                .enumerate()
                .map(|(source_index, entry)| FileListRow::Entry {
                    entry,
                    source_index,
                })
                .collect();
        }

        let visible_paths: HashSet<PathBuf> = compacted
            .visible
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let folded_paths: HashSet<PathBuf> = compacted
            .folded
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let key = self.folded_key();
        let expanded = self.expanded_folded.contains(&key);
        let mut out = Vec::new();

        for (source_index, entry) in rows.iter().cloned().enumerate() {
            if visible_paths.contains(&entry.path) {
                out.push(FileListRow::Entry {
                    entry,
                    source_index,
                });
            }
        }

        out.push(FileListRow::Folded {
            key: key.clone(),
            label: folded_label(compacted.folded.len(), compacted.folded_size),
            count: compacted.folded.len(),
            size: compacted.folded_size,
        });

        if expanded {
            for (source_index, entry) in rows.iter().cloned().enumerate() {
                if folded_paths.contains(&entry.path) {
                    out.push(FileListRow::Entry {
                        entry,
                        source_index,
                    });
                }
            }
        }

        out
    }

    fn header_label(
        col: SortColumn,
        sort_col: Option<SortColumn>,
        sort_phase: Option<SortPhase>,
        text: &str,
    ) -> String {
        let suffix = if sort_col == Some(col) {
            match sort_phase {
                Some(SortPhase::Asc) => " ▲",
                Some(SortPhase::Desc) => " ▼",
                None => "",
            }
        } else {
            ""
        };
        format!("{text}{suffix}")
    }

    fn ai_output_mut(&mut self, tab: AiPanelTab) -> &mut String {
        match tab {
            AiPanelTab::FolderAnalysis => &mut self.ai_analysis_output,
            AiPanelTab::CleanupPlan => &mut self.ai_cleanup_output,
        }
    }

    fn ai_output(&self, tab: AiPanelTab) -> &str {
        match tab {
            AiPanelTab::FolderAnalysis => &self.ai_analysis_output,
            AiPanelTab::CleanupPlan => &self.ai_cleanup_output,
        }
    }

    fn ai_tab_for_target(target: AiTaskTarget) -> AiPanelTab {
        match target {
            AiTaskTarget::Analysis | AiTaskTarget::HealthCheck => AiPanelTab::FolderAnalysis,
            AiTaskTarget::Cleanup => AiPanelTab::CleanupPlan,
        }
    }

    fn begin_ai_task(
        &mut self,
        target: AiTaskTarget,
        status: String,
    ) -> (u64, Arc<AtomicBool>, mpsc::Sender<AiMsg>) {
        self.ai_generation += 1;
        let generation = self.ai_generation;
        self.ai_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.ai_cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.ai_rx = Some(rx);
        self.ai_loading = true;
        self.ai_started_at = Some(Instant::now());
        *self.ai_output_mut(Self::ai_tab_for_target(target)) = status;
        (generation, cancel, tx)
    }

    fn cancel_ai(&mut self) {
        if !self.ai_loading {
            return;
        }
        self.ai_cancel.store(true, Ordering::Relaxed);
        self.ai_loading = false;
        let elapsed = self.ai_started_at.take();
        *self.ai_output_mut(self.ai_tab) = Self::append_elapsed("分析已打断。".into(), elapsed);
    }

    fn start_ai_folder_analysis(&mut self) {
        let Some(idx) = self.selected else {
            self.ai_analysis_output = "请先在列表中选择一个文件夹或文件。".into();
            return;
        };
        if self.view_mode == ViewMode::Home {
            self.ai_analysis_output = "请先双击磁盘进入目录，再选择要分析的项目。".into();
            return;
        }
        let rows = self.display_rows();
        let Some(entry) = rows.get(idx).cloned() else {
            return;
        };
        let config = self.config.ai.clone();
        let current_path = self.scan_path.clone();
        let target = AiTaskTarget::Analysis;
        let (generation, cancel, tx) = self.begin_ai_task(target, "AI 正在分析...".into());

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(analyze_folder(
                &config,
                &entry.path,
                entry.size,
                Some(&current_path),
                None,
                cancel,
            ));
            let _ = tx.send(AiMsg::Done {
                generation,
                target,
                result,
            });
        });
    }

    fn start_ai_current_structure(&mut self) {
        if self.view_mode == ViewMode::Home || self.entries.is_empty() {
            self.ai_analysis_output = "请先进入目录并完成扫描。".into();
            return;
        }
        let root = PathBuf::from(&self.scan_path);
        let entries = self.entries.clone();
        let rag = format_current_level_rag(&root, &entries);
        let config = self.config.ai.clone();
        let target = AiTaskTarget::Analysis;
        let (generation, cancel, tx) =
            self.begin_ai_task(target, "AI 正在分析当前目录结构...".into());

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(analyze_current_structure(&config, &root, &rag, cancel));
            let _ = tx.send(AiMsg::Done {
                generation,
                target,
                result,
            });
        });
    }

    fn start_ai_deep_analyze(&mut self, entry: ScanEntry) {
        if !entry.is_dir {
            self.ai_analysis_output = "深度分析仅支持文件夹。".into();
            return;
        }
        let config = self.config.ai.clone();
        let path = entry.path.clone();
        let name = entry.name.clone();
        let target = AiTaskTarget::Analysis;
        let (generation, cancel, tx) =
            self.begin_ai_task(target, format!("正在深层扫描「{name}」的目录结构..."));

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let scan_result = build_deep_structure_rag(&path, 3, 10, &cancel, |_| {});
            let result = match scan_result {
                Ok(rag) => rt.block_on(analyze_deep_structure(&config, &path, &rag, cancel)),
                Err(_e) if cancel.load(Ordering::Relaxed) => Err("分析已打断。".into()),
                Err(e) => Err(e),
            };
            let _ = tx.send(AiMsg::Done {
                generation,
                target,
                result,
            });
        });
    }

    fn start_ai_cleanup_plan(&mut self) {
        if self.entries.is_empty() || self.view_mode == ViewMode::Home {
            self.ai_cleanup_output = "请先扫描目录或执行全盘扫描。".into();
            return;
        }
        let config = self.config.ai.clone();
        let entries = self.entries.clone();
        let current_path = self.scan_path.clone();
        let target = AiTaskTarget::Cleanup;
        let (generation, cancel, tx) = self.begin_ai_task(target, "AI 正在生成清理计划...".into());

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(generate_cleanup_plan(
                &config,
                &entries,
                Some(&current_path),
                cancel,
            ));
            let _ = tx.send(AiMsg::Done {
                generation,
                target,
                result,
            });
        });
    }

    fn start_drive_health_check(&mut self, drive: DriveInfo) {
        if self.ai_loading {
            self.ai_cancel.store(true, Ordering::Relaxed);
            self.ai_loading = false;
            self.ai_rx = None;
            self.ai_started_at = None;
        }
        self.pending_health_check = Some(drive.clone());
        self.ai_tab = AiPanelTab::FolderAnalysis;
        self.ai_analysis_output = health_check_scan_status(&drive.letter);
        self.navigate_to(drive.path.clone());
    }

    fn start_pending_health_check_if_ready(&mut self) {
        let Some(drive) = self.pending_health_check.take() else {
            return;
        };
        if PathBuf::from(&self.scan_path) != drive.path {
            return;
        }

        let config = self.config.ai.clone();
        let path = drive.path.clone();
        let label = drive.letter.clone();
        let entries = self.entries.clone();
        let scan_tree = self.scan_tree.clone();
        let target = AiTaskTarget::HealthCheck;
        let (generation, cancel, tx) =
            self.begin_ai_task(target, health_check_report_status(&label));

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let progress_tx = tx.clone();
            let result = run_interactive_health_check_from_entries(
                &rt,
                &config,
                &path,
                &entries,
                scan_tree.as_ref(),
                cancel,
                move |message| {
                    let _ = progress_tx.send(AiMsg::Progress {
                        generation,
                        target,
                        message,
                    });
                },
            );
            let _ = tx.send(AiMsg::Done {
                generation,
                target,
                result,
            });
        });
    }

    fn poll_ai(&mut self) {
        let Some(rx) = self.ai_rx.take() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(AiMsg::Progress {
                    generation,
                    target,
                    message,
                }) => {
                    if generation != self.ai_generation {
                        continue;
                    }
                    if self.ai_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    *self.ai_output_mut(Self::ai_tab_for_target(target)) = message;
                    continue;
                }
                Ok(AiMsg::Done {
                    generation,
                    target,
                    result,
                }) => {
                    if generation != self.ai_generation {
                        continue;
                    }
                    if self.ai_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    self.ai_loading = false;
                    let elapsed = self.ai_started_at.take();
                    let rendered = Self::append_elapsed(
                        match result {
                            Ok(text) => text,
                            Err(e) => format!("错误: {e}"),
                        },
                        elapsed,
                    );
                    match target {
                        AiTaskTarget::Analysis => self.ai_analysis_output = rendered,
                        AiTaskTarget::Cleanup => self.ai_cleanup_output = rendered,
                        AiTaskTarget::HealthCheck => {
                            let split = split_health_report(&rendered);
                            self.ai_analysis_output = split.analysis;
                            self.ai_cleanup_output = split.cleanup;
                        }
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.ai_rx = Some(rx);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
    }
    fn poll_settings_test(&mut self) {
        if let Some(rx) = &self.settings_test_rx {
            if let Ok(AiMsg::Done {
                generation: _,
                target: _,
                result,
            }) = rx.try_recv()
            {
                self.settings_test_loading = false;
                self.settings_test_rx = None;
                let elapsed = self.settings_test_started_at.take();
                self.settings_test_result = Self::append_elapsed(
                    match result {
                        Ok(msg) => format!("连接成功: {msg}"),
                        Err(e) => format!("连接失败: {e}"),
                    },
                    elapsed,
                );
            }
        }
    }

    fn start_settings_test(&mut self) {
        let config = self.settings_draft.clone();
        let (tx, rx) = mpsc::channel();
        self.settings_test_rx = Some(rx);
        self.settings_test_loading = true;
        self.settings_test_started_at = Some(Instant::now());
        self.settings_test_result = "正在测试连接...".into();
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(test_connection(&config));
            let _ = tx.send(AiMsg::Done {
                generation: 0,
                target: AiTaskTarget::Analysis,
                result,
            });
        });
    }

    fn open_in_explorer(&self, path: &Path) {
        let target = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let _ = open::that(&target);
    }

    fn render_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("路径:");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.scan_path)
                    .desired_width(360.0)
                    .hint_text("例如 C:\\ 或 D:\\Projects"),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if !self.scan_path.trim().is_empty() {
                    self.navigate_to(PathBuf::from(self.scan_path.trim()));
                }
            }

            if ui
                .add_enabled(
                    !self.scanning && !self.scan_path.trim().is_empty(),
                    quiet_button("转到"),
                )
                .clicked()
            {
                self.navigate_to(PathBuf::from(self.scan_path.trim()));
            }

            if ui
                .add_enabled(
                    !self.scanning && self.view_mode != ViewMode::Home,
                    quiet_button("上级"),
                )
                .clicked()
            {
                self.navigate_up();
            }

            if ui.add(quiet_button("此电脑")).clicked() {
                self.go_home();
            }

            if ui.add(quiet_button("浏览...")).clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.navigate_to(path);
                }
            }

            let full_btn = ui.add_enabled(!self.scanning, quiet_button("全盘扫描"));
            if full_btn.clicked() {
                self.start_full_scan();
            }

            if self.scanning {
                if ui.add(danger_button("取消")).clicked() {
                    self.cancel_scan();
                }
                ui.spinner();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(quiet_button("设置")).clicked() {
                    self.settings_draft = self.config.ai.clone();
                    self.show_settings = true;
                }
            });
        });

        ui.horizontal(|ui| {
            ui.label(format!("状态: {}", self.scan_status));
            if self.scanning {
                ui.label(format!("已扫描文件: {}", self.scanned_files));
            }
        });
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("目录浏览器");
        ui.add_space(6.0);

        ui.label("此电脑");
        let mut health_check: Option<DriveInfo> = None;
        ui.add_space(4.0);
        for drive in self.drives.clone() {
            let card_inner_width = (ui.available_width() - 20.0).max(0.0);
            let card = card_frame().show(ui, |ui| {
                ui.set_min_width(card_inner_width);
                ui.horizontal(|ui| {
                    self.shell_icons
                        .show(ui, &drive_icon_path(&drive.letter), true, true);
                    ui.vertical(|ui| {
                        let _response = ui.selectable_label(false, drive.label());
                        let _response = _response.on_hover_text(drive_summary_label(&drive));
                        ui.label(format!(
                            "容量 {}  可用 {}  ({:.0}% 已用)",
                            format_size(drive.total_bytes),
                            format_size(drive.free_bytes),
                            drive.used_percent()
                        ));
                    });
                });
            });
            let response = ui
                .interact(
                    card.response.rect,
                    ui.make_persistent_id(("drive_card", &drive.letter)),
                    egui::Sense::click(),
                )
                .on_hover_text(drive_summary_label(&drive));
            if response.double_clicked() {
                self.navigate_to(drive.path.clone());
            }
            let drive_for_menu = drive.clone();
            response.context_menu(|ui| {
                if ui
                    .add(health_check_button(ui, HEALTH_CHECK_LABEL))
                    .clicked()
                {
                    health_check = Some(drive_for_menu.clone());
                    ui.close_menu();
                }
            });
        }
        if let Some(drive) = health_check {
            self.start_drive_health_check(drive);
        }
    }

    fn render_file_list(&mut self, ui: &mut egui::Ui) {
        let home = self.view_mode == ViewMode::Home && !self.scanning;
        ui.horizontal(|ui| {
            ui.strong("目录详情");
            if home {
                ui.label("— 双击磁盘开始分析第一层");
            } else if self.view_mode == ViewMode::FullScan {
                ui.label("— 全盘概览");
            } else if !self.scan_path.is_empty() {
                ui.label(format!("— {} （双击文件夹进入）", self.scan_path));
            }
        });

        let rows = self.display_rows();
        let size_computing = self.scanning && self.view_mode == ViewMode::Directory;
        let table_rows = self.file_list_rows(&rows, size_computing);

        if rows.is_empty() && !self.scanning {
            ui.label("此目录为空。");
            return;
        }
        if rows.is_empty() && self.scanning {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在读取目录...");
            });
            return;
        }

        let sort_col = self.sort_col;
        let sort_phase = self.sort_phase;
        let mut sort_click: Option<SortColumn> = None;
        let selected = self.selected;
        let mut new_selected = selected;
        let mut drill_path: Option<PathBuf> = None;
        let mut open_path: Option<PathBuf> = None;
        let mut deep_analyze: Option<ScanEntry> = None;
        let mut toggle_folded: Option<String> = None;
        let expanded_keys = self.expanded_folded.clone();
        let icons = &mut self.shell_icons;
        icons.trim(512);
        let mut column_widths = self.directory_col_widths;

        let table_scroll_height = ui.available_height().max(160.0);
        let viewport_width = ui.available_width();
        let viewport_height = table_scroll_height;
        let row_height = 22.0;

        egui::ScrollArea::both()
            .id_salt("directory_table_scroll")
            .max_height(table_scroll_height)
            .auto_shrink([false, false])
            .scroll_bar_visibility(
                egui::containers::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
            )
            .show(ui, |ui| {
                let content_width = directory_table_content_width(viewport_width, &column_widths);
                let content_height = viewport_height.max(
                    DIRECTORY_TABLE_HEADER_HEIGHT + row_height * table_rows.len() as f32 + 4.0,
                );
                let (table_rect, _) = ui.allocate_exact_size(
                    egui::vec2(content_width, content_height),
                    egui::Sense::hover(),
                );
                let table_width = directory_table_total_width(&column_widths);
                let header_rect = egui::Rect::from_min_size(
                    table_rect.left_top(),
                    egui::vec2(table_width, DIRECTORY_TABLE_HEADER_HEIGHT),
                );
                let painter = ui.painter().with_clip_rect(ui.clip_rect());

                let mut x = table_rect.left();
                for (index, &(col, title)) in directory_table_columns().iter().enumerate() {
                    let width = column_widths[index];
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(x, header_rect.top()),
                        egui::vec2(width, DIRECTORY_TABLE_HEADER_HEIGHT),
                    );
                    let fill_rect = directory_header_fill_rect(rect);
                    let sort_rect =
                        directory_header_sort_rect(rect, index, directory_table_columns().len());
                    painter.rect_filled(
                        fill_rect,
                        egui::CornerRadius::same(2),
                        egui::Color32::from_rgb(226, 226, 226),
                    );
                    let label = Self::header_label(col, sort_col, sort_phase, title);
                    painter.text(
                        fill_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(14.0),
                        ui.visuals().text_color(),
                    );
                    x += width;
                    if DIRECTORY_TABLE_HEADER_RESIZE
                        && index + 1 < directory_table_columns().len()
                        && x < header_rect.right()
                    {
                        if let Some(delta) =
                            directory_column_resize_delta(ui, header_rect, x, index)
                        {
                            resize_directory_columns(&mut column_widths, index, delta);
                            ui.ctx().request_repaint();
                        }
                    }
                    if ui
                        .interact(
                            sort_rect,
                            ui.id().with(("directory-header-sort", index)),
                            egui::Sense::click(),
                        )
                        .clicked()
                    {
                        sort_click = Some(col);
                    }
                }

                let mut y = header_rect.bottom() + 4.0;
                for row_item in &table_rows {
                    let row_rect = egui::Rect::from_min_size(
                        egui::pos2(table_rect.left(), y),
                        egui::vec2(table_width, row_height),
                    );
                    match row_item {
                        FileListRow::Entry {
                            entry,
                            source_index,
                        } => {
                            let is_selected = selected == Some(*source_index);
                            if is_selected {
                                painter.rect_filled(
                                    row_rect,
                                    egui::CornerRadius::same(2),
                                    egui::Color32::from_rgb(226, 235, 246),
                                );
                            }
                            let entry_path = entry.path.clone();
                            let entry_is_dir = entry.is_dir;
                            let entry_for_menu = entry.clone();
                            let mut cell_x = table_rect.left();
                            let mut next_cell = |index: usize| {
                                let rect = egui::Rect::from_min_size(
                                    egui::pos2(cell_x, y),
                                    egui::vec2(column_widths[index], row_height),
                                );
                                cell_x += column_widths[index];
                                rect
                            };

                            table_cell_ui(ui, next_cell(0), |ui| {
                                icons.show(ui, &entry_path, entry_is_dir, home);
                                let r = ui.selectable_label(is_selected, &entry.name);
                                if r.clicked() {
                                    new_selected = Some(*source_index);
                                }
                                if r.double_clicked() {
                                    drill_path = Some(entry.path.clone());
                                }
                                r.context_menu(|ui| {
                                    if entry_for_menu.is_dir
                                        && ui.add(quiet_button("AI 深度分析")).clicked()
                                    {
                                        deep_analyze = Some(entry_for_menu.clone());
                                        ui.close_menu();
                                    }
                                    if ui.add(quiet_button("打开位置")).clicked() {
                                        open_path = Some(entry.path.clone());
                                        ui.close_menu();
                                    }
                                });
                            });
                            table_cell_ui(ui, next_cell(1), |ui| {
                                if home {
                                    ui.label(drive_capacity_cell(entry));
                                } else {
                                    ui.label(Self::format_size_cell(size_computing, entry.size));
                                }
                            });
                            table_cell_ui(ui, next_cell(2), |ui| {
                                ui.label(Self::format_percent_cell(size_computing, entry.percent));
                            });
                            table_cell_ui(ui, next_cell(3), |ui| {
                                ui.label(Self::format_count_cell(
                                    home || size_computing,
                                    entry.file_count,
                                ));
                            });
                        }
                        FileListRow::Folded {
                            key,
                            label,
                            count,
                            size,
                        } => {
                            let prefix = if expanded_keys.contains(key) {
                                "v"
                            } else {
                                ">"
                            };
                            let mut cell_x = table_rect.left();
                            let mut next_cell = |index: usize| {
                                let rect = egui::Rect::from_min_size(
                                    egui::pos2(cell_x, y),
                                    egui::vec2(column_widths[index], row_height),
                                );
                                cell_x += column_widths[index];
                                rect
                            };
                            table_cell_ui(ui, next_cell(0), |ui| {
                                let r = ui.selectable_label(false, format!("{prefix} {label}"));
                                if r.clicked() {
                                    toggle_folded = Some(key.clone());
                                }
                            });
                            table_cell_ui(ui, next_cell(1), |ui| {
                                ui.label(format_size(*size));
                            });
                            table_cell_ui(ui, next_cell(2), |ui| {
                                ui.label("-");
                            });
                            table_cell_ui(ui, next_cell(3), |ui| {
                                ui.label(count.to_string());
                            });
                        }
                    }
                    y += row_height;
                }
            });

        self.persist_layout_if_changed(self.sidebar_width, self.ai_panel_width, column_widths);

        if let Some(col) = sort_click {
            self.sort_by_column(col);
        }
        if let Some(key) = toggle_folded {
            if !self.expanded_folded.remove(&key) {
                self.expanded_folded.insert(key);
            }
        }

        self.selected = new_selected;
        if let Some(entry) = deep_analyze {
            self.start_ai_deep_analyze(entry);
        } else if let Some(path) = open_path {
            self.open_in_explorer(&path);
        } else if let Some(path) = drill_path {
            if path.is_dir() {
                self.navigate_to(path);
            } else {
                self.open_in_explorer(&path);
            }
        }
    }

    #[allow(dead_code)]
    fn render_bar_chart(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("大小分布");
            ui.label("（柱状图 — 当前目录 Top 项）");
        });
        ui.add_space(8.0);

        let home = self.view_mode == ViewMode::Home && !self.scanning;
        let rows: Vec<ScanEntry> = if home {
            self.drives.iter().map(|d| d.to_entry()).collect()
        } else {
            self.entries.clone()
        };

        if rows.is_empty() {
            ui.label("扫描完成后在此显示占比图。");
            return;
        }

        let max_size = rows.iter().map(|e| e.size).max().unwrap_or(1).max(1);
        let show = rows.iter().take(10);

        egui::ScrollArea::vertical()
            .max_height(150.0)
            .show(ui, |ui| {
                for (i, entry) in show.enumerate() {
                    ui.horizontal(|ui| {
                        self.shell_icons.show(ui, &entry.path, entry.is_dir, home);
                        ui.label(format!("{:<20}", entry.name));
                        let frac = entry.size as f32 / max_size as f32;
                        size_bar(ui, frac, bar_color(i), 280.0);
                        ui.label(format_size(entry.size));
                    });
                }
            });
    }

    fn render_ai_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.ai_tab, AiPanelTab::FolderAnalysis, "文件夹分析");
            ui.selectable_value(&mut self.ai_tab, AiPanelTab::CleanupPlan, "清理计划");
        });

        ui.add_space(8.0);

        ui.horizontal(|ui| {
            match self.ai_tab {
                AiPanelTab::FolderAnalysis => {
                    if ui
                        .add_enabled(!self.ai_loading, quiet_button("分析选中项"))
                        .clicked()
                    {
                        self.start_ai_folder_analysis();
                    }
                    if ui
                        .add_enabled(
                            !self.ai_loading
                                && self.view_mode != ViewMode::Home
                                && !self.entries.is_empty(),
                            quiet_button("分析结构"),
                        )
                        .clicked()
                    {
                        self.start_ai_current_structure();
                    }
                }
                AiPanelTab::CleanupPlan => {
                    if ui
                        .add_enabled(
                            !self.ai_loading && !self.entries.is_empty(),
                            quiet_button("清理计划"),
                        )
                        .clicked()
                    {
                        self.start_ai_cleanup_plan();
                    }
                }
            }
            if self.ai_loading {
                ui.spinner();
                if ui.add(danger_button("打断")).clicked() {
                    self.cancel_ai();
                }
            }
        });

        let active_text = self.ai_output(self.ai_tab).to_owned();
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if active_text.is_empty() {
                    render_ai_empty_state(ui, self.ai_tab);
                } else {
                    render_ai_markdown(ui, &active_text);
                    if self.ai_tab == AiPanelTab::CleanupPlan {
                        ui.add_space(12.0);
                        self.render_cleanup_bat_actions(ui, &active_text);
                    }
                }
            });
    }

    fn render_cleanup_bat_actions(&mut self, ui: &mut egui::Ui, markdown: &str) {
        let actions = cleanup_bat_actions(markdown);
        if actions.is_empty() {
            return;
        }

        ui.separator();
        ui.strong("临时 BAT 执行入口");
        ui.add_space(4.0);
        for (index, action) in actions.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{}.", index + 1));
                ui.label(&action.title);
                if let Some(path) = &action.path {
                    ui.monospace(path);
                }
                if ui.add(quiet_button("运行BAT")).clicked() {
                    self.cleanup_bat_status = match write_and_run_cleanup_bat(index, action) {
                        Ok(path) => format!("已打开临时 BAT: {}", path.display()),
                        Err(err) => format!("BAT 生成失败: {err}"),
                    };
                }
            });
        }
        if !self.cleanup_bat_status.is_empty() {
            ui.add_space(4.0);
            ui.label(&self.cleanup_bat_status);
        }
    }
    fn render_settings(&mut self, ctx: &egui::Context) {
        egui::Window::new("AI 设置")
            .collapsible(false)
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label("支持 OpenAI 兼容 API（OpenAI、DeepSeek、Ollama 等）");
                ui.add_space(8.0);

                egui::Grid::new("settings_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Base URL:");
                        ui.text_edit_singleline(&mut self.settings_draft.base_url);
                        ui.end_row();

                        ui.label("API Key:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings_draft.api_key)
                                .password(true),
                        );
                        ui.end_row();

                        ui.label("Model:");
                        ui.text_edit_singleline(&mut self.settings_draft.model);
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.label("示例:");
                ui.monospace("Base URL: https://api.deepseek.com/v1");
                ui.monospace("Model:    deepseek-chat");

                ui.add_space(10.0);
                ui.checkbox(
                    &mut self.settings_draft.web_search_enabled,
                    "启用联网搜索 / 高级请求参数",
                );
                if self.settings_draft.web_search_enabled {
                    ui.add_space(4.0);
                    ui.label(format!(
                        "识别模板: {}",
                        native_search_profile_label(&self.settings_draft.base_url)
                    ));
                    ui.label("自定义 JSON 支持 $set 与 $append；用于中转站和特殊模型参数。");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.settings_draft.custom_request_json)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(8)
                            .lock_focus(true),
                    );
                    if let Err(err) = serde_json::from_str::<serde_json::Value>(
                        &self.settings_draft.custom_request_json,
                    ) {
                        ui.label(format!("JSON 无效: {err}"));
                    }
                }

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.settings_test_loading, quiet_button("测试连接"))
                        .clicked()
                    {
                        self.start_settings_test();
                    }
                    if self.settings_test_loading {
                        ui.spinner();
                    }
                });
                if !self.settings_test_result.is_empty() {
                    ui.label(&self.settings_test_result);
                }

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.add(quiet_button("保存")).clicked() {
                        self.config.ai = self.settings_draft.clone();
                        self.config.save();
                        self.show_settings = false;
                    }
                    if ui.add(quiet_button("取消")).clicked() {
                        self.show_settings = false;
                    }
                });
            });
    }
}

impl eframe::App for DriverDoctorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan();
        self.poll_ai();
        self.poll_settings_test();

        if self.scanning || self.ai_loading || self.settings_test_loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("toolbar")
            .show_separator_line(SHOW_PANEL_SEPARATOR_LINES)
            .frame(panel_shell_frame())
            .show(ctx, |ui| {
                region_frame().show(ui, |ui| {
                    self.render_toolbar(ui);
                });
            });

        egui::CentralPanel::default()
            .frame(panel_shell_frame())
            .show(ctx, |ui| {
                let full = ui.max_rect();
                self.sidebar_width =
                    clamp_panel_width(self.sidebar_width, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                let ai_panel_max_width =
                    dynamic_ai_panel_max_width(full.width(), self.sidebar_width);
                self.ai_panel_width =
                    clamp_panel_width(self.ai_panel_width, AI_PANEL_MIN_WIDTH, ai_panel_max_width);
                let layout = main_layout_rects(full, self.sidebar_width, self.ai_panel_width);

                region_at_rect(ui, layout.sidebar, |ui| {
                    self.render_sidebar(ui);
                });
                region_at_rect(ui, layout.center, |ui| {
                    self.render_file_list(ui);
                });
                region_at_rect(ui, layout.ai, |ui| {
                    ui.heading("AI 助手");
                    ui.add_space(8.0);
                    self.render_ai_panel(ui);
                });

                if let Some(delta) =
                    vertical_resize_delta(ui, layout.sidebar_center_gap, "sidebar-center-resize")
                {
                    self.sidebar_width =
                        (self.sidebar_width + delta).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
                    ui.ctx().request_repaint();
                }
                if let Some(delta) =
                    vertical_resize_delta(ui, layout.center_ai_gap, "center-ai-resize")
                {
                    self.ai_panel_width =
                        (self.ai_panel_width - delta).clamp(AI_PANEL_MIN_WIDTH, ai_panel_max_width);
                    ui.ctx().request_repaint();
                }
            });

        self.persist_layout_if_changed(
            self.sidebar_width,
            self.ai_panel_width,
            self.directory_col_widths,
        );

        if self.show_settings {
            self.render_settings(ctx);
        }
    }
}

struct HealthReportSplit {
    analysis: String,
    cleanup: String,
}

#[derive(Clone, Debug)]
struct CleanupBatAction {
    title: String,
    path: Option<String>,
    script: String,
}

fn split_health_report(report: &str) -> HealthReportSplit {
    let cleanup_start = find_cleanup_section_start(report);

    if let Some(index) = cleanup_start {
        let (analysis, cleanup) = report.split_at(index);
        HealthReportSplit {
            analysis: analysis.trim().to_string(),
            cleanup: cleanup.trim().to_string(),
        }
    } else {
        HealthReportSplit {
            analysis: report.trim().to_string(),
            cleanup: String::new(),
        }
    }
}

fn find_cleanup_section_start(report: &str) -> Option<usize> {
    let markers = [
        "## 清理意见报告",
        "## 清理计划",
        "## 清理意见",
        "## Cleanup",
        "\nCleanup",
        "\nPriority",
        "\n清理意见报告",
        "\n清理计划",
        "\n清理意见",
        "\n执行优先级",
    ];
    markers
        .iter()
        .filter_map(|marker| report.find(marker))
        .min()
}

fn render_ai_empty_state(ui: &mut egui::Ui, tab: AiPanelTab) {
    match tab {
        AiPanelTab::FolderAnalysis => {
            ui.label("配置 AI 后，可分析目录用途并获取清理建议。");
            ui.add_space(8.0);
            ui.label("• 分析当前目录结构：基于列表生成 RAG 文档后分析");
            ui.label("• 右键文件夹 → AI 深度分析：多层级扫描");
            ui.label("• 分析选中项：单个文件夹快速说明");
        }
        AiPanelTab::CleanupPlan => {
            ui.label("生成清理计划后，这里会显示清理建议和可执行 BAT 入口。");
            ui.add_space(8.0);
            ui.label("• 全盘扫描后 → 生成清理计划");
            ui.label("• 盘符健康检查后 → 自动拆分清理意见到这里");
        }
    }
}

fn render_ai_markdown(ui: &mut egui::Ui, markdown: &str) {
    let mut in_code = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(6.0);
            continue;
        }
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            wrapped_monospace(ui, line);
            continue;
        }
        if let Some(text) = trimmed.strip_prefix("### ") {
            wrapped_strong(ui, &clean_markdown_inline(text));
        } else if let Some(text) = trimmed.strip_prefix("## ") {
            ui.heading(clean_markdown_inline(text));
        } else if let Some(text) = trimmed.strip_prefix("# ") {
            ui.heading(clean_markdown_inline(text));
        } else if is_markdown_table_separator(trimmed) {
            continue;
        } else if is_markdown_table_row(trimmed) {
            render_markdown_table_row(ui, trimmed);
        } else if let Some(text) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ui.horizontal_wrapped(|ui| {
                ui.label("•");
                wrapped_label(ui, &clean_markdown_inline(text));
            });
        } else {
            wrapped_label(ui, &clean_markdown_inline(trimmed));
        }
    }
}

fn wrapped_label(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(text).wrap());
}

fn wrapped_strong(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).strong()).wrap());
}

fn wrapped_monospace(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
}

fn clean_markdown_inline(text: &str) -> String {
    text.replace("**", "")
        .replace('`', "")
        .replace("<br>", " ")
        .replace("<br/>", " ")
}

fn is_markdown_table_row(line: &str) -> bool {
    line.starts_with('|') && line.ends_with('|') && line.matches('|').count() >= 2
}

fn is_markdown_table_separator(line: &str) -> bool {
    is_markdown_table_row(line)
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn render_markdown_table_row(ui: &mut egui::Ui, line: &str) {
    let cells: Vec<String> = line
        .trim_matches('|')
        .split('|')
        .map(|cell| clean_markdown_inline(cell.trim()))
        .collect();
    let cell_max_width = (ui.available_width() - 24.0).clamp(180.0, 420.0);
    ui.horizontal_wrapped(|ui| {
        for cell in cells {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(238, 242, 247))
                .corner_radius(egui::CornerRadius::same(3))
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.set_max_width(cell_max_width);
                    wrapped_label(ui, &cell);
                });
        }
    });
}

fn cleanup_bat_actions(markdown: &str) -> Vec<CleanupBatAction> {
    let mut actions = Vec::new();
    for line in markdown.lines() {
        if actions.len() >= 12 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || is_markdown_table_separator(trimmed) {
            continue;
        }
        let candidate = trimmed.starts_with('|')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with(|c: char| c.is_ascii_digit());
        if !candidate {
            continue;
        }
        let title = cleanup_action_title(trimmed);
        if title.is_empty() {
            continue;
        }
        let path = extract_windows_path(trimmed);
        let script = build_cleanup_bat_script(&title, path.as_deref(), trimmed);
        actions.push(CleanupBatAction {
            title,
            path,
            script,
        });
    }
    actions
}

fn cleanup_action_title(line: &str) -> String {
    let cleaned = clean_markdown_inline(
        line.trim()
            .trim_matches('|')
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim(),
    );
    cleaned
        .split('|')
        .map(str::trim)
        .find(|cell| !cell.is_empty())
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

fn extract_windows_path(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    for index in 0..bytes.len().saturating_sub(2) {
        if bytes[index].is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && bytes[index + 2] == b'\\'
        {
            let mut end = index + 3;
            while end < bytes.len() {
                let ch = line[end..].chars().next()?;
                if matches!(ch, '|' | '`' | '"' | '\r' | '\n') {
                    break;
                }
                end += ch.len_utf8();
            }
            return Some(line[index..end].trim().trim_end_matches('\\').to_string());
        }
    }
    None
}

fn build_cleanup_bat_script(title: &str, path: Option<&str>, source_line: &str) -> String {
    let escaped_title = title.replace('"', "'");
    let escaped_source = source_line.replace('"', "'");
    let mut script = String::from("@echo off\r\nchcp 65001 >nul\r\n");
    script.push_str("title Driver Doctor Cleanup Helper\r\n");
    script.push_str("echo Driver Doctor 临时清理助手\r\n");
    script.push_str(&format!("echo 计划: {escaped_title}\r\n"));
    script.push_str(&format!("echo 来源: {escaped_source}\r\n"));
    if let Some(path) = path {
        script.push_str(&format!("set \"TARGET={path}\"\r\n"));
        script.push_str("echo 目标路径: %TARGET%\r\n");
        script.push_str("if exist \"%TARGET%\" (\r\n");
        script.push_str("  start \"\" \"%TARGET%\"\r\n");
        script.push_str(") else (\r\n");
        script.push_str("  echo 目标路径不存在或无权限访问。\r\n");
        script.push_str(")\r\n");
    }
    script.push_str("echo.\r\n");
    script.push_str("echo 为避免误删，本脚本默认打开目标位置并展示建议，不静默删除文件。\r\n");
    script.push_str("echo 请先确认软件内迁移、自动清理或关闭生成选项，再手动清理。\r\n");
    script.push_str("pause\r\n");
    script
}

fn write_and_run_cleanup_bat(index: usize, action: &CleanupBatAction) -> Result<PathBuf, String> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "driver_doctor_cleanup_{}_{}.bat",
        index + 1,
        sanitize_bat_name(&action.title)
    ));
    fs::write(&path, &action.script).map_err(|e| e.to_string())?;
    Command::new("cmd")
        .args(["/C", "start", "", &path.display().to_string()])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(path)
}

fn sanitize_bat_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(32)
        .collect();
    if out.is_empty() {
        out = "plan".into();
    }
    out
}

fn run_interactive_health_check_from_entries(
    rt: &tokio::runtime::Runtime,
    config: &AiConfig,
    root: &Path,
    entries: &[ScanEntry],
    scan_tree: Option<&ScanNode>,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(String),
) -> Result<String, String> {
    let mut rag = scan_tree
        .map(|tree| format_scan_tree_diagnostic_rag(tree, "health_check_full_tree"))
        .unwrap_or_else(|| format_diagnostic_tree_rag(root, &entries, "health_check_root"));
    let mut scanned = std::collections::BTreeSet::new();
    scanned.insert(root.to_path_buf());
    progress(format!(
        "已完成第一层扫描，共 {} 项。\n正在准备健康检查数据...",
        entries.len()
    ));
    write_health_debug_log(format!(
        "start root=\"{}\" entries={} rag_chars={}",
        root.display(),
        entries.len(),
        rag.chars().count()
    ));

    if scan_tree.is_none() {
        let mut initial_scope_budget = HEALTH_CHECK_INITIAL_SCOPE_BUDGET;
        append_initial_health_check_scopes(
            &mut rag,
            root,
            entries,
            1,
            &mut initial_scope_budget,
            &mut scanned,
            &cancel,
            &mut progress,
        )?;
    } else {
        write_health_debug_log(format!(
            "full_tree_rag_ready root=\"{}\" rag_chars={}",
            root.display(),
            rag.chars().count()
        ));
    }

    for round in 1..=3 {
        if cancel.load(Ordering::Relaxed) {
            write_health_debug_log(format!(
                "cancelled round={round} root=\"{}\"",
                root.display()
            ));
            return Err("分析已打断".into());
        }
        let force_report = round == 3;
        progress(health_check_ai_round_status(round, force_report));
        write_health_debug_log(format!(
            "request round={round} force_report={force_report} rag_chars={}",
            rag.chars().count()
        ));
        let reply = rt.block_on(generate_health_check_step(
            config,
            root,
            &rag,
            round,
            force_report,
            cancel.clone(),
        ));
        let reply = match reply {
            Ok(reply) => reply,
            Err(err) => {
                write_health_debug_log(format!("error round={round} message=\"{}\"", err));
                return Err(err);
            }
        };
        match reply {
            HealthCheckReply::Report(report) => {
                write_health_debug_log(format!(
                    "report round={round} chars={}",
                    report.chars().count()
                ));
                return finalize_health_check_report(
                    rt,
                    config,
                    root,
                    report,
                    cancel.clone(),
                    &mut progress,
                );
            }
            HealthCheckReply::NeedPaths(paths) if force_report => {
                write_health_debug_log(format!(
                    "need_paths_ignored_on_force round={round} count={}",
                    paths.len()
                ));
                let _ = paths;
                continue;
            }
            HealthCheckReply::NeedPaths(paths) => {
                write_health_debug_log(format!("need_paths round={round} count={}", paths.len()));
                progress(format!(
                    "AI 请求继续深挖 {} 个路径，正在准备扫描...",
                    paths.len()
                ));
                let mut appended = 0;
                let total = paths.len().min(6);
                for (index, requested) in paths.into_iter().take(6).enumerate() {
                    if appended >= 6 {
                        break;
                    }
                    let child = PathBuf::from(requested);
                    if !child.starts_with(root) || !child.is_dir() || scanned.contains(&child) {
                        write_health_debug_log(format!(
                            "skip_child_invalid round={round} path=\"{}\"",
                            child.display()
                        ));
                        continue;
                    }
                    progress(health_check_deep_scan_status(
                        round,
                        index + 1,
                        total,
                        &child,
                    ));
                    write_health_debug_log(format!(
                        "scan_child_start round={round} path=\"{}\"",
                        child.display()
                    ));
                    let child_entries = match scan_directory(&child, &cancel, |_| {}) {
                        Ok(entries) => entries,
                        Err(err) => {
                            append_skipped_health_check_scope(&mut rag, round, &child, &err);
                            write_health_debug_log(format!(
                                "skip_child round={round} path=\"{}\" err=\"{}\"",
                                child.display(),
                                err
                            ));
                            scanned.insert(child);
                            appended += 1;
                            continue;
                        }
                    };
                    write_health_debug_log(format!(
                        "scan_child_done round={round} path=\"{}\" entries={}",
                        child.display(),
                        child_entries.len()
                    ));
                    append_health_check_scope(
                        &mut rag,
                        "health_check_requested_child",
                        round,
                        &child,
                        &child_entries,
                    );
                    scanned.insert(child);
                    appended += 1;
                    progress(format!(
                        "已追加第 {appended}/{total} 个深挖路径。\n正在准备下一轮 AI 判断..."
                    ));
                }
                if appended == 0 {
                    write_health_debug_log(format!(
                        "no_paths_appended round={round}; forcing report"
                    ));
                    progress("AI 请求的深挖路径无可扫描项，正在强制生成报告...".into());
                    let forced = rt.block_on(generate_health_check_step(
                        config,
                        root,
                        &rag,
                        round + 1,
                        true,
                        cancel.clone(),
                    ));
                    let forced = match forced {
                        Ok(reply) => reply.into_report(),
                        Err(err) => Err(err),
                    };
                    if let Err(err) = &forced {
                        write_health_debug_log(format!(
                            "forced_report_error round={round} message=\"{}\"",
                            err
                        ));
                    }
                    return match forced {
                        Ok(report) => finalize_health_check_report(
                            rt,
                            config,
                            root,
                            report,
                            cancel.clone(),
                            &mut progress,
                        ),
                        Err(err) => Err(err),
                    };
                }
            }
        }
    }

    write_health_debug_log(format!(
        "final_force round=4 rag_chars={}",
        rag.chars().count()
    ));
    progress("正在请求最终健康检查报告...".into());
    let final_reply = rt.block_on(generate_health_check_step(
        config,
        root,
        &rag,
        4,
        true,
        cancel.clone(),
    ));
    let final_reply = match final_reply {
        Ok(reply) => reply.into_report(),
        Err(err) => Err(err),
    };
    if let Err(err) = &final_reply {
        write_health_debug_log(format!("final_error message=\"{}\"", err));
    }
    match final_reply {
        Ok(report) => finalize_health_check_report(rt, config, root, report, cancel, &mut progress),
        Err(err) => Err(err),
    }
}

fn append_skipped_health_check_scope(rag: &mut String, round: u32, path: &Path, reason: &str) {
    rag.push_str(&format!(
        "\n\n## skipped_child_scope round={} path=\"{}\"\nreason=\"{}\"\n",
        round,
        path.display(),
        reason.replace('"', "'")
    ));
}

fn finalize_health_check_report(
    rt: &tokio::runtime::Runtime,
    config: &AiConfig,
    root: &Path,
    report: String,
    cancel: Arc<AtomicBool>,
    progress: &mut impl FnMut(String),
) -> Result<String, String> {
    progress("正在基于健康检查报告生成清理计划...".into());
    write_health_debug_log(format!(
        "cleanup_plan_request root=\"{}\" report_chars={}",
        root.display(),
        report.chars().count()
    ));
    let cleanup = rt.block_on(generate_cleanup_plan_from_health_report(
        config, root, &report, cancel,
    ));

    match cleanup {
        Ok(cleanup) => {
            write_health_debug_log(format!(
                "cleanup_plan_done root=\"{}\" chars={}",
                root.display(),
                cleanup.chars().count()
            ));
            Ok(format!("{report}\n\n{cleanup}"))
        }
        Err(err) => {
            write_health_debug_log(format!(
                "cleanup_plan_error root=\"{}\" message=\"{}\"",
                root.display(),
                err
            ));
            Ok(format!(
                "{report}\n\n## 清理计划\n清理计划生成失败：{err}\n\n## 执行说明\n请先查看占用报告，确认路径后再手动处理。"
            ))
        }
    }
}

fn append_health_check_scope(
    rag: &mut String,
    scan_mode: &str,
    round: u32,
    path: &Path,
    entries: &[ScanEntry],
) {
    rag.push_str(&format!(
        "\n\n## {scan_mode} round={} path=\"{}\"\n\n",
        round,
        path.display()
    ));
    rag.push_str(&format_diagnostic_tree_rag(path, entries, scan_mode));
}

fn append_initial_health_check_scopes(
    rag: &mut String,
    root: &Path,
    entries: &[ScanEntry],
    depth: u32,
    scope_budget: &mut usize,
    scanned: &mut std::collections::BTreeSet<PathBuf>,
    cancel: &Arc<AtomicBool>,
    progress: &mut impl FnMut(String),
) -> Result<(), String> {
    if depth > HEALTH_CHECK_INITIAL_MAX_DEPTH || *scope_budget == 0 {
        return Ok(());
    }

    let targets = health_check_initial_deep_targets(entries);
    let total = targets.len();
    for (index, child) in targets.into_iter().enumerate() {
        if *scope_budget == 0 || cancel.load(Ordering::Relaxed) {
            break;
        }
        if !child.starts_with(root) || !child.is_dir() || scanned.contains(&child) {
            write_health_debug_log(format!(
                "initial_deep_scan_skip depth={depth} path=\"{}\"",
                child.display()
            ));
            continue;
        }

        progress(format!(
            "正在递归生成诊断树，第 {depth} 层 {}/{}：{}",
            index + 1,
            total,
            child.display()
        ));
        write_health_debug_log(format!(
            "initial_deep_scan_start depth={depth} path=\"{}\"",
            child.display()
        ));

        match scan_directory(&child, cancel, |_| {}) {
            Ok(child_entries) => {
                *scope_budget = scope_budget.saturating_sub(1);
                append_health_check_scope(
                    rag,
                    "health_check_initial_child",
                    depth,
                    &child,
                    &child_entries,
                );
                scanned.insert(child.clone());
                write_health_debug_log(format!(
                    "initial_deep_scan_done depth={depth} path=\"{}\" entries={} rag_chars={} remaining_budget={} scan_mode=\"health_check_initial_child\"",
                    child.display(),
                    child_entries.len(),
                    rag.chars().count(),
                    *scope_budget
                ));
                append_initial_health_check_scopes(
                    rag,
                    root,
                    &child_entries,
                    depth + 1,
                    scope_budget,
                    scanned,
                    cancel,
                    progress,
                )?;
            }
            Err(err) => {
                *scope_budget = scope_budget.saturating_sub(1);
                append_skipped_health_check_scope(rag, depth, &child, &err);
                scanned.insert(child.clone());
                write_health_debug_log(format!(
                    "initial_deep_scan_error depth={depth} path=\"{}\" err=\"{}\" remaining_budget={}",
                    child.display(),
                    err,
                    *scope_budget
                ));
            }
        }
    }

    Ok(())
}

impl HealthCheckReply {
    fn into_report(self) -> Result<String, String> {
        match self {
            HealthCheckReply::Report(report) if report.trim().is_empty() => {
                Err("健康检查 REPORT 内容为空".into())
            }
            HealthCheckReply::Report(report) => Ok(report),
            HealthCheckReply::NeedPaths(_) => Err("AI 未在最终轮次返回报告".into()),
        }
    }
}

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 720.0])
            .with_title("Driver Doctor — 磁盘空间分析"),
        ..Default::default()
    };

    eframe::run_native(
        "Driver Doctor",
        options,
        Box::new(|cc| {
            setup_cjk_fonts(&cc.egui_ctx);
            configure_app_style(&cc.egui_ctx);
            Ok(Box::new(DriverDoctorApp::default()))
        }),
    )
}

fn configure_app_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.visuals.panel_fill = app_bg_color();
    style.visuals.faint_bg_color = egui::Color32::from_rgb(238, 243, 249);
    style.visuals.extreme_bg_color = app_bg_color();
    style.visuals.window_stroke = egui::Stroke::NONE;
    style.visuals.selection.stroke = egui::Stroke::NONE;
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.noninteractive.expansion = 0.0;
    style.visuals.widgets.inactive.expansion = 0.0;
    style.visuals.widgets.hovered.expansion = 0.0;
    style.visuals.widgets.active.expansion = 0.0;
    style.visuals.widgets.open.expansion = 0.0;
    style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(CARD_RADIUS);
    style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(CARD_RADIUS);
    style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(CARD_RADIUS);
    style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(CARD_RADIUS);
    style.visuals.widgets.open.corner_radius = egui::CornerRadius::same(CARD_RADIUS);
    ctx.set_style(style);
}

/// 加载 Windows 系统中文字体，解决中文显示为方块的问题。
fn setup_cjk_fonts(ctx: &egui::Context) {
    const FONT_CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ];

    let Some(bytes) = FONT_CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };

    let mut font_data = egui::FontData::from_owned(bytes);
    font_data.index = 0;

    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".into(), std::sync::Arc::new(font_data));

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "cjk".into());
    }

    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_check_label_does_not_use_emoji_glyph() {
        assert_eq!(HEALTH_CHECK_LABEL, "健康检查");
        assert!(!HEALTH_CHECK_LABEL.contains('🩺'));
    }

    #[test]
    fn health_check_menu_uses_single_image_text_button() {
        let source = include_str!("app.rs");
        let sidebar_start = source.find("fn render_sidebar(").unwrap();
        let sidebar_end = source[sidebar_start..]
            .find("fn render_file_list(")
            .unwrap();
        let sidebar_source = &source[sidebar_start..sidebar_start + sidebar_end];

        assert!(sidebar_source.contains("health_check_button(ui, HEALTH_CHECK_LABEL)"));
        assert!(!sidebar_source.contains("health_check_icon(ui);"));
        assert!(!sidebar_source.contains("quiet_button(HEALTH_CHECK_LABEL)"));
    }

    #[test]
    fn drive_summary_label_merges_capacity_free_and_used_percent() {
        let drive = DriveInfo {
            letter: "C:".into(),
            path: PathBuf::from(r"C:\"),
            total_bytes: 100,
            free_bytes: 25,
        };

        assert_eq!(
            drive_summary_label(&drive),
            "本地磁盘 (C:)  容量 100 B  可用 25 B  (75% 已用)"
        );
    }

    #[test]
    fn drive_home_capacity_cell_uses_used_over_total() {
        let drive = DriveInfo {
            letter: "C:".into(),
            path: PathBuf::from(r"C:\"),
            total_bytes: 100,
            free_bytes: 25,
        };
        let entry = drive.to_entry();

        assert_eq!(entry.size, 75);
        assert_eq!(entry.allocated, 100);
        assert_eq!(drive_capacity_cell(&entry), "75 B/100 B");
    }

    #[test]
    fn skipped_health_check_scope_is_recorded_for_ai_context() {
        let mut rag = String::from("root");

        append_skipped_health_check_scope(
            &mut rag,
            2,
            Path::new(r"C:\System Volume Information"),
            "access denied: \"os error 5\"",
        );

        assert!(rag.contains("skipped_child_scope round=2"));
        assert!(rag.contains(r#"path="C:\System Volume Information""#));
        assert!(rag.contains("access denied: 'os error 5'"));
    }

    #[test]
    fn health_check_deep_scan_progress_is_visible_and_logged() {
        let status = health_check_deep_scan_status(2, 3, 6, Path::new(r"C:\Users"));

        assert!(status.contains("第 3/6 个"));
        assert!(status.contains("第 2 轮"));
        assert!(status.contains(r"C:\Users"));

        let source = include_str!("app.rs");
        assert!(source.contains("AiMsg::Progress"));
        assert!(source.contains("scan_child_start"));
        assert!(source.contains("scan_child_done"));
    }

    #[test]
    fn health_check_status_text_is_readable_chinese() {
        assert_eq!(
            health_check_scan_status("C:"),
            "正在扫描 C:，完成后生成健康检查报告..."
        );
        assert_eq!(
            health_check_report_status("C:"),
            "正在基于 C: 扫描结果生成健康检查报告..."
        );
        assert!(!health_check_scan_status("C:").contains("姝"));
        assert!(!health_check_report_status("C:").contains("濮"));
    }

    #[test]
    fn resizing_directory_columns_grows_left_and_shrinks_right() {
        let mut widths = DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS;

        resize_directory_columns(&mut widths, 0, 10.0);

        assert_eq!(widths[0], DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS[0] + 10.0);
        assert_eq!(widths[1], DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS[1] - 10.0);
    }

    #[test]
    fn directory_header_sort_rect_leaves_resize_gutter_free() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(120.0, 22.0));

        let first = directory_header_sort_rect(rect, 0, 4);
        let middle = directory_header_sort_rect(rect, 2, 4);
        let last = directory_header_sort_rect(rect, 3, 4);

        assert!(first.max.x <= rect.max.x - DIRECTORY_HEADER_RESIZE_HANDLE_WIDTH / 2.0);
        assert!(middle.min.x >= rect.min.x + DIRECTORY_HEADER_RESIZE_HANDLE_WIDTH / 2.0);
        assert!(middle.max.x <= rect.max.x - DIRECTORY_HEADER_RESIZE_HANDLE_WIDTH / 2.0);
        assert!(last.min.x >= rect.min.x + DIRECTORY_HEADER_RESIZE_HANDLE_WIDTH / 2.0);
    }

    #[test]
    fn health_report_is_split_between_analysis_and_cleanup_tabs() {
        let report =
            "## 占用报告\nUsers 很大\n\n## 清理意见报告\n清理 Users 缓存\n\n## 执行优先级\n先迁移";
        let split = split_health_report(report);

        assert!(split.analysis.contains("## 占用报告"));
        assert!(!split.analysis.contains("清理 Users 缓存"));
        assert!(split.cleanup.contains("## 清理意见报告"));
        assert!(split.cleanup.contains("## 执行优先级"));
    }

    #[test]
    fn health_report_split_accepts_plain_cleanup_marker() {
        let report = "Usage report\nUsers large\n\nCleanup\nMove caches\n\nPriority\nMove first";
        let split = split_health_report(report);

        assert!(split.analysis.contains("Usage report"));
        assert!(!split.analysis.contains("Move caches"));
        assert!(split.cleanup.contains("Cleanup"));
    }

    #[test]
    fn ai_panel_width_and_output_scroll_are_not_hard_clipped() {
        assert!(AI_PANEL_MAX_WIDTH >= 900.0);
        assert!(dynamic_ai_panel_max_width(1580.0, 220.0) > 560.0);
        assert_eq!(dynamic_ai_panel_max_width(880.0, 220.0), AI_PANEL_MIN_WIDTH);

        let source = include_str!("app.rs");
        let ai_start = source.find("fn render_ai_panel(").unwrap();
        let ai_end = source[ai_start..]
            .find("fn render_cleanup_bat_actions")
            .unwrap();
        let ai_source = &source[ai_start..ai_start + ai_end];

        assert!(ai_source.contains("egui::ScrollArea::both()"));
        assert!(source.contains("dynamic_ai_panel_max_width(full.width(), self.sidebar_width)"));
    }

    #[test]
    fn health_check_report_runs_second_cleanup_plan_step() {
        let source = include_str!("app.rs");
        let health_start = source
            .find("fn run_interactive_health_check_from_entries(")
            .unwrap();
        let health_end = source[health_start..]
            .find("fn append_skipped_health_check_scope")
            .unwrap();
        let health_source = &source[health_start..health_start + health_end];

        assert!(health_source.contains("finalize_health_check_report("));
        assert!(source.contains("generate_cleanup_plan_from_health_report"));
        assert!(source.contains("cleanup_plan_request"));
        assert!(source.contains("cleanup_plan_done"));
    }

    #[test]
    fn health_check_auto_deepens_top_large_directories_before_ai_report() {
        let source = include_str!("app.rs");
        let health_start = source
            .find("fn run_interactive_health_check_from_entries(")
            .unwrap();
        let health_end = source[health_start..]
            .find("fn append_skipped_health_check_scope")
            .unwrap();
        let health_source = &source[health_start..health_start + health_end];

        assert!(source.contains("fn health_check_initial_deep_targets("));
        assert!(source.contains("fn append_health_check_scope("));
        assert!(source.contains("fn append_initial_health_check_scopes("));
        assert!(source.contains("HEALTH_CHECK_INITIAL_SCOPE_BUDGET"));
        assert!(source.contains("HEALTH_CHECK_INITIAL_MAX_DEPTH"));
        assert!(health_source.contains("append_initial_health_check_scopes("));
        assert!(source.contains("initial_deep_scan_done"));
        assert!(source.contains("scan_mode=\\\"health_check_initial_child\\\""));
        assert!(
            health_source
                .find("append_initial_health_check_scopes(")
                .unwrap()
                < health_source.find("request round={round}").unwrap()
        );
    }

    #[test]
    fn scan_result_tree_is_cached_and_used_by_health_check() {
        let source = include_str!("app.rs");

        assert!(source.contains("scan_tree: Option<ScanNode>"));
        assert!(source.contains("self.scan_tree = scan_result.tree"));
        assert!(source.contains("let scan_tree = self.scan_tree.clone();"));
        assert!(source.contains("scan_tree.as_ref()"));
        assert!(
            source.contains("format_scan_tree_diagnostic_rag(tree, \"health_check_full_tree\")")
        );
        assert!(source.contains("full_tree_rag_ready"));
    }

    #[test]
    fn cleanup_bat_actions_extract_paths_from_table_rows() {
        let plan = "| C:\\Users\\Admin\\AppData\\Local\\Temp | 5 GB | 可清理 |\n| C:\\Program Files\\Docker | 3 GB | 设置迁移 |";
        let actions = cleanup_bat_actions(plan);

        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[0].path.as_deref(),
            Some(r"C:\Users\Admin\AppData\Local\Temp")
        );
        assert!(actions[0]
            .script
            .contains(r#"set "TARGET=C:\Users\Admin\AppData\Local\Temp""#));
    }

    #[test]
    fn card_and_button_style_constants_stay_stable() {
        assert!(CARD_RADIUS <= 8);
        assert_eq!(BUTTON_WIDTH, 72.0);
        assert_eq!(BUTTON_HEIGHT, 24.0);
        assert!(DRIVE_CARD_GAP >= 8.0);
        assert_eq!(REGION_GAP, 8);
        assert_eq!(REGION_INNER_MARGIN, 8);
        assert!(!SHOW_PANEL_SEPARATOR_LINES);
        assert_eq!(app_bg_color(), egui::Color32::from_rgb(232, 238, 246));
        assert!(SIDEBAR_MIN_WIDTH < SIDEBAR_DEFAULT_WIDTH);
        assert!(SIDEBAR_DEFAULT_WIDTH < SIDEBAR_MAX_WIDTH);
        assert!(AI_PANEL_MIN_WIDTH < AI_PANEL_DEFAULT_WIDTH);
        assert!(AI_PANEL_DEFAULT_WIDTH < AI_PANEL_MAX_WIDTH);
        assert_eq!(
            configured_panel_width(
                Some(999.0),
                SIDEBAR_DEFAULT_WIDTH,
                SIDEBAR_MIN_WIDTH,
                SIDEBAR_MAX_WIDTH
            ),
            SIDEBAR_MAX_WIDTH
        );
    }

    #[test]
    fn directory_table_columns_use_compact_localized_layout() {
        let labels: Vec<_> = directory_table_columns()
            .iter()
            .map(|(_, title)| *title)
            .collect();

        assert_eq!(labels, ["名称", "占用", "占比", "文件"]);
        assert!(!labels
            .iter()
            .any(|label| matches!(*label, "Modified" | "Accessed" | "Owner" | "文件夹")));
        assert!(!SHOW_RESIZE_DEBUG_LINES);
        assert!(DIRECTORY_TABLE_HEADER_RESIZE);
        assert_eq!(DIRECTORY_TABLE_HEADER_HEIGHT, 22.0);
        assert!(DIRECTORY_TABLE_COLUMN_MIN_WIDTHS.len() >= labels.len());
        assert!(DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS.len() >= labels.len());
        assert!(directory_table_total_width(&DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS) <= 520.0);
        assert_eq!(
            directory_table_content_width(900.0, &DIRECTORY_TABLE_COLUMN_DEFAULT_WIDTHS),
            900.0
        );
        assert_eq!(
            clamp_directory_column_width(0, 1.0),
            DIRECTORY_TABLE_COLUMN_MIN_WIDTHS[0]
        );
        assert_eq!(
            configured_directory_col_widths(Some([1.0; 6]))[0],
            DIRECTORY_TABLE_COLUMN_MIN_WIDTHS[0]
        );
        assert_eq!(DriverDoctorApp::format_count_cell(true, 0), "待定");
    }

    #[test]
    fn main_layout_uses_manual_gap_resize_rects() {
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 640.0));
        let layout = main_layout_rects(full, 220.0, 340.0);

        assert_eq!(layout.sidebar.top(), layout.center.top());
        assert_eq!(layout.center.top(), layout.ai.top());
        assert_eq!(layout.sidebar.bottom(), layout.center.bottom());
        assert_eq!(layout.center.bottom(), layout.ai.bottom());
        assert_eq!(layout.sidebar_center_gap.left(), layout.sidebar.right());
        assert_eq!(layout.center_ai_gap.right(), layout.ai.left());
        assert_eq!(layout.sidebar_center_gap.width(), REGION_GAP as f32);
        assert_eq!(layout.center_ai_gap.width(), REGION_GAP as f32);
        assert_eq!(layout.sidebar.left(), full.left() + REGION_GAP as f32);
        assert_eq!(layout.ai.right(), full.right() - REGION_GAP as f32);
        assert_eq!(layout.sidebar.bottom(), full.bottom() - REGION_GAP as f32);
        assert_eq!(layout.center.bottom(), full.bottom() - REGION_GAP as f32);
        assert_eq!(layout.ai.bottom(), full.bottom() - REGION_GAP as f32);
        assert_eq!(
            layout.center.left() - layout.sidebar.right(),
            REGION_GAP as f32
        );
        assert_eq!(layout.ai.left() - layout.center.right(), REGION_GAP as f32);

        let source = include_str!("app.rs");
        let update_start = source.find("fn update(").unwrap();
        let update_end = source[update_start..]
            .find("struct HealthReportSplit")
            .unwrap();
        let update_source = &source[update_start..update_start + update_end];
        assert!(!update_source.contains("SidePanel::left"));
        assert!(!update_source.contains("SidePanel::right"));
        assert!(update_source.contains("layout.sidebar_center_gap"));
        assert!(update_source.contains("layout.center_ai_gap"));
    }

    #[test]
    fn directory_table_column_resize_uses_parent_boundary_handles() {
        let source = include_str!("app.rs");
        let table_start = source.find("fn render_file_list(").unwrap();
        let table_end = source[table_start..].find("#[allow(dead_code)]").unwrap();
        let table_source = &source[table_start..table_start + table_end];

        assert!(!table_source.contains("TableBuilder::new"));
        assert!(!table_source.contains("h.interact("));
        assert!(table_source.contains("directory_column_resize_delta("));
    }
}
