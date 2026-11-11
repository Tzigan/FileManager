use egui::{Context, CornerRadius, Margin, Style, vec2};

pub(crate) const SPACE_XXS: f32 = 2.0;
pub(crate) const SPACE_XS: f32 = 4.0;
pub(crate) const SPACE_SM: f32 = 8.0;
pub(crate) const SPACE_MD: f32 = 12.0;
pub(crate) const SPACE_LG: f32 = 16.0;
pub(crate) const SPACE_EMPTY_STATE: f32 = 60.0;
pub(crate) const TAB_BAR_VERTICAL_SPACING: f32 = 0.0;
pub(crate) const CONTEXT_MENU_ITEM_HEIGHT: f32 = 20.0;

pub(crate) const RADIUS_ICON: f32 = 2.0;
pub(crate) const RADIUS_ROW: f32 = 4.0;
pub(crate) const RADIUS_CONTROL: f32 = 6.0;
pub(crate) const RADIUS_ICON_FINE: f32 = 1.5;
pub(crate) const RADIUS_WINDOW: u8 = 10;

pub(crate) const FRAME_PADDING_X: i8 = SPACE_SM as i8;
pub(crate) const FRAME_PADDING_Y: i8 = SPACE_XS as i8;
pub(crate) const TOOLBAR_BUTTON_SIZE: f32 = 28.0;
pub(crate) const TOOLBAR_ICON_SIZE: f32 = 14.0;
pub(crate) const TREE_INDENT: f32 = 16.0;
pub(crate) const TREE_DISCLOSURE_SIZE: f32 = 18.0;
pub(crate) const TREE_ICON_WIDTH: f32 = 20.0;
pub(crate) const TREE_ICON_HEIGHT: f32 = 18.0;

/// Доля занятости и sRGB-цвет. Зелёный держится до половины диска:
/// иначе переход к янтарю с нуля делает уже 43% жёлтым.
const DISK_USAGE_COLOR_STOPS: &[(f32, [u8; 3])] = &[
    (0.0, [46, 160, 90]),
    (0.50, [46, 160, 90]),
    (0.75, [214, 158, 46]),
    (0.90, [204, 68, 62]),
];

/// Цвет заполненной части полоски. Считается по доле 0..1, не по округлённому проценту.
pub(crate) fn disk_usage_fill(ratio: f32) -> egui::Color32 {
    let ratio = ratio.clamp(0.0, 1.0);
    let stops = DISK_USAGE_COLOR_STOPS;
    let first = stops[0];
    if ratio <= first.0 {
        return srgb(first.1);
    }
    for pair in stops.windows(2) {
        let (start, start_color) = pair[0];
        let (end, end_color) = pair[1];
        if ratio <= end {
            let span = end - start;
            let t = if span <= f32::EPSILON {
                1.0
            } else {
                (ratio - start) / span
            };
            return lerp_srgb(start_color, end_color, t);
        }
    }
    srgb(stops[stops.len() - 1].1)
}

fn srgb(channels: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(channels[0], channels[1], channels[2])
}

/// egui хранит пиксели в sRGB, а смешивает в linear RGB. Так 43% и 60% не схлопываются в один оттенок.
fn lerp_srgb(from: [u8; 3], to: [u8; 3], t: f32) -> egui::Color32 {
    let from = egui::Rgba::from(srgb(from));
    let to = egui::Rgba::from(srgb(to));
    let mix = |a: f32, b: f32| a + (b - a) * t;
    egui::Color32::from(egui::Rgba::from_rgba_unmultiplied(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
        1.0,
    ))
}

pub(crate) fn configure(ctx: &Context) {
    ctx.all_styles_mut(configure_style);
}

pub(crate) fn dense_vertical_spacing(style: &mut Style) {
    style.spacing.item_spacing.y = SPACE_XXS;
}

pub(crate) fn compact_context_menu(ui: &mut egui::Ui) {
    compact_context_menu_style(ui.style_mut());
}

fn compact_context_menu_style(style: &mut Style) {
    style.spacing.item_spacing.y = SPACE_XXS;
    style.spacing.interact_size.y = CONTEXT_MENU_ITEM_HEIGHT;
}

