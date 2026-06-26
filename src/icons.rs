use egui::{Color32, CornerRadius, Painter, Rect, Stroke, Vec2};

const HEALTH_CHECK_ICON_SIZE: [usize; 2] = [32, 32];
const HEALTH_CHECK_ICON_RGBA: &[u8] = include_bytes!("assets/health_check_icon_32.rgba");
const APP_ICON_SIZE: u32 = 256;
const APP_ICON_RGBA: &[u8] = include_bytes!("assets/app_icon_256.rgba");

/// Shell 图标加载失败时的简易占位
pub fn paint_fallback_icon(painter: &Painter, rect: Rect, is_dir: bool, is_drive: bool) {
    if is_drive {
        paint_drive(painter, rect);
    } else if is_dir {
        paint_folder(painter, rect);
    } else {
        paint_file(painter, rect);
    }
}

fn paint_drive(painter: &Painter, rect: Rect) {
    let body = rect.shrink2(Vec2::new(1.0, 3.0));
    painter.rect_filled(
        body,
        CornerRadius::same(2),
        Color32::from_rgb(100, 110, 130),
    );
    painter.rect_stroke(
        body,
        CornerRadius::same(2),
        Stroke::new(1.0, Color32::from_rgb(60, 70, 90)),
        egui::StrokeKind::Outside,
    );
}

fn paint_folder(painter: &Painter, rect: Rect) {
    let tab_h = rect.height() * 0.28;
    let tab = Rect::from_min_size(rect.min, Vec2::new(rect.width() * 0.45, tab_h));
    let body = Rect::from_min_max(rect.min + Vec2::new(0.0, tab_h * 0.55), rect.max);
    let yellow = Color32::from_rgb(255, 205, 60);
    let dark = Color32::from_rgb(210, 160, 30);
    painter.rect_filled(body, CornerRadius::same(2), yellow);
    painter.rect_filled(tab, CornerRadius::same(2), Color32::from_rgb(255, 220, 90));
    painter.rect_stroke(
        body,
        CornerRadius::same(2),
        Stroke::new(1.0, dark),
        egui::StrokeKind::Outside,
    );
}

fn paint_file(painter: &Painter, rect: Rect) {
    let mut points = vec![
        rect.left_top() + Vec2::new(2.0, 1.0),
        rect.right_top() + Vec2::new(-2.0, 1.0),
        rect.right_bottom() + Vec2::new(-2.0, -1.0),
        rect.left_bottom() + Vec2::new(2.0, -1.0),
    ];
    let fold = rect.right_top() + Vec2::new(-5.0, 1.0);
    points[1] = fold + Vec2::new(0.0, 5.0);
    painter.add(egui::Shape::convex_polygon(
        points,
        Color32::WHITE,
        Stroke::new(1.0, Color32::from_rgb(120, 130, 150)),
    ));
}

/// 横向占比条（FolderSize 风格）
pub fn size_bar(ui: &mut egui::Ui, fraction: f32, color: Color32, width: f32) {
    let height = 14.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(2),
        Color32::from_rgb(235, 235, 240),
    );
    let fill_w =
        (rect.width() * fraction.clamp(0.0, 1.0)).max(if fraction > 0.0 { 2.0 } else { 0.0 });
    let fill = Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
    painter.rect_filled(fill, CornerRadius::same(2), color);
}

fn health_check_image(ui: &egui::Ui) -> egui::Image<'static> {
    let image =
        egui::ColorImage::from_rgba_unmultiplied(HEALTH_CHECK_ICON_SIZE, HEALTH_CHECK_ICON_RGBA);
    let texture = ui
        .ctx()
        .load_texture("health-check-icon", image, egui::TextureOptions::LINEAR);
    egui::Image::new(&texture).fit_to_exact_size(Vec2::new(16.0, 16.0))
}

pub fn health_check_button(
    ui: &egui::Ui,
    label: impl Into<egui::WidgetText>,
) -> egui::Button<'static> {
    egui::Button::image_and_text(health_check_image(ui), label)
        .min_size(Vec2::new(72.0, 24.0))
        .fill(Color32::from_rgb(238, 242, 247))
        .stroke(Stroke::NONE)
}

pub fn app_icon_data() -> egui::IconData {
    egui::IconData {
        rgba: APP_ICON_RGBA.to_vec(),
        width: APP_ICON_SIZE,
        height: APP_ICON_SIZE,
    }
}

pub fn bar_color(index: usize) -> Color32 {
    const PALETTE: [Color32; 8] = [
        Color32::from_rgb(55, 95, 160),
        Color32::from_rgb(70, 130, 180),
        Color32::from_rgb(90, 150, 200),
        Color32::from_rgb(110, 170, 210),
        Color32::from_rgb(130, 185, 215),
        Color32::from_rgb(150, 195, 220),
        Color32::from_rgb(170, 205, 225),
        Color32::from_rgb(190, 215, 235),
    ];
    PALETTE[index % PALETTE.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_check_icon_asset_has_expected_rgba_size() {
        assert_eq!(
            HEALTH_CHECK_ICON_RGBA.len(),
            HEALTH_CHECK_ICON_SIZE[0] * HEALTH_CHECK_ICON_SIZE[1] * 4
        );
    }

    #[test]
    fn app_icon_asset_has_expected_rgba_size() {
        assert_eq!(
            APP_ICON_RGBA.len(),
            APP_ICON_SIZE as usize * APP_ICON_SIZE as usize * 4
        );
        let icon = app_icon_data();
        assert_eq!(icon.width, APP_ICON_SIZE);
        assert_eq!(icon.height, APP_ICON_SIZE);
        assert!(!icon.rgba.is_empty());
    }
}
