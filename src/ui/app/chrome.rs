//! Значки панели и расчёт полосы вкладок.

use super::*;

pub(crate) fn tab_scroll_source() -> egui::scroll_area::ScrollSource {
    egui::scroll_area::ScrollSource::MOUSE_WHEEL
}

pub(crate) fn tab_close_button_hovered(
    dragging: bool,
    pinned: bool,
    close_rect: egui::Rect,
    pointer: Option<egui::Pos2>,
) -> bool {
    !dragging && !pinned && pointer.is_some_and(|pos| close_rect.contains(pos))
}

pub(crate) fn tab_strip_content_margin() -> egui::Margin {
    // Кнопка «+» снаружи прокрутки, поэтому справа дополнительный отступ не нужен.
    egui::Margin {
        left: style::FRAME_PADDING_X,
        right: 0,
        top: 0,
        bottom: 0,
    }
}

pub(crate) fn configure_tab_scroll_style(style: &mut egui::Style) {
    // В обычных горизонтальных областях egui прокрутка колесом требует Shift.
    // Для полосы вкладок колесо прокручивает список напрямую.
    style.always_scroll_the_only_direction = true;
    // Высота панели подстраивается к строке, не меняя размеры вкладок и кнопки «+».
    style.spacing.item_spacing.y = super::style::TAB_BAR_VERTICAL_SPACING;
}

/// Квадратная кнопка панели: глиф рисуется вручную, без символов шрифта.
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    hover: String,
    draw: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let button_size = style::TOOLBAR_BUTTON_SIZE;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(button_size, button_size), sense);
    let resp = resp.on_hover_text(hover);
    let visuals = ui.visuals();
    let painter = ui.painter();
    if enabled && resp.hovered() {
        painter.rect_filled(rect, style::RADIUS_CONTROL, visuals.widgets.hovered.bg_fill);
    }
    let color = if !enabled {
        visuals.weak_text_color().gamma_multiply(0.4)
    } else if resp.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    let icon_inset = (button_size - style::TOOLBAR_ICON_SIZE) * 0.5;
    draw(painter, rect.shrink(icon_inset), color);
    resp
}

pub(crate) fn icon_pin(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let x = rect.center().x;
    let head = egui::pos2(x, rect.top() + 3.0);
    painter.circle_filled(head, 2.2, color);
    painter.line_segment(
        [egui::pos2(x, rect.top() + 5.0), egui::pos2(x, rect.bottom() - 1.0)],
        egui::Stroke::new(1.3, color),
    );
    painter.line_segment(
        [
            egui::pos2(x - 3.0, rect.top() + 7.0),
            egui::pos2(x + 3.0, rect.top() + 7.0),
        ],
        egui::Stroke::new(1.3, color),
    );
}

/// Arrow glyph: `dir` is the tip direction (unit axis vector).
pub(crate) fn icon_arrow(p: &egui::Painter, r: egui::Rect, dir: egui::Vec2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.6, color);
    let c = r.center();
    let tip = c + dir * (r.width() * 0.5);
    let tail = c - dir * (r.width() * 0.42);
    p.line_segment([tail, tip], stroke);
    let perp = egui::vec2(-dir.y, dir.x);
    let w = r.width() * 0.32;
    p.line_segment([tip, tip - dir * w + perp * w], stroke);
    p.line_segment([tip, tip - dir * w - perp * w], stroke);
}

/// Circular arrow (refresh): ~280° arc with a filled triangular tip at top-right.
pub(crate) fn icon_refresh(p: &egui::Painter, r: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.7, color);
    let c = r.center();
    let rad = r.width() * 0.44;
    let pts: Vec<egui::Pos2> = (0..=20)
        .map(|i| {
            let a = 0.9 + std::f32::consts::TAU * 0.78 * (i as f32 / 20.0);
            c + egui::vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    p.add(egui::Shape::line(pts.clone(), stroke));
    let end = *pts.last().unwrap();
    let tan = (end - pts[pts.len() - 2]).normalized();
    let perp = egui::vec2(-tan.y, tan.x);
    p.add(egui::Shape::convex_polygon(
        vec![end, end - tan * 5.0 + perp * 2.6, end - tan * 5.0 - perp * 2.6],
        color,
        egui::Stroke::NONE,
    ));
}

/// Звезда закладки. Контур один и тот же: «не добавлено» — только обводка,
/// «добавлено» — та же фигура, залитая цветом обводки. Размер задаёт прямоугольник кнопки.
pub(crate) fn icon_star(p: &egui::Painter, r: egui::Rect, color: egui::Color32, filled: bool) {
    let center = r.center();
    let outer = r.width() * 0.5;
    let inner = outer * 0.42;
    let points: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let radius = if i % 2 == 0 { outer } else { inner };
            let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    let stroke = egui::Stroke::new(1.4, color);
    if filled {
        // Выпуклая заливка склеивала лучи в пятиугольник. Треугольники от центра
        // заполняют именно внутренность этого контура тем же цветом.
        for index in 0..points.len() {
            let next = points[(index + 1) % points.len()];
            p.add(egui::Shape::convex_polygon(
                vec![center, points[index], next],
                color,
                egui::Stroke::NONE,
            ));
        }
    }
    let mut closed = points;
    closed.push(closed[0]);
    p.add(egui::Shape::line(closed, stroke));
}
