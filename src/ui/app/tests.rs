//! Тесты расположения вкладок, навигации и окон.
use super::{
    PreviewState, breadcrumb_segments, configure_tab_scroll_style, dropped_paths,
    floating_dialog_viewport, is_tab_dragging, parse_archive_address, persisted_tab_path,
    tab_close_button_hovered, tab_scroll_source, tab_strip_content_margin,
};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct MockDrop(PathBuf);

impl egui::DroppedFile for MockDrop {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(vec![])
    }
}

#[test]
fn dropped_paths_collects() {
    let files: Vec<egui::DroppedFileHandle> = vec![
        std::sync::Arc::new(MockDrop(PathBuf::from("C:\\a.txt"))),
        std::sync::Arc::new(MockDrop(PathBuf::from("D:\\b.txt"))),
    ];
    let got = dropped_paths(&files);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], PathBuf::from("C:\\a.txt"));
    assert_eq!(got[1], PathBuf::from("D:\\b.txt"));
}

#[test]
fn breadcrumbs_split_path() {
    #[cfg(windows)]
    {
        let segs = breadcrumb_segments(Path::new("C:\\a\\b"));
        assert_eq!(
            segs.iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>(),
            vec!["C:\\", "a", "b"]
        );
        assert_eq!(segs[0].0, "C:\\");
        assert_eq!(segs[0].1, PathBuf::from("C:\\"));
        let last = segs.last().unwrap();
        assert_eq!(last.0, "b");
        assert!(last.1.ends_with("b"));
    }
}

#[test]
fn archive_address_round_trips_the_internal_folder() {
    let (archive, prefix) = parse_archive_address(r"C:\Downloads\files.zip!/docs/notes").unwrap();
    assert_eq!(archive, PathBuf::from(r"C:\Downloads\files.zip"));
    assert_eq!(prefix, "docs/notes");
}

#[test]
fn archive_tabs_restore_the_containing_filesystem_folder() {
    let mut tab = super::Tab::new(PathBuf::from(r"C:\Downloads\files.zip"));
    tab.archive_prefix = Some("docs".to_owned());
    assert_eq!(persisted_tab_path(&tab), PathBuf::from(r"C:\Downloads"));
}

#[test]
fn tab_strip_uses_mouse_wheel_without_showing_a_scrollbar() {
    let source = tab_scroll_source();
    assert!(source.mouse_wheel);
    assert!(!source.scroll_bar);
    assert_eq!(source.drag, egui::scroll_area::DragScroll::Never);

    let mut style = egui::Style::default();
    configure_tab_scroll_style(&mut style);
    assert!(style.always_scroll_the_only_direction);
    assert_eq!(
        style.spacing.item_spacing.y,
        super::style::TAB_BAR_VERTICAL_SPACING
    );
    let margin = tab_strip_content_margin();
    assert_eq!(margin.left, super::style::FRAME_PADDING_X);
    assert_eq!(margin.right, super::style::FRAME_PADDING_X);
    assert_eq!(margin.top, 0);
    assert_eq!(margin.bottom, 0);
}

#[test]
fn dragging_over_another_tabs_close_control_does_not_activate_it() {
    let close_rect = egui::Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(18.0, 24.0));
    let pointer = Some(close_rect.center());
    assert!(tab_close_button_hovered(false, false, close_rect, pointer));
    assert!(!tab_close_button_hovered(true, false, close_rect, pointer));
    assert!(!tab_close_button_hovered(false, true, close_rect, pointer));

    let press = Some((0, egui::pos2(10.0, 10.0)));
    assert!(!is_tab_dragging(None, press, Some(egui::pos2(15.0, 10.0))));
    assert!(is_tab_dragging(None, press, Some(egui::pos2(16.0, 10.0))));
    assert!(is_tab_dragging(Some(0), None, None));
}

#[test]
fn detached_dialog_viewports_are_topmost_and_keep_user_moved_positions() {
    let ctx = egui::Context::default();
    let size = egui::vec2(480.0, 280.0);
    let initial = floating_dialog_viewport(&ctx, "About".into(), size, false);
    assert_eq!(
        initial.window_level,
        Some(egui::viewport::WindowLevel::AlwaysOnTop)
    );
    assert_eq!(initial.inner_size, Some(size));
    assert_eq!(initial.maximize_button, Some(false));
    assert!(initial.position.is_some());

    let moved = floating_dialog_viewport(&ctx, "About".into(), size, true);
    assert_eq!(
        moved.window_level,
        Some(egui::viewport::WindowLevel::AlwaysOnTop)
    );
    assert_eq!(moved.maximize_button, Some(false));
    assert_eq!(moved.position, None);
}

