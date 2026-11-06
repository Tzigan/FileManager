use egui::{Context, CornerRadius, Margin, Style, vec2};

pub(crate) const SPACE_XXS: f32 = 2.0;
pub(crate) const SPACE_XS: f32 = 4.0;
pub(crate) const SPACE_SM: f32 = 8.0;
pub(crate) const SPACE_MD: f32 = 12.0;
pub(crate) const SPACE_LG: f32 = 16.0;
pub(crate) const SPACE_EMPTY_STATE: f32 = 60.0;
pub(crate) const TAB_BAR_VERTICAL_SPACING: f32 = 0.0;

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

pub(crate) fn configure(ctx: &Context) {
    ctx.all_styles_mut(configure_style);
}

pub(crate) fn dense_vertical_spacing(style: &mut Style) {
    style.spacing.item_spacing.y = SPACE_XXS;
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
}