fn configure_style(style: &mut Style) {
    style.spacing.item_spacing = vec2(SPACE_SM, SPACE_SM);
    style.spacing.window_margin = Margin::same(SPACE_LG as i8);
    style.spacing.menu_margin = Margin::same(SPACE_SM as i8);
    style.spacing.button_padding = vec2(SPACE_SM, SPACE_XS);
    style.spacing.indent = TREE_INDENT;
    style.spacing.interact_size = vec2(TOOLBAR_BUTTON_SIZE, TOOLBAR_BUTTON_SIZE);
    style.spacing.extra_text_line_spacing = SPACE_XXS;
    style.spacing.menu_spacing = SPACE_SM;
    style.spacing.scroll.content_margin = Margin::same(SPACE_SM as i8);

    let visuals = &mut style.visuals;
    visuals.window_corner_radius = CornerRadius::same(RADIUS_WINDOW);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS_WINDOW);

    let widget_radius = CornerRadius::same(RADIUS_CONTROL as u8);
    visuals.widgets.noninteractive.corner_radius = widget_radius;
    visuals.widgets.inactive.corner_radius = widget_radius;
    visuals.widgets.hovered.corner_radius = widget_radius;
    visuals.widgets.active.corner_radius = widget_radius;
    visuals.widgets.open.corner_radius = widget_radius;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_and_corner_tokens_apply_to_both_themes() {
        let ctx = Context::default();
        configure(&ctx);

        for (theme, is_dark) in [(egui::Theme::Light, false), (egui::Theme::Dark, true)] {
            let style = ctx.style_of(theme);
            assert_eq!(style.spacing.item_spacing, vec2(SPACE_SM, SPACE_SM));
            assert_eq!(style.spacing.window_margin, Margin::same(SPACE_LG as i8));
            assert_eq!(style.spacing.menu_margin, Margin::same(SPACE_SM as i8));
            assert_eq!(style.spacing.button_padding, vec2(SPACE_SM, SPACE_XS));
            assert_eq!(
                style.spacing.interact_size,
                vec2(TOOLBAR_BUTTON_SIZE, TOOLBAR_BUTTON_SIZE)
            );
            assert_eq!(style.spacing.indent, TREE_INDENT);
            assert_eq!(style.spacing.menu_spacing, SPACE_SM);
            assert_eq!(
                style.spacing.scroll.content_margin,
                Margin::same(SPACE_SM as i8)
            );
            assert_eq!(
                style.visuals.window_corner_radius,
                CornerRadius::same(RADIUS_WINDOW)
            );
            assert_eq!(
                style.visuals.menu_corner_radius,
                CornerRadius::same(RADIUS_WINDOW)
            );
            assert_eq!(
                style.visuals.widgets.hovered.corner_radius,
                CornerRadius::same(RADIUS_CONTROL as u8)
            );
            assert_eq!(style.visuals.dark_mode, is_dark);
        }
    }

    #[test]
    fn dense_layout_reduces_only_vertical_item_spacing() {
        let mut style = Style::default();
        style.spacing.item_spacing = vec2(SPACE_LG, SPACE_LG);

        dense_vertical_spacing(&mut style);

        assert_eq!(style.spacing.item_spacing.x, SPACE_LG);
        assert_eq!(style.spacing.item_spacing.y, SPACE_XXS);
    }

    #[test]
    fn context_menu_rows_use_compact_height_and_gap() {
        let mut style = Style::default();
        style.spacing.interact_size = vec2(TOOLBAR_BUTTON_SIZE, TOOLBAR_BUTTON_SIZE);
        style.spacing.item_spacing = vec2(SPACE_SM, SPACE_LG);

        compact_context_menu_style(&mut style);

        assert_eq!(style.spacing.interact_size.x, TOOLBAR_BUTTON_SIZE);
        assert_eq!(style.spacing.interact_size.y, CONTEXT_MENU_ITEM_HEIGHT);
        assert_eq!(style.spacing.item_spacing.x, SPACE_SM);
        assert_eq!(style.spacing.item_spacing.y, SPACE_XXS);
    }

    #[test]
    fn disk_usage_color_changes_continuously_and_holds_the_warning_color() {
        let calm = disk_usage_fill(0.0);
        let amber = disk_usage_fill(0.75);
        let warning = disk_usage_fill(0.90);
        assert_eq!(disk_usage_fill(0.43), calm);
        assert_eq!(disk_usage_fill(1.0), warning);
        assert_ne!(disk_usage_fill(0.60), calm);
        assert_ne!(disk_usage_fill(0.60), amber);
        assert_ne!(calm, amber);
        assert_ne!(amber, warning);
    }
}
