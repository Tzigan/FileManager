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
fn clearing_preview_resets_floating_viewport_state() {
    let mut preview = PreviewState::default();
    preview.target = Some(PathBuf::from("preview.txt"));
    preview.viewport_placed = true;

    preview.clear();

    assert!(preview.target.is_none());
    assert!(!preview.viewport_placed);
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