#[test]
fn window_frame_maps_resize_edges_and_drop_targets() {
    use super::window_frame::{release_is_outside_frame, resize_direction_at};

    let frame = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
    assert_eq!(
        resize_direction_at(frame, egui::pos2(1.0, 1.0)),
        Some(egui::viewport::ResizeDirection::NorthWest)
    );
    assert_eq!(
        resize_direction_at(frame, egui::pos2(399.0, 299.0)),
        Some(egui::viewport::ResizeDirection::SouthEast)
    );
    assert_eq!(resize_direction_at(frame, frame.center()), None);

    assert!(!release_is_outside_frame(frame, Some(frame.center())));
    assert!(release_is_outside_frame(
        frame,
        Some(egui::pos2(401.0, 140.0))
    ));
    assert!(release_is_outside_frame(frame, None));
}

#[test]
fn main_window_drag_area_avoids_resize_edges() {
    use super::window_frame::{
        MAIN_WINDOW_DRAG_AREA_MIN_WIDTH, WINDOW_DRAG_AREA_MIN_WIDTH, WINDOW_FRAME_HEIGHT,
        WINDOW_RESIZE_HANDLE_WIDTH, main_window_drag_hit_rect,
    };

    let frame = egui::Rect::from_min_size(
        egui::pos2(-400.0, -200.0),
        egui::vec2(640.0, WINDOW_FRAME_HEIGHT),
    );
    let drag_area = main_window_drag_hit_rect(frame);

    assert_eq!(drag_area.left(), frame.left());
    assert_eq!(drag_area.right(), frame.right());
    assert!(drag_area.top() > frame.top() + WINDOW_RESIZE_HANDLE_WIDTH);
    assert!(drag_area.bottom() < frame.bottom() - WINDOW_RESIZE_HANDLE_WIDTH);
    assert!(drag_area.contains(frame.center()));
    assert!(MAIN_WINDOW_DRAG_AREA_MIN_WIDTH > WINDOW_DRAG_AREA_MIN_WIDTH);
}

#[test]
fn new_tab_button_size_tracks_tab_text_and_click_target() {
    use super::window_frame::{
        TAB_PLUS_BUTTON_WIDTH, TAB_VERTICAL_EXTRA_HEIGHT, new_tab_button_size,
    };

    let compact = new_tab_button_size(14.0, 20.0);
    assert_eq!(compact.x, TAB_PLUS_BUTTON_WIDTH);
    assert_eq!(compact.y, 20.0);

    let larger_font = new_tab_button_size(18.0, 20.0);
    assert_eq!(larger_font.y, 18.0 + TAB_VERTICAL_EXTRA_HEIGHT);
}

#[test]
fn tab_drag_preview_converts_negative_screen_coordinates_and_preserves_drop_slots() {
    use super::tab_drag_preview::{
        preview_window_position, screen_position_to_viewport, tab_drop_slot_at,
    };

    let local = screen_position_to_viewport((-1500, -150), egui::pos2(-1280.0, -200.0), 1.5);
    assert_eq!(local, egui::pos2(280.0, 100.0));
    assert_eq!(
        preview_window_position((-1500, -150), egui::vec2(10.0, -16.0), 1.5),
        (-1515, -126)
    );

    let tabs = [
        (
            0,
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 30.0)),
        ),
        (
            1,
            egui::Rect::from_min_size(egui::pos2(112.0, 10.0), egui::vec2(100.0, 30.0)),
        ),
    ];
    assert_eq!(tab_drop_slot_at(egui::pos2(20.0, 20.0), &tabs), Some(0));
    assert_eq!(tab_drop_slot_at(egui::pos2(100.0, 20.0), &tabs), Some(1));
    assert_eq!(tab_drop_slot_at(egui::pos2(300.0, 20.0), &tabs), None);
}

#[test]
fn detached_window_position_stays_on_the_release_monitor() {
    use super::tab_drag_preview::detached_window_position;

    let right_monitor = detached_window_position(
        (3830, 1060),
        1.5,
        egui::vec2(1000.0, 640.0),
        (1920, 0, 3840, 1080),
    );
    assert_eq!(right_monitor, (2328, 108));

    let left_monitor = detached_window_position(
        (-10, -190),
        1.0,
        egui::vec2(1000.0, 640.0),
        (-1920, -200, 0, 1080),
    );
    assert_eq!(left_monitor, (-1008, -182));
}

#[test]
fn detached_tab_viewport_is_borderless_resizable_and_keeps_its_size() {
    use super::window_frame::detached_viewport_builder_at;

    let release_position = egui::pos2(-720.0, 140.0);
    let initial = detached_viewport_builder_at("Detached".into(), false, Some(release_position));
    assert_eq!(initial.decorations, Some(false));
    assert_eq!(initial.resizable, Some(true));
    assert_eq!(initial.inner_size, Some(egui::vec2(1000.0, 640.0)));
    assert_eq!(initial.position, Some(release_position));
    assert_eq!(initial.visible, Some(false));

    let placed = detached_viewport_builder_at("Detached".into(), true, None);
    assert_eq!(placed.inner_size, None);
    assert_eq!(placed.position, None);
    assert_eq!(placed.visible, None);
}

#[test]
fn window_control_visual_states_and_maximize_icon_follow_viewport_state() {
    use super::window_frame::{
        WINDOW_CONTROL_GAP, WINDOW_CONTROL_HEIGHT, WINDOW_CONTROL_VERTICAL_GAP,
        WINDOW_CONTROL_WIDTH, WINDOW_CONTROLS_WIDTH, WINDOW_FRAME_HEIGHT,
        WINDOW_RESIZE_HANDLE_WIDTH, WindowControlIcon, maximize_restore_icon,
        window_control_background,
    };

    let visuals = egui::Visuals::default();
    let idle = window_control_background(&visuals, WindowControlIcon::Minimize, true, 0.0, 0.0);
    let hover = window_control_background(&visuals, WindowControlIcon::Minimize, true, 1.0, 0.0);
    let pressed = window_control_background(&visuals, WindowControlIcon::Minimize, true, 0.0, 1.0);
    let close_hover = window_control_background(&visuals, WindowControlIcon::Close, true, 1.0, 0.0);
    let close_pressed =
        window_control_background(&visuals, WindowControlIcon::Close, true, 0.0, 1.0);
    let disabled =
        window_control_background(&visuals, WindowControlIcon::Maximize, false, 1.0, 0.0);

    assert_eq!(idle, egui::Color32::TRANSPARENT);
    assert_ne!(hover, idle);
    assert_ne!(pressed, hover);
    assert_ne!(close_hover, hover);
    assert_ne!(close_pressed, close_hover);
    assert_eq!(disabled, egui::Color32::TRANSPARENT);
    assert_eq!(maximize_restore_icon(false), WindowControlIcon::Maximize);
    assert_eq!(maximize_restore_icon(true), WindowControlIcon::Restore);
    assert_eq!(
        WINDOW_CONTROL_HEIGHT,
        WINDOW_FRAME_HEIGHT - WINDOW_RESIZE_HANDLE_WIDTH * 2.0 - WINDOW_CONTROL_VERTICAL_GAP * 2.0
    );
    assert_eq!(
        WINDOW_CONTROLS_WIDTH,
        WINDOW_CONTROL_WIDTH * 3.0 + WINDOW_CONTROL_GAP * 2.0
    );
}

#[test]
fn preview_viewport_can_resize_and_keeps_its_user_size() {
    let ctx = egui::Context::default();
    let initial = super::preview::preview_viewport_builder(&ctx, "Preview".into(), false);
    assert_eq!(initial.inner_size, Some(egui::vec2(760.0, 540.0)));
    assert!(initial.position.is_some());
    assert_eq!(initial.resizable, Some(true));
    assert_eq!(initial.maximize_button, Some(true));
    assert_eq!(
        initial.window_level,
        Some(egui::viewport::WindowLevel::AlwaysOnTop)
    );

    let placed = super::preview::preview_viewport_builder(&ctx, "Preview".into(), true);
    assert_eq!(placed.inner_size, None);
    assert_eq!(placed.position, None);
    assert_eq!(placed.resizable, Some(true));
    assert_eq!(placed.maximize_button, Some(true));
}

#[test]
fn side_mouse_buttons_map_to_navigation_without_changing_standard_buttons() {
    use super::input::{MouseNavigationAction, map_mouse_button_to_navigation};

    assert_eq!(
        map_mouse_button_to_navigation(egui::PointerButton::Extra1),
        Some(MouseNavigationAction::Back)
    );
    assert_eq!(
        map_mouse_button_to_navigation(egui::PointerButton::Extra2),
        Some(MouseNavigationAction::Forward)
    );
    for button in [
        egui::PointerButton::Primary,
        egui::PointerButton::Secondary,
        egui::PointerButton::Middle,
    ] {
        assert_eq!(map_mouse_button_to_navigation(button), None);
    }
}

#[test]
fn clearing_preview_resets_floating_viewport_state() {
    let mut preview = PreviewState::default();
    preview.target = Some(PathBuf::from("preview.txt"));
    preview.viewport_placed = true;

    preview.clear();

    assert!(preview.target.is_none());
    assert!(!preview.viewport_placed);
}

#[test]
fn update_percent_is_bounded_and_handles_zero_total() {
    let percent = super::dialogs::update_percent;

    assert_eq!(percent(0, 100), 0);
    assert_eq!(percent(25, 100), 25);
    assert_eq!(percent(50, 100), 50);
    assert_eq!(percent(100, 100), 100);
    assert_eq!(percent(125, 100), 100);
    assert_eq!(percent(0, 0), 0);
}

#[test]
fn drive_root_is_one_absolute_breadcrumb() {
    #[cfg(windows)]
    {
        let segs = breadcrumb_segments(Path::new("C:\\"));
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "C:\\");
        assert_eq!(segs[0].1, PathBuf::from("C:\\"));
    }
}
